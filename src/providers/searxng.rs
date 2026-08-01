use anyhow::Result;
use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use url::Url;

use crate::{Image, Provider, SearchProvider, SearchResponse, SearchResult};

pub const SEARXNG_BASE_URL: &str = "http://localhost:8888/";

#[derive(Deserialize)]
struct SearXNGResponse {
    pub query: String,
    pub results: Vec<SearXNGResult>,
    pub unresponsive_engines: Vec<Vec<String>>,
}

#[derive(Debug, Deserialize)]
struct SearXNGResult {
    pub url: Url,
    pub title: String,
    pub content: String,
    pub engine: String,
    pub thumbnail: Option<String>,
}

#[derive(Debug, Serialize, Clone, PartialEq)]
pub struct SearXNGProvider {
    pub base_url: Url,
}

#[async_trait]
impl SearchProvider for SearXNGProvider {
    async fn search(&self, client: &Client, query: &str) -> Result<SearchResponse> {
        let response: SearXNGResponse = client
            .get(self.base_url.join("search")?)
            .query(&[("q", query), ("format", "json")])
            .send()
            .await?
            .json()
            .await?;

        let answer = None;
        let results: Vec<SearchResult> = response
            .results
            .into_iter()
            .map(|r| {
                let image = r
                    .thumbnail
                    .and_then(|t| Url::parse(&t).ok())
                    .map(|url| Image {
                        src: url,
                        alt: String::from("Image"),
                    });

                SearchResult {
                    title: r.title,
                    url: r.url,
                    image,
                    snippet: r.content,
                    source: r.engine,
                }
            })
            .collect();

        Ok(SearchResponse {
            query: query.to_string(),
            answer,
            results,
            provider: Provider::SearXNG,
        })
    }
}
