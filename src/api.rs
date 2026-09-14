use crate::{cli::SourceArgs, models::Resolved};
use anyhow::{bail, Context, Result};
use futures_util::StreamExt;
use reqwest::{
    header::{HeaderMap, HeaderName, HeaderValue},
    Client, Url,
};
use std::{
    collections::BTreeMap,
    path::Path,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::{io::AsyncWriteExt, sync::Mutex as AsyncMutex};

/// Caps outgoing request rate so bulk downloads don't look like abuse to the source API/CDN.
struct RateLimiter {
    interval: Duration,
    next: AsyncMutex<Instant>,
}
impl RateLimiter {
    fn new(per_second: u32) -> Self {
        Self {
            interval: Duration::from_secs_f64(1.0 / per_second.max(1) as f64),
            next: AsyncMutex::new(Instant::now()),
        }
    }
    async fn acquire(&self) {
        let mut next = self.next.lock().await;
        let now = Instant::now();
        let start = (*next).max(now);
        if start > now {
            tokio::time::sleep(start - now).await;
        }
        *next = start + self.interval;
    }
}

#[derive(Clone)]
pub struct Http {
    client: Client,
    retries: u32,
    limiter: Arc<RateLimiter>,
}
impl Http {
    pub fn new(retries: u32, requests_per_second: u32) -> Result<Self> {
        Ok(Self {
            client: Client::builder()
                .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36")
                .connect_timeout(Duration::from_secs(20))
                .read_timeout(Duration::from_secs(60))
                .build()?,
            retries,
            limiter: Arc::new(RateLimiter::new(requests_per_second)),
        })
    }
    pub async fn fetch(
        &self,
        url: &Url,
        headers: &HeaderMap,
        destination: Option<&Path>,
        max: u64,
    ) -> Result<(Url, Vec<u8>)> {
        anyhow::ensure!(
            matches!(url.scheme(), "http" | "https"),
            "only HTTP(S) URLs are supported"
        );
        for attempt in 0..=self.retries {
            let result = async {
                self.limiter.acquire().await;
                let response = self
                    .client
                    .get(url.clone())
                    .headers(headers.clone())
                    .send()
                    .await?;
                let status = response.status();
                if !status.is_success() {
                    if let Some(error) = response.error_for_status_ref().err() {
                        return Err(error.into());
                    }
                    bail!("unexpected HTTP status: {status}");
                }
                let final_url = response.url().clone();
                let mut body = response.bytes_stream();
                let mut bytes = Vec::new();
                let mut file = match destination {
                    Some(p) => Some(tokio::fs::File::create(p).await?),
                    None => None,
                };
                let mut count = 0u64;
                while let Some(chunk) = body.next().await {
                    let chunk = chunk?;
                    count += chunk.len() as u64;
                    anyhow::ensure!(count <= max, "response exceeds size limit");
                    if let Some(f) = &mut file {
                        f.write_all(&chunk).await?;
                    } else {
                        bytes.extend_from_slice(&chunk);
                    }
                }
                if let Some(f) = &mut file {
                    f.sync_all().await?;
                }
                anyhow::ensure!(count > 0, "empty response");
                Ok((final_url, bytes))
            }
            .await;
            match result {
                Ok(v) => return Ok(v),
                Err(e) => {
                    let retry = e.downcast_ref::<reqwest::Error>().is_some_and(|e| {
                        e.is_timeout()
                            || e.is_connect()
                            || e.is_body()
                            || e.status()
                                .is_some_and(|s| s.is_server_error() || s.as_u16() == 429)
                    });
                    if !retry || attempt == self.retries {
                        return Err(e).context("HTTP transfer failed");
                    }
                    tracing::warn!(attempt = attempt + 1, "transient HTTP failure; retrying");
                    tokio::time::sleep(Duration::from_millis(500 * (1 << attempt))).await;
                }
            }
        }
        bail!("retry loop exhausted")
    }
    pub async fn text(&self, url: &Url, headers: &HeaderMap) -> Result<(Url, String)> {
        let (url, bytes) = self.fetch(url, headers, None, 8 * 1024 * 1024).await?;
        Ok((url, String::from_utf8(bytes)?))
    }
}
pub fn headers(values: &BTreeMap<String, String>) -> Result<HeaderMap> {
    let mut result = HeaderMap::new();
    for (k, v) in values {
        result.insert(
            HeaderName::from_bytes(k.as_bytes()).context("invalid source header name")?,
            HeaderValue::from_str(v).context("invalid source header value")?,
        );
    }
    Ok(result)
}
#[async_trait::async_trait]
pub trait SourceResolver: Send + Sync {
    async fn resolve(&self, source: &SourceArgs, episode: u32) -> Result<Resolved>;
}
pub struct ApiResolver {
    pub http: Http,
    pub endpoint: Url,
}
pub struct SavedResolver {
    resolved: Resolved,
    episode: u32,
}
impl SavedResolver {
    pub async fn load(path: &Path, episode: u32) -> Result<Self> {
        let body = tokio::fs::read(path)
            .await
            .context("could not read --source-json file")?;
        Self::from_json(&body, episode)
    }
    pub fn from_json(body: &[u8], episode: u32) -> Result<Self> {
        let resolved: Resolved = serde_json::from_slice(body)
            .context("invalid source JSON; save the raw API response, without Markdown links")?;
        anyhow::ensure!(
            !resolved.sources.is_empty(),
            "saved response contains no sources"
        );
        headers(&resolved.headers)?;
        for source in &resolved.sources {
            let url = Url::parse(&source.url).context("invalid saved stream URL")?;
            anyhow::ensure!(
                matches!(url.scheme(), "http" | "https"),
                "saved stream must use HTTP(S)"
            );
        }
        Ok(Self { resolved, episode })
    }
}
#[async_trait::async_trait]
impl SourceResolver for SavedResolver {
    async fn resolve(&self, _: &SourceArgs, episode: u32) -> Result<Resolved> {
        anyhow::ensure!(
            episode == self.episode,
            "saved response belongs to a different episode"
        );
        Ok(self.resolved.clone())
    }
}
#[async_trait::async_trait]
impl SourceResolver for ApiResolver {
    async fn resolve(&self, source: &SourceArgs, episode: u32) -> Result<Resolved> {
        let mut url = self.endpoint.clone();
        url.query_pairs_mut()
            .append_pair("id", &source.anime_id)
            .append_pair("epNum", &episode.to_string())
            .append_pair("type", &source.language.to_string())
            .append_pair("providerId", &source.provider);
        let (_, body) = self.http.text(&url, &HeaderMap::new()).await.map_err(|e| {
            if e.downcast_ref::<reqwest::Error>().and_then(|e| e.status()) == Some(reqwest::StatusCode::FORBIDDEN) {
                e.context("source API denied access (403). If you already have an authorized resolver response, save its raw JSON and use --source-json <file> for that episode")
            } else { e }
        })?;
        let resolved: Resolved = serde_json::from_str(&body).context("invalid resolver JSON")?;
        anyhow::ensure!(!resolved.sources.is_empty(), "resolver returned no sources");
        Ok(resolved)
    }
}
