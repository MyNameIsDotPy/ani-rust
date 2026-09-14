use crate::{api::Http, filenames::sanitize, models::Track};
use anyhow::Result;
use reqwest::{header::HeaderMap, Url};
use std::path::Path;
pub async fn download(
    http: &Http,
    tracks: &[Track],
    headers: &HeaderMap,
    dir: &Path,
) -> Result<Vec<String>> {
    let mut files = Vec::new();
    for (i, track) in tracks.iter().enumerate() {
        if !track.kind.is_empty() && track.kind != "subtitles" && track.kind != "captions" {
            continue;
        }
        let url = Url::parse(&track.file)?;
        let ext = url
            .path()
            .rsplit('.')
            .next()
            .unwrap_or("")
            .to_ascii_lowercase();
        anyhow::ensure!(
            ["vtt", "srt", "ass", "ssa"].contains(&ext.as_str()),
            "unsupported subtitle format for track {}",
            i + 1
        );
        let name = format!("subtitle-{}-{}.{}", i + 1, sanitize(&track.label), ext);
        http.fetch(&url, headers, Some(&dir.join(&name)), 32 * 1024 * 1024)
            .await?;
        files.push(name);
    }
    Ok(files)
}
