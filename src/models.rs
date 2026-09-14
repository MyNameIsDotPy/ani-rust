use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Source {
    pub url: String,
    #[serde(default)]
    pub drm: Option<serde_json::Value>,
}
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Track {
    #[serde(alias = "url")]
    pub file: String,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub kind: String,
}
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Resolved {
    pub sources: Vec<Source>,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    #[serde(default)]
    pub tracks: Vec<Track>,
    #[serde(default)]
    pub drm: Option<serde_json::Value>,
}
pub fn protected(value: &Option<serde_json::Value>) -> bool {
    value.as_ref().is_some_and(|v| !v.is_null() && v != false)
}
