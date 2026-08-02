pub mod providers;

use anyhow::Result;
use async_trait::async_trait;
use reqwest::Client;
use serde::Serialize;
use url::Url;

#[derive(Debug, Serialize, Clone, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Provider {
    DuckDuckGo,
    SearXNG,
}

#[derive(Debug, Serialize, Clone, PartialEq)]
pub struct SearchResponse {
    pub query: String,
    pub answer: Option<Answer>,
    pub results: Vec<SearchResult>,
    pub provider: Provider,
    pub diagnostics: Diagnostics,
}

#[derive(Debug, Serialize, Clone, PartialEq, Default)]
pub struct Diagnostics {
    pub unresponsive_engines: Vec<EngineIssue>,
    pub skipped_results: usize,
}

#[derive(Debug, Serialize, Clone, PartialEq)]
pub struct EngineIssue {
    pub engine: String,
    pub reason: String,
}

#[derive(Debug, Serialize, Clone, PartialEq)]
pub struct SearchResult {
    pub title: String,
    pub url: Url,
    pub snippet: String,
    pub image: Option<Url>,
    pub source: String,
}

#[derive(Debug, Serialize, Clone, PartialEq)]
pub struct Answer {
    pub question: Option<String>,
    pub url: Option<Url>,
    pub content: String,
}

#[async_trait]
pub trait SearchProvider {
    async fn search(&self, client: &Client, query: &str) -> Result<SearchResponse>;
}
