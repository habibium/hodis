pub mod cache;
pub mod config;
pub mod engine;
pub mod keys;
pub mod mcp;
pub mod output;
pub mod progress;
pub mod providers;
pub mod setup;
pub mod tui;

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
    /// DuckDuckGo's web results, read from its results page
    #[value(
        name = "duckduckgo",
        help = "Works right away, but heavy use gets rate limited"
    )]
    DuckDuckGo,
    /// A SearXNG instance: yours, or one hodis runs itself
    #[value(
        name = "searxng",
        help = "The most reliable, but needs a server hodis can install"
    )]
    SearXNG,
    /// Google results through serper.dev, with an API key
    #[value(help = "Google results, but needs a paid serper.dev API key")]
    Serper,
}

/// Which providers a search goes to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Providers {
    /// SearXNG when an instance answers at its configured address, and
    /// otherwise DuckDuckGo, so that searching needs nothing installed.
    Auto,
    /// These, all at once, as a config file or `--provider` names them.
    Listed(Vec<Provider>),
}

impl Provider {
    /// The name messages use.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::DuckDuckGo => "DuckDuckGo",
            Self::SearXNG => "SearXNG",
            Self::Serper => "Serper",
        }
    }

    /// The name config files and `--provider` use.
    #[must_use]
    pub fn id(self) -> &'static str {
        match self {
            Self::DuckDuckGo => "duckduckgo",
            Self::SearXNG => "searxng",
            Self::Serper => "serper",
        }
    }

    /// Its trade-off in a few words, as `--help` lists it.
    #[must_use]
    pub fn summary(self) -> String {
        self.to_possible_value()
            .and_then(|value| value.get_help().map(ToString::to_string))
            .unwrap_or_default()
    }
}

// Field docs double as descriptions in the `json` format's schema, so each
// stays on one line: a line break would land in the description.

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, JsonSchema)]
pub struct SearchResponse {
    pub query: String,
    /// A direct answer, such as a fact or a definition, when there is one.
    pub answer: Option<Answer>,
    /// Best first.
    pub results: Vec<SearchResult>,
    /// The providers that answered, in the order they were asked.
    pub providers: Vec<Provider>,
    /// What went wrong without stopping the search.
    pub diagnostics: Diagnostics,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Default, JsonSchema)]
pub struct Diagnostics {
    /// Providers that failed or ran out of time, so their results are missing.
    pub failed_providers: Vec<ProviderIssue>,
    /// Engines that did not answer, so their results are missing.
    pub unresponsive_engines: Vec<EngineIssue>,
    /// Results dropped for lacking a title or a valid URL.
    #[schemars(transform = drop_format)]
    pub skipped_results: usize,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, JsonSchema)]
pub struct ProviderIssue {
    pub provider: Provider,
    pub reason: String,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, JsonSchema)]
pub struct EngineIssue {
    pub engine: String,
    pub reason: String,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, JsonSchema)]
pub struct SearchResult {
    pub title: String,
    pub url: Url,
    /// Text from the page, possibly empty.
    pub snippet: String,
    /// A thumbnail, when the provider has one.
    pub image: Option<Url>,
    /// The engine that found the result, such as `google`, possibly empty.
    pub source: String,
    pub provider: Provider,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, JsonSchema)]
pub struct Answer {
    /// The page the answer comes from, when known.
    pub url: Option<Url>,
    pub content: String,
    pub provider: Provider,
}

/// For `#[schemars(transform)]` on an integer: JSON Schema defines no formats
/// like schemars' `uint`, and validators warn about formats they do not know.
pub(crate) fn drop_format(schema: &mut schemars::Schema) {
    schema.remove("format");
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchParams {
    pub query: String,
    pub page: Option<NonZeroU32>,
}

#[async_trait]
#[allow(
    clippy::double_must_use,
    reason = "async_trait marks its boxed futures must_use, as they already are"
)]
pub trait SearchProvider: Send + Sync {
    async fn search(&self, client: &Client, params: &SearchParams) -> Result<SearchResponse>;
}
