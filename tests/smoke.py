"""End-to-end checks against generated, authorized loopback fixtures. No third-party media."""
import contextlib, json, os, pathlib, shutil, subprocess, sys, tempfile, threading, uuid
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import urlparse, parse_qs

binary = pathlib.Path(sys.argv[1]).resolve()
counts = {}
payloads = []
stop_fixture = threading.Event()
@contextlib.contextmanager
def fixture_directory():
    parent = pathlib.Path(os.environ.get('ANI_TEST_ROOT', tempfile.gettempdir())).resolve()
    path = parent / ('ani-rust-test-' + uuid.uuid4().hex)
    path.mkdir()
    try: yield path
    finally:
        assert path.resolve().parent == parent and path.name.startswith('ani-rust-test-')
        shutil.rmtree(path)
with fixture_directory() as temp:
    root = pathlib.Path(temp)
    subprocess.run(['ffmpeg','-hide_banner','-loglevel','error','-f','lavfi','-i','color=c=blue:s=160x90:r=10','-t','1','-c:v','libx264','-f','hls','-hls_time','0.5','-hls_list_size','0',str(root/'media.m3u8')],check=True)
    class Handler(BaseHTTPRequestHandler):
        def send(self, status, body, content='application/json'):
            body = body.encode() if isinstance(body,str) else body
            self.send_response(status); self.send_header('Content-Type',content); self.send_header('Content-Length',str(len(body))); self.end_headers(); self.wfile.write(body)
        def do_POST(self):
            payload = json.loads(self.rfile.read(int(self.headers['Content-Length'])))
            payloads.append(payload)
            assert payload['operationName'] == 'FastSearch'
            assert 'catalogAnime' in payload['query']
            query = payload['variables']['query']
            if query == 'error': return self.send(200,json.dumps({'errors':[{'message':'fixture failure'}]}))
            if query == 'empty': return self.send(200,json.dumps({'data':{'catalogAnime':{'items':[]}}}))
            items = [{'id':'fixture-internal-id','anilistId':42,'malId':43,'titleEnglish':'Fixture title','episodeCount':12,'seasonYear':2026,'format':'TV','status':'FINISHED','genres':['Mystery']}]
            self.send(200,json.dumps({'data':{'catalogAnime':{'items':items}}}))
        def do_GET(self):
            path = urlparse(self.path).path
            if path == '/shutdown' and os.environ.get('ANI_TUI_FIXTURE'):
                self.send(200,'done'); stop_fixture.set(); return
            counts[path] = counts.get(path,0)+1
            if path == '/api':
                q = parse_qs(urlparse(self.path).query)
                assert q['type'] == ['sub'] and q['providerId'] == ['beep']
                encrypted = q['id'] == ['encrypted']
                playlist = '/encrypted.m3u8' if encrypted else '/master.m3u8'
                return self.send(200,json.dumps({'sources':[{'url':base+playlist}], 'headers':{'Referer':'https://authorized.example/','X-Fixture':'required'},'tracks':[{'file':base+'/english.vtt','label':'English','kind':'captions'}]}))
            if self.headers.get('Referer') != 'https://authorized.example/' or self.headers.get('X-Fixture') != 'required': return self.send(403,'missing headers')
            if path == '/master.m3u8': return self.send(200,'#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=100,RESOLUTION=160x90\nmedia.m3u8\n#EXT-X-STREAM-INF:BANDWIDTH=50,RESOLUTION=80x45\nlow.m3u8\n')
            if path == '/encrypted.m3u8': return self.send(200,'#EXTM3U\n#EXT-X-KEY:METHOD=AES-128,URI="key"\n#EXTINF:1,\nsecret.ts\n#EXT-X-ENDLIST\n')
            if path == '/english.vtt': return self.send(200,'WEBVTT\n\n00:00.000 --> 00:00.500\nFixture\n')
            if path.endswith('.ts') and counts[path] == 1: return self.send(503,'retry fixture')
            file = root/path.lstrip('/')
            if file.is_file(): return self.send(200,file.read_bytes(),'application/octet-stream')
            self.send(404,'missing')
        def log_message(self,*args): pass
    server = ThreadingHTTPServer(('127.0.0.1',0),Handler)
    base = f'http://127.0.0.1:{server.server_port}'
    thread = threading.Thread(target=server.serve_forever,daemon=True); thread.start()
    def run(*args,ok=True):
        p = subprocess.run([str(binary),'--plain','--api-url',base+'/api','--graphql-url',base+'/graphql',*args],capture_output=True,text=True,timeout=60)
        assert (p.returncode == 0) == ok, (args,p.returncode,p.stdout,p.stderr)
        return p
    p = run('search','arbitrary title')
    assert 'fixture-internal-id' in p.stdout and 'Fixture title' in p.stdout
    assert payloads[-1]['variables'] == {'query':'arbitrary title','limit':10,'includeAdult':False}
    p = run('--debug','search','another','--limit','2','--include-adult','true')
    assert payloads[-1]['variables']['includeAdult'] is True
    assert all(s in p.stderr for s in ['FastSearch','variables','200','results'])
    assert 'X-Fixture' not in p.stderr and 'Referer' not in p.stderr
    run('search','error',ok=False)
    assert 'No anime found' in run('search','empty').stdout
    run('search','title','--limit','0',ok=False)
    p = run('inspect','fixture-internal-id')
    assert len(json.loads(p.stdout)['streams'][0]['qualities']) == 2
    output = root/'downloads'
    args = ['download','fixture-internal-id','--episodes','1,2-3','--output',str(output),'--concurrency','2']
    run(*args)
    manifests = list(output.glob('*/metadata.json'))
    assert len(manifests) == 3
    for file in manifests:
        m = json.loads(file.read_text())
        assert m['complete'] and m['selected_url'].endswith('/media.m3u8')
        assert len(m['artifacts']) == 2
        assert (file.parent/'video.mp4').stat().st_size > 0
    before = dict(counts)
    run(*args)
    assert counts == before, 'skip must avoid API and media requests'
    # Playlist-only must coexist with video outputs and never fetch media/subtitles.
    transfers = {k:v for k,v in counts.items() if k.endswith(('.ts','.vtt','.mp4'))}
    run(*args,'--playlist-only','--ffmpeg','definitely-missing-ffmpeg')
    assert {k:v for k,v in counts.items() if k.endswith(('.ts','.vtt','.mp4'))} == transfers
    playlists = list(output.glob('*-playlist/playlist.m3u8'))
    assert len(playlists) == 3
    for playlist in playlists:
        text = playlist.read_text()
        assert base+'/media0.ts' in text
        manifest = json.loads(playlist.with_name('metadata.json').read_text())
        assert manifest['playlist_only'] is True
        assert manifest['resolved']['headers']['X-Fixture'] == 'required'
        assert len(manifest['artifacts']) == 1
        assert not playlist.with_name('video.mp4').exists()
    before = dict(counts)
    run(*args,'--playlist-only')
    assert counts == before
    run('download','encrypted','--episodes','1','--playlist-only','--output',str(output),ok=False)
    saved = root/'saved.json'
    saved.write_text(json.dumps({'sources':[{'url':base+'/master.m3u8'}], 'headers':{'Referer':'https://authorized.example/','X-Fixture':'required'}, 'tracks':[{'url':base+'/english.vtt','label':'English','kind':'captions'}]}))
    before_api = counts['/api']
    run('inspect','saved-fixture','--source-json',str(saved))
    run('download','saved-fixture','--episodes','7','--source-json',str(saved),'--output',str(output))
    assert counts['/api'] == before_api, 'saved source must not call the resolver API'
    saved_manifest = next(output.glob('*saved-fixture*/metadata.json'))
    assert json.loads(saved_manifest.read_text())['episode'] == 7
    p = run('download','saved-fixture','--episodes','8-9','--source-json',str(saved),'--output',str(output),ok=False)
    assert 'exactly one episode' in p.stderr
    saved.write_text('{invalid json')
    run('inspect','saved-fixture','--source-json',str(saved),ok=False)
    assert counts['/api'] == before_api
    run('download','encrypted','--episodes','1','--output',str(output),ok=False)
    assert '/key' not in counts and '/secret.ts' not in counts
    run('download','fixture-internal-id','--episodes','4','--quality','720p','--output',str(output),ok=False)
    assert not list(output.glob('.*')), 'failed staging should be cleaned up'
    if os.environ.get('ANI_TUI_FIXTURE'):
        saved.write_text(json.dumps({'sources':[{'url':base+'/master.m3u8'}], 'headers':{'Referer':'https://authorized.example/','X-Fixture':'required'},'tracks':[]}))
        pathlib.Path(os.environ['ANI_TUI_FIXTURE']).write_text(json.dumps({'base':base,'output':str(root/'tui-downloads'),'saved':str(saved)}))
        print('TUI fixture ready',flush=True)
        stop_fixture.wait(300)
    server.shutdown()
print('PASS: catalog, debug, inspect, headers, HLS/remux, retries, concurrency, subtitles, manifests, skip, saved JSON and protection')
