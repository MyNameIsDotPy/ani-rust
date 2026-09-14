//! Catalog operations are independent of source resolution and media transfers.
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::time::Duration;

const FAST_SEARCH: &str = r#"
query FastSearch($query: String, $limit: Int, $includeAdult: Boolean) {
  catalogAnime(filter: { query: $query includeAdult: $includeAdult }, limit: $limit) {
    items {
      id anilistId malId titleRomaji titleEnglish coverImage format status
      episodeCount seasonYear season color genres bannerImage
    }
  }
}"#;

#[derive(Debug, Deserialize)]
pub struct GraphQlResponse<T> {
    pub data: Option<T>,
    #[serde(default)]
    pub errors: Vec<GraphQlError>,
}
#[derive(Debug, Deserialize, Serialize)]
pub struct GraphQlError {
    pub message: String,
}
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchData {
    pub catalog_anime: Option<CatalogAnime>,
}
#[derive(Debug, Deserialize)]
pub struct CatalogAnime {
    pub items: Vec<AnimeSearchResult>,
}
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AnimeSearchResult {
    /// Internal AnimeX identifier; AniList/MAL IDs are informational only.
    pub id: String,
    pub anilist_id: Option<i64>,
    pub mal_id: Option<i64>,
    pub title_romaji: Option<String>,
    pub title_english: Option<String>,
    // AnimeX image scalars can contain structured size/URL objects.
    pub cover_image: Option<serde_json::Value>,
    pub banner_image: Option<serde_json::Value>,
    pub color: Option<String>,
    pub format: Option<String>,
    pub status: Option<String>,
    pub episode_count: Option<i32>,
    pub season_year: Option<i32>,
    pub season: Option<String>,
    pub genres: Option<Vec<String>>,
}
pub struct AnimeXClient {
    http: reqwest::Client,
    graphql_url: reqwest::Url,
    retries: u32,
}
impl AnimeXClient {
    pub fn new(graphql_url: &str, retries: u32) -> Result<Self> {
        let graphql_url = reqwest::Url::parse(graphql_url)?;
        anyhow::ensure!(
            matches!(graphql_url.scheme(), "http" | "https"),
            "GraphQL endpoint must use HTTP(S)"
        );
        Ok(Self {
            http: reqwest::Client::builder()
                .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36")
                .connect_timeout(Duration::from_secs(20))
                .timeout(Duration::from_secs(60))
                .build()?,
            graphql_url,
            retries,
        })
    }
    /// Reusable transport for observed read-only GraphQL operations.
    async fn query<T: serde::de::DeserializeOwned>(
        &self,
        operation: &str,
        query: &str,
        variables: serde_json::Value,
    ) -> Result<T> {
        tracing::debug!(operation,variables=%variables,"GraphQL request");
        let request =
            serde_json::json!({"operationName":operation,"query":query,"variables":variables});
        for attempt in 0..=self.retries {
            let response = self
                .http
                .post(self.graphql_url.clone())
                .json(&request)
                .send()
                .await;
            let result: Result<GraphQlResponse<T>> = async {
                let response = response?;
                tracing::debug!(operation,status=%response.status(),"GraphQL HTTP response");
                // Read error bodies too: GraphQL servers may use non-2xx for validation errors.
                let status = response.status();
                let status_error = response.error_for_status_ref().err();
                let body = response.bytes().await?;
                let parsed = serde_json::from_slice::<GraphQlResponse<T>>(&body);
                if let Ok(envelope) = &parsed {
                    for error in &envelope.errors {
                        tracing::debug!(operation,error=%clean(&error.message),"GraphQL error");
                    }
                }
                if let Some(e) = status_error {
                    return Err(e.into());
                }
                parsed.with_context(|| format!("invalid GraphQL JSON (HTTP {status})"))
            }
            .await;
            match result {
                Ok(envelope) => {
                    anyhow::ensure!(
                        envelope.errors.is_empty(),
                        "GraphQL returned errors: {}",
                        envelope
                            .errors
                            .iter()
                            .map(|e| clean(&e.message))
                            .collect::<Vec<_>>()
                            .join("; ")
                    );
                    return envelope.data.context("GraphQL response has no data");
                }
                Err(e) => {
                    let retry = e.downcast_ref::<reqwest::Error>().is_some_and(|e| {
                        e.is_timeout()
                            || e.is_connect()
                            || e.is_body()
                            || e.status()
                                .is_some_and(|s| s.is_server_error() || s.as_u16() == 429)
                    });
                    if !retry || attempt == self.retries {
                        return Err(e).context("GraphQL request failed");
                    }
                    tracing::warn!(operation, attempt = attempt + 1, "retrying GraphQL request");
                    tokio::time::sleep(Duration::from_millis(500 * (1 << attempt))).await;
                }
            }
        }
        anyhow::bail!("GraphQL retries exhausted")
    }
    pub async fn search_anime(
        &self,
        query: &str,
        limit: usize,
        include_adult: bool,
    ) -> Result<Vec<AnimeSearchResult>> {
        anyhow::ensure!(!query.trim().is_empty(), "search query must not be empty");
        anyhow::ensure!(
            (1..=100).contains(&limit),
            "limit must be between 1 and 100"
        );
        let data: SearchData = self
            .query(
                "FastSearch",
                FAST_SEARCH,
                serde_json::json!({"query":query,"limit":limit,"includeAdult":include_adult}),
            )
            .await?;
        let items = data
            .catalog_anime
            .context("GraphQL catalogAnime is null")?
            .items;
        tracing::debug!(
            operation = "FastSearch",
            results = items.len(),
            "catalog search complete"
        );
        Ok(items)
    }
}
fn clean(s: &str) -> String {
    s.chars().filter(|c| !c.is_control()).collect()
}
pub fn display(results: &[AnimeSearchResult]) {
    if results.is_empty() {
        println!("No anime found.");
        return;
    }
    for (i, r) in results.iter().enumerate() {
        let title = r
            .title_english
            .as_deref()
            .filter(|s| !s.is_empty())
            .or(r.title_romaji.as_deref())
            .unwrap_or(&r.id);
        println!("[{}] {}\n    ID: {}", i + 1, clean(title), clean(&r.id));
        for (label, value) in [
            ("AniList", r.anilist_id.map(|v| v.to_string())),
            ("MAL", r.mal_id.map(|v| v.to_string())),
            ("Episodes", r.episode_count.map(|v| v.to_string())),
            ("Year", r.season_year.map(|v| v.to_string())),
            ("Season", r.season.clone()),
            ("Format", r.format.clone()),
            ("Status", r.status.clone()),
            ("Genres", r.genres.as_ref().map(|v| v.join(", "))),
        ] {
            if let Some(value) = value {
                println!("    {label}: {}", clean(&value));
            }
        }
        println!();
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn nullable_metadata_and_internal_id() {
        let response: GraphQlResponse<SearchData> = serde_json::from_str(r#"{"data":{"catalogAnime":{"items":[{"id":"actual-internal-id","anilistId":20,"titleRomaji":"Example"}]}}}"#).unwrap();
        let item = response
            .data
            .unwrap()
            .catalog_anime
            .unwrap()
            .items
            .pop()
            .unwrap();
        assert_eq!(item.id, "actual-internal-id");
        assert_eq!(item.anilist_id, Some(20));
        assert_eq!(item.episode_count, None);
    }
    #[test]
    fn errors_without_data() {
        let response: GraphQlResponse<SearchData> =
            serde_json::from_str(r#"{"errors":[{"message":"bad query"}]}"#).unwrap();
        assert!(response.data.is_none());
        assert_eq!(response.errors[0].message, "bad query");
    }
    #[test]
    fn image_scalars_accept_objects_and_urls() {
        let item: AnimeSearchResult = serde_json::from_str(r#"{"id":"fixture","coverImage":{"large":"https://example.org/cover.jpg"},"bannerImage":"https://example.org/banner.jpg"}"#).unwrap();
        assert_eq!(
            item.cover_image.unwrap()["large"],
            "https://example.org/cover.jpg"
        );
        assert!(item.banner_image.unwrap().is_string());
    }
}
