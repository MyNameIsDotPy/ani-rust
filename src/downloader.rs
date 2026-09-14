use crate::{
    api::{headers, Http, SourceResolver},
    cli::SourceArgs,
    filenames::sanitize,
    hls,
    models::{protected, Resolved},
    subtitles,
};
use anyhow::{Context, Result};
use reqwest::Url;
use serde::{Deserialize, Serialize};
use std::path::Path;
#[derive(Serialize, Deserialize)]
struct Artifact {
    name: String,
    bytes: u64,
}
#[derive(Serialize, Deserialize)]
struct Manifest {
    complete: bool,
    #[serde(default)]
    playlist_only: bool,
    anime_id: String,
    episode: u32,
    language: String,
    provider: String,
    quality: String,
    selected_url: String,
    resolved: Resolved,
    artifacts: Vec<Artifact>,
}
pub async fn inspect(
    http: &Http,
    resolver: &dyn SourceResolver,
    source: &SourceArgs,
    episode: u32,
) -> Result<()> {
    println!("{}", inspection(http, resolver, source, episode).await?);
    Ok(())
}
pub async fn inspection(
    http: &Http,
    resolver: &dyn SourceResolver,
    source: &SourceArgs,
    episode: u32,
) -> Result<String> {
    let resolved = resolver.resolve(source, episode).await?;
    let headers = headers(&resolved.headers)?;
    let mut streams = Vec::new();
    for stream in &resolved.sources {
        let url = Url::parse(&stream.url)?;
        let protection = protected(&stream.drm) || protected(&resolved.drm);
        let mut item = serde_json::json!({"url":url.as_str(), "protected":protection});
        if is_hls(&url) && !protection {
            match hls::load(http, &url, &headers).await {
                Ok(p) => {
                    item["qualities"] = serde_json::to_value(&p.variants)?;
                }
                Err(e) => item["playlist_error"] = format!("{e:#}").into(),
            }
        }
        streams.push(item);
    }
    Ok(serde_json::to_string_pretty(
        &serde_json::json!({"streams": streams,"headers":resolved.headers,"tracks":resolved.tracks}),
    )?)
}
fn is_hls(url: &Url) -> bool {
    url.path().to_ascii_lowercase().ends_with(".m3u8")
}
async fn complete(dir: &Path, source: &SourceArgs, episode: u32) -> bool {
    let Ok(bytes) = tokio::fs::read(dir.join("metadata.json")).await else {
        return false;
    };
    let Ok(m) = serde_json::from_slice::<Manifest>(&bytes) else {
        return false;
    };
    if !m.complete
        || m.anime_id != source.anime_id
        || m.episode != episode
        || m.language != source.language.to_string()
        || m.provider != source.provider
        || m.quality != source.quality
        || m.playlist_only != source.playlist_only
        || m.artifacts.is_empty()
    {
        return false;
    }
    for a in m.artifacts {
        if a.name.contains(['/', '\\']) || a.name == ".." {
            return false;
        }
        if !tokio::fs::metadata(dir.join(a.name))
            .await
            .is_ok_and(|v| v.is_file() && v.len() == a.bytes && a.bytes > 0)
        {
            return false;
        }
    }
    true
}
pub async fn episode(
    http: &Http,
    resolver: &dyn SourceResolver,
    source: &SourceArgs,
    number: u32,
    output: &Path,
    ffmpeg: &Path,
    progress: indicatif::ProgressBar,
) -> Result<()> {
    let anime_dir = output.join(sanitize(
        source.display_name.as_deref().unwrap_or(&source.anime_id),
    ));
    tokio::fs::create_dir_all(&anime_dir).await?;
    let mut name = format!(
        "{}-{}-ep{number:04}-{}",
        source.language,
        sanitize(&source.provider),
        source.quality
    );
    if source.playlist_only {
        name.push_str("-playlist");
    }
    let target = anime_dir.join(&name);
    if complete(&target, source, number).await {
        progress.finish_with_message(format!("episode {number}: skipped"));
        return Ok(());
    }
    anyhow::ensure!(!target.exists(), "episode directory exists but is incomplete or mismatched: {}; move it aside before retrying", target.display());
    let stage = tempfile::Builder::new()
        .prefix(&format!(".{name}-"))
        .tempdir_in(&anime_dir)?;
    let resolved = resolver.resolve(source, number).await?;
    let stream = &resolved.sources[0];
    anyhow::ensure!(
        !protected(&resolved.drm) && !protected(&stream.drm),
        "encrypted/DRM-protected source is unsupported"
    );
    let headers = headers(&resolved.headers)?;
    let url = Url::parse(&stream.url)?;
    let selected;
    let mut names;
    if is_hls(&url) {
        let playlist = hls::media(
            http,
            hls::load(http, &url, &headers).await?,
            &headers,
            &source.quality,
        )
        .await?;
        selected = playlist.url.to_string();
        if source.playlist_only {
            tokio::fs::write(stage.path().join("playlist.m3u8"), hls::export(&playlist)?).await?;
            progress.set_length(1);
            progress.set_position(1);
        } else {
            let segments = stage.path().join("hls");
            tokio::fs::create_dir(&segments).await?;
            hls::stage(http, &playlist, &headers, &segments, &progress).await?;
            let executable = if ffmpeg.components().count() > 1 {
                std::fs::canonicalize(ffmpeg).context("ffmpeg path does not exist")?
            } else {
                ffmpeg.to_path_buf()
            };
            let result = tokio::process::Command::new(executable)
                .current_dir(&segments)
                .args([
                    "-nostdin",
                    "-hide_banner",
                    "-loglevel",
                    "error",
                    "-xerror",
                    "-n",
                    "-protocol_whitelist",
                    "file",
                    "-allowed_extensions",
                    "ALL",
                    "-i",
                ])
                .arg("local.m3u8")
                .args([
                    "-map",
                    "0:v:0",
                    "-map",
                    "0:a?",
                    "-c",
                    "copy",
                    "-movflags",
                    "+faststart",
                ])
                .arg("../video.mp4")
                .kill_on_drop(true)
                .output()
                .await
                .context("could not run ffmpeg; install it or set --ffmpeg")?;
            anyhow::ensure!(
                result.status.success(),
                "ffmpeg failed: {}",
                String::from_utf8_lossy(&result.stderr)
            );
            tokio::fs::remove_dir_all(&segments).await?;
        }
    } else {
        anyhow::ensure!(
            !source.playlist_only,
            "playlist-only mode requires an HLS .m3u8 source"
        );
        // Restrict direct files: opaque/DASH endpoints are not sent to ffmpeg.
        anyhow::ensure!(
            url.path().to_ascii_lowercase().ends_with(".mp4"),
            "unsupported stream format (expected .m3u8 or .mp4)"
        );
        anyhow::ensure!(
            source.quality == "best",
            "resolution selection requires an HLS master playlist"
        );
        selected = url.to_string();
        http.fetch(
            &url,
            &headers,
            Some(&stage.path().join("video.mp4")),
            u64::MAX,
        )
        .await?;
        reject_encrypted_mp4(&stage.path().join("video.mp4")).await?;
    }
    names = vec![if source.playlist_only {
        "playlist.m3u8"
    } else {
        "video.mp4"
    }
    .into()];
    names.extend(subtitles::download(http, &resolved.tracks, &headers, stage.path()).await?);
    let mut artifacts = Vec::new();
    for name in names {
        let bytes = tokio::fs::metadata(stage.path().join(&name)).await?.len();
        anyhow::ensure!(bytes > 0, "empty output file");
        artifacts.push(Artifact { name, bytes });
    }
    let manifest = Manifest {
        complete: true,
        playlist_only: source.playlist_only,
        anime_id: source.anime_id.clone(),
        episode: number,
        language: source.language.to_string(),
        provider: source.provider.clone(),
        quality: source.quality.clone(),
        selected_url: selected,
        resolved,
        artifacts,
    };
    tokio::fs::write(
        stage.path().join("metadata.json"),
        serde_json::to_vec_pretty(&manifest)?,
    )
    .await?;
    // Same filesystem directory rename publishes video, subtitles and completion marker together.
    tokio::fs::rename(stage.path(), &target)
        .await
        .context("publishing completed episode")?;
    progress.finish_with_message(format!("episode {number}: complete"));
    Ok(())
}
async fn reject_encrypted_mp4(path: &Path) -> Result<()> {
    use tokio::io::AsyncReadExt;
    let mut file = tokio::fs::File::open(path).await?;
    let mut buf = vec![0; 65536];
    let mut tail = Vec::new();
    let mut first = true;
    loop {
        let n = file.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        let mut chunk = tail;
        chunk.extend_from_slice(&buf[..n]);
        if first {
            anyhow::ensure!(
                chunk.len() >= 8 && &chunk[4..8] == b"ftyp",
                "direct response is not an MP4"
            );
            first = false;
        }
        anyhow::ensure!(
            !chunk
                .windows(4)
                .any(|w| [b"pssh", b"sinf", b"tenc", b"encv", b"enca"]
                    .contains(&w.try_into().unwrap())),
            "possible encrypted MP4 detected; refusing media"
        );
        tail = chunk[chunk.len().saturating_sub(3)..].to_vec();
    }
    Ok(())
}
