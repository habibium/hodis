pub mod config;
pub mod engine;
pub mod keys;
pub mod mcp;
pub mod output;
pub mod providers;

use std::num::NonZeroU32;

use anyhow::Result;
use async_trait::async_trait;
use clap::ValueEnum;
use reqwest::Client;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use url::Url;

#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq, ValueEnum, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Provider {
    /// Not searchable yet, so neither a flag nor a config file can choose it.
    #[serde(skip_deserializing)]
    #[value(skip)]
    DuckDuckGo,
    /// A SearXNG instance: yours, or one hodis runs itself
    #[value(name = "searxng")]
    SearXNG,
    /// Google results through serper.dev, with an API key
    Serper,
}

// Field docs double as descriptions in the `json` format's schema, so each
// stays on one line: a line break would land in the description.

#[derive(Debug, Serialize, Clone, PartialEq, JsonSchema)]
pub struct SearchResponse {
    pub query: String,
    /// A direct answer, such as a fact or a definition, when there is one.
    pub answer: Option<Answer>,
    /// Best first.
    pub results: Vec<SearchResult>,
    pub provider: Provider,
    /// What went wrong without stopping the search.
    pub diagnostics: Diagnostics,
}

#[derive(Debug, Serialize, Clone, PartialEq, Default, JsonSchema)]
pub struct Diagnostics {
    /// Engines that did not answer, so their results are missing.
    pub unresponsive_engines: Vec<EngineIssue>,
    /// Results dropped for lacking a title or a valid URL.
    #[schemars(transform = drop_format)]
    pub skipped_results: usize,
}

#[derive(Debug, Serialize, Clone, PartialEq, JsonSchema)]
pub struct EngineIssue {
    pub engine: String,
    pub reason: String,
}

#[derive(Debug, Serialize, Clone, PartialEq, JsonSchema)]
pub struct SearchResult {
    pub title: String,
    pub url: Url,
    /// Text from the page, possibly empty.
    pub snippet: String,
    /// A thumbnail, when the provider has one.
    pub image: Option<Url>,
    /// The engine that found the result, such as `google`, possibly empty.
    pub source: String,
}

#[derive(Debug, Serialize, Clone, PartialEq, JsonSchema)]
pub struct Answer {
    /// The page the answer comes from, when known.
    pub url: Option<Url>,
    pub content: String,
}

/// For `#[schemars(transform)]` on an integer: JSON Schema defines no formats
/// like schemars' `uint`, and validators warn about formats they do not know.
pub(crate) fn drop_format(schema: &mut schemars::Schema) {
    schema.remove("format");
}

#[derive(Debug)]
pub struct SearchParams {
    pub query: String,
    pub page: Option<NonZeroU32>,
}

#[async_trait]
pub trait SearchProvider: Send + Sync {
    async fn search(&self, client: &Client, params: &SearchParams) -> Result<SearchResponse>;
}
