use crate::api::Http;
use anyhow::{bail, Context, Result};
use reqwest::{header::HeaderMap, Url};
use serde::Serialize;
use std::{collections::BTreeMap, path::Path};

#[derive(Debug, Clone, Serialize)]
pub struct Variant {
    pub url: UrlString,
    pub height: u32,
    pub bandwidth: u64,
}
type UrlString = String;
pub struct Playlist {
    pub url: Url,
    pub text: String,
    pub variants: Vec<Variant>,
}
pub fn attributes(s: &str) -> Result<BTreeMap<String, String>> {
    let mut quoted = false;
    let mut start = 0;
    let mut fields = Vec::new();
    for (i, c) in s.char_indices() {
        if c == '"' {
            quoted = !quoted;
        }
        if c == ',' && !quoted {
            fields.push(&s[start..i]);
            start = i + 1;
        }
    }
    anyhow::ensure!(!quoted, "unterminated playlist attribute");
    fields.push(&s[start..]);
    fields
        .into_iter()
        .map(|f| {
            let (k, v) = f.split_once('=').context("invalid playlist attribute")?;
            Ok((k.trim().to_owned(), v.trim().trim_matches('"').to_owned()))
        })
        .collect()
}
pub fn validate(text: &str) -> Result<()> {
    anyhow::ensure!(
        text.trim_start().starts_with("#EXTM3U"),
        "not an HLS playlist"
    );
    for line in text.lines().map(str::trim) {
        if line.starts_with("#EXT-X-KEY:") || line.starts_with("#EXT-X-SESSION-KEY:") {
            let a = attributes(line.split_once(':').unwrap().1)?;
            anyhow::ensure!(
                a.get("METHOD").map(String::as_str) == Some("NONE")
                    && !a.contains_key("URI")
                    && !a.contains_key("KEYFORMAT"),
                "encrypted/DRM-protected HLS is unsupported"
            );
        }
        anyhow::ensure!(
            !line.starts_with("#EXT-X-CONTENT-PROTECTION"),
            "DRM-protected HLS is unsupported"
        );
    }
    Ok(())
}
pub fn variants(text: &str, base: &Url) -> Result<Vec<Variant>> {
    validate(text)?;
    let mut result = Vec::new();
    let mut pending = None;
    for line in text.lines().map(str::trim).filter(|l| !l.is_empty()) {
        if let Some(attrs) = line.strip_prefix("#EXT-X-STREAM-INF:") {
            anyhow::ensure!(pending.is_none(), "missing variant URI");
            pending = Some(attributes(attrs)?);
        } else if !line.starts_with('#') {
            if let Some(attrs) = pending.take() {
                result.push(Variant {
                    url: base.join(line)?.to_string(),
                    height: attrs
                        .get("RESOLUTION")
                        .and_then(|r| r.split_once('x'))
                        .and_then(|(_, h)| h.parse().ok())
                        .unwrap_or(0),
                    bandwidth: attrs
                        .get("BANDWIDTH")
                        .and_then(|v| v.parse().ok())
                        .unwrap_or(0),
                });
            }
        }
    }
    anyhow::ensure!(pending.is_none(), "missing variant URI");
    Ok(result)
}
pub fn select<'a>(variants: &'a [Variant], quality: &str) -> Result<&'a Variant> {
    let height = if quality == "best" {
        None
    } else {
        Some(quality.trim_end_matches('p').parse::<u32>()?)
    };
    variants
        .iter()
        .filter(|v| height.is_none_or(|h| v.height == h))
        .max_by_key(|v| (v.height, v.bandwidth))
        .context("requested resolution is unavailable")
}
pub async fn load(http: &Http, url: &Url, headers: &HeaderMap) -> Result<Playlist> {
    let (url, text) = http.text(url, headers).await?;
    let variants = variants(&text, &url)?;
    Ok(Playlist {
        url,
        text,
        variants,
    })
}
pub async fn media(
    http: &Http,
    mut p: Playlist,
    headers: &HeaderMap,
    quality: &str,
) -> Result<Playlist> {
    anyhow::ensure!(
        quality == "best" || !p.variants.is_empty(),
        "requested resolution cannot be verified without a master playlist"
    );
    for _ in 0..5 {
        if p.variants.is_empty() {
            return Ok(p);
        }
        for line in p.text.lines().map(str::trim) {
            if let Some(attrs) = line.strip_prefix("#EXT-X-MEDIA:") {
                let a = attributes(attrs)?;
                anyhow::ensure!(
                    !(a.get("TYPE").map(String::as_str) == Some("AUDIO") && a.contains_key("URI")),
                    "separate HLS audio renditions are not yet supported"
                );
            }
        }
        let url = Url::parse(&select(&p.variants, quality)?.url)?;
        p = load(http, &url, headers).await?;
    }
    bail!("too many nested master playlists")
}
/// Export a VOD playlist without requesting any referenced media.
/// Relative references must be absolute when the playlist is saved to disk.
pub fn export(p: &Playlist) -> Result<String> {
    validate(&p.text)?;
    anyhow::ensure!(
        p.text
            .lines()
            .any(|l| !l.trim().is_empty() && !l.trim().starts_with('#')),
        "playlist has no media references"
    );
    anyhow::ensure!(
        p.variants.is_empty(),
        "export requires a selected media playlist"
    );
    anyhow::ensure!(
        p.text.lines().any(|l| l.trim() == "#EXT-X-ENDLIST"),
        "live/incomplete HLS is unsupported"
    );
    anyhow::ensure!(
        !p.text.contains("{$"),
        "HLS variable substitution is unsupported"
    );
    let absolute = |value: &str| -> Result<String> {
        let url = p.url.join(value)?;
        anyhow::ensure!(
            matches!(url.scheme(), "http" | "https"),
            "playlist references must use HTTP(S)"
        );
        Ok(url.to_string())
    };
    let mut output = String::new();
    for line in p.text.lines().map(str::trim) {
        if !line.is_empty() && !line.starts_with('#') {
            output.push_str(&absolute(line)?);
        } else if line.starts_with("#EXT") {
            let mut remaining = line;
            while let Some(start) = remaining.find("URI=\"") {
                let value_start = start + 5;
                let end = remaining[value_start..]
                    .find('"')
                    .context("unterminated playlist URI")?
                    + value_start;
                output.push_str(&remaining[..value_start]);
                output.push_str(&absolute(&remaining[value_start..end])?);
                output.push('"');
                remaining = &remaining[end + 1..];
            }
            output.push_str(remaining);
        } else {
            output.push_str(line);
        }
        output.push('\n');
    }
    Ok(output)
}
// Build a new, restricted local playlist. No remote URI or arbitrary tag reaches ffmpeg.
pub async fn stage(
    http: &Http,
    p: &Playlist,
    headers: &HeaderMap,
    dir: &Path,
    progress: &indicatif::ProgressBar,
) -> Result<()> {
    validate(&p.text)?;
    anyhow::ensure!(
        p.text.lines().any(|l| l.trim() == "#EXT-X-ENDLIST"),
        "live/incomplete HLS is unsupported"
    );
    let mut files = Vec::new();
    let mut local = String::from("#EXTM3U\n");
    for line in p.text.lines().map(str::trim).filter(|l| !l.is_empty()) {
        if line == "#EXTM3U" || line.starts_with("#EXT-X-KEY:") {
            continue;
        }
        if let Some(attrs) = line.strip_prefix("#EXT-X-MAP:") {
            let a = attributes(attrs)?;
            anyhow::ensure!(
                !a.contains_key("BYTERANGE"),
                "HLS byte ranges are not yet supported"
            );
            let url = p.url.join(a.get("URI").context("map missing URI")?)?;
            let name = format!("init-{}.mp4", files.len());
            local.push_str(&format!("#EXT-X-MAP:URI=\"{name}\"\n"));
            files.push((url, name));
        } else if !line.starts_with('#') {
            let name = format!("segment-{}.ts", files.len());
            files.push((p.url.join(line)?, name.clone()));
            local.push_str(&format!("{name}\n"));
        } else if let Some(value) = line.strip_prefix("#EXTINF:") {
            let duration: f64 = value.split(',').next().unwrap().parse()?;
            anyhow::ensure!(
                duration.is_finite() && duration > 0.0,
                "invalid segment duration"
            );
            local.push_str(&format!("#EXTINF:{duration},\n"));
        } else if [
            "#EXT-X-VERSION:",
            "#EXT-X-TARGETDURATION:",
            "#EXT-X-MEDIA-SEQUENCE:",
            "#EXT-X-DISCONTINUITY-SEQUENCE:",
        ]
        .iter()
        .any(|prefix| line.starts_with(prefix))
        {
            let (tag, value) = line.split_once(':').unwrap();
            let value: u64 = value.parse()?;
            local.push_str(&format!("{tag}:{value}\n"));
        } else if matches!(
            line,
            "#EXT-X-ENDLIST"
                | "#EXT-X-DISCONTINUITY"
                | "#EXT-X-INDEPENDENT-SEGMENTS"
                | "#EXT-X-PLAYLIST-TYPE:VOD"
        ) {
            local.push_str(line);
            local.push('\n');
        } else if line.starts_with("#EXT-X-PROGRAM-DATE-TIME:") || !line.starts_with("#EXT") { /* descriptive only */
        } else {
            bail!("unsupported HLS tag: {}", line.split(':').next().unwrap());
        }
    }
    anyhow::ensure!(!files.is_empty(), "playlist has no media");
    progress.set_length(files.len() as u64);
    for (url, name) in files {
        http.fetch(&url, headers, Some(&dir.join(name)), 2 * 1024 * 1024 * 1024)
            .await?;
        progress.inc(1);
    }
    tokio::fs::write(dir.join("local.m3u8"), local).await?;
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn export_preserves_ranges_and_absolutizes_references() {
        let p = Playlist { url:Url::parse("https://example.org/a/media.m3u8?token=playlist").unwrap(), variants:vec![], text:"#EXTM3U\n#EXT-X-MAP:URI=\"../init.mp4?token=init\",BYTERANGE=\"100@0\"\n#EXTINF:1,\n#EXT-X-BYTERANGE:100@100\nsegment.m4s?token=media\n#EXT-X-ENDLIST\n".into() };
        let result = export(&p).unwrap();
        assert!(
            result.contains("URI=\"https://example.org/init.mp4?token=init\",BYTERANGE=\"100@0\"")
        );
        assert!(result.contains("https://example.org/a/segment.m4s?token=media"));
        assert!(result.contains("#EXT-X-BYTERANGE:100@100"));
        let mut encrypted = p;
        encrypted
            .text
            .push_str("#EXT-X-KEY:METHOD=AES-128,URI=\"key\"\n");
        assert!(export(&encrypted).is_err());
    }
    #[test]
    fn quoted_and_quality() {
        assert_eq!(
            attributes("CODECS=\"a,b\",BANDWIDTH=20").unwrap()["CODECS"],
            "a,b"
        );
        let v = variants("#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=100,RESOLUTION=1280x720\n720.m3u8\n#EXT-X-STREAM-INF:BANDWIDTH=200,RESOLUTION=1920x1080\n../1080.m3u8", &Url::parse("https://example.org/a/master.m3u8").unwrap()).unwrap();
        assert_eq!(select(&v, "best").unwrap().height, 1080);
        assert_eq!(select(&v, "720p").unwrap().height, 720);
        assert!(select(&v, "480").is_err());
        assert_eq!(v[1].url, "https://example.org/1080.m3u8");
    }
    #[test]
    fn protection() {
        for method in ["AES-128", "SAMPLE-AES", "SAMPLE-AES-CTR"] {
            assert!(validate(&format!("#EXTM3U\n#EXT-X-KEY:METHOD={method},URI=\"key\"")).is_err());
        }
        assert!(validate("#EXTM3U\n#EXT-X-SESSION-KEY:METHOD=AES-128").is_err());
        assert!(validate("#EXTM3U\n#EXT-X-KEY:METHOD=NONE").is_ok());
    }
}
