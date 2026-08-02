use anyhow::Result;
use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use url::Url;

use crate::{
    Answer, Diagnostics, EngineIssue, Provider, SearchProvider, SearchResponse, SearchResult,
};

pub const SEARXNG_BASE_URL: &str = "http://localhost:8888/";

#[derive(Debug, Deserialize)]
struct SearXNGResponse {
    #[serde(default)]
    results: Vec<SearXNGResult>,
    #[serde(default)]
    answers: Vec<SearXNGAnswer>,
    #[serde(default)]
    unresponsive_engines: Vec<Vec<String>>,
}

#[derive(Debug, Deserialize)]
struct SearXNGResult {
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    engine: Option<String>,
    #[serde(default)]
    thumbnail: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SearXNGAnswer {
    #[serde(default)]
    answer: String,
    #[serde(default)]
    url: Option<String>,
}

#[derive(Debug, Serialize, Clone, PartialEq)]
pub struct SearXNGProvider {
    pub base_url: Url,
}

impl SearXNGProvider {
    pub fn parse_body(&self, query: &str, body: &str) -> Result<SearchResponse> {
        let wire: SearXNGResponse = serde_json::from_str(body)?;

        let mut results = Vec::with_capacity(wire.results.len());
        let mut skipped = 0;
        for r in wire.results {
            // a missing/malformed url or a blank title drops the result and moves on.
            let Some(Ok(url)) = r.url.as_deref().map(Url::parse) else {
                skipped += 1;
                continue;
            };
            let Some(title) = r.title.filter(|t| !t.is_empty()) else {
                skipped += 1;
                continue;
            };
            results.push(SearchResult {
                title,
                url,
                snippet: r.content.unwrap_or_default(),
                image: r.thumbnail.as_deref().and_then(|t| Url::parse(t).ok()),
                source: r.engine.unwrap_or_default(),
            });
        }

        let answer = wire
            .answers
            .into_iter()
            .find(|a| !a.answer.is_empty())
            .map(|a| Answer {
                question: None,
                url: a.url.as_deref().and_then(|u| Url::parse(u).ok()),
                content: a.answer,
            });

        let unresponsive_engines = wire
            .unresponsive_engines
            .into_iter()
            .filter_map(|entry| {
                // Observed as [engine, reason] pairs; tolerate anything else.
                let mut parts = entry.into_iter();
                let engine = parts.next()?;
                let reason = parts.next().unwrap_or_default();
                Some(EngineIssue { engine, reason })
            })
            .collect();

        Ok(SearchResponse {
            query: query.to_string(),
            answer,
            results,
            provider: Provider::SearXNG,
            diagnostics: Diagnostics {
                unresponsive_engines,
                skipped_results: skipped,
            },
        })
    }
}

#[async_trait]
impl SearchProvider for SearXNGProvider {
    async fn search(&self, client: &Client, query: &str) -> Result<SearchResponse> {
        let body = client
            .get(self.base_url.join("search")?)
            .query(&[("q", query), ("format", "json")])
            .send()
            .await?
            .error_for_status()?
            .text()
            .await?;

        self.parse_body(query, &body)
    }
}
