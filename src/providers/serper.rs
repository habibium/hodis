//! Google results through the serper.dev API, which needs an API key.

use std::num::NonZeroU32;

use anyhow::{Result, bail};
use async_trait::async_trait;
use reqwest::{Client, header::HeaderValue};
use serde::{Deserialize, Serialize};
use url::Url;

use crate::{
    Answer, Diagnostics, Provider, SearchParams, SearchProvider, SearchResponse, SearchResult,
    keys::ApiKey,
};

pub const SERPER_BASE_URL: &str = "https://google.serper.dev/";

/// Every result comes from Google, so it carries Google as its source, the
/// way a SearXNG result carries the engine that found it.
const SOURCE: &str = "google";

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SerperResponse {
    #[serde(default)]
    organic: Vec<SerperResult>,
    #[serde(default)]
    answer_box: Option<SerperAnswerBox>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SerperResult {
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    link: Option<String>,
    #[serde(default)]
    snippet: Option<String>,
    #[serde(default)]
    image_url: Option<String>,
}

/// Either a short `answer`, such as "Oslo", or a `snippet` quoted from the
/// page at `link`.
#[derive(Debug, Deserialize)]
struct SerperAnswerBox {
    #[serde(default)]
    answer: Option<String>,
    #[serde(default)]
    snippet: Option<String>,
    #[serde(default)]
    link: Option<String>,
}

/// The body of a refused request, such as one with a wrong key or no credits.
#[derive(Debug, Deserialize)]
struct SerperError {
    message: String,
}

#[derive(Debug, Serialize)]
struct SerperRequest<'a> {
    q: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    page: Option<NonZeroU32>,
}

#[derive(Debug, Clone)]
pub struct SerperProvider {
    pub base_url: Url,
    pub key: ApiKey,
}

impl SerperProvider {
    pub fn parse_body(&self, query: &str, body: &str) -> Result<SearchResponse> {
        let wire: SerperResponse = serde_json::from_str(body)?;

        let total = wire.organic.len();
        let results: Vec<SearchResult> = wire
            .organic
            .into_iter()
            .filter_map(|r| {
                let title = r.title.filter(|t| !t.is_empty())?;
                let url = r.link.as_deref().and_then(|url| Url::parse(url).ok())?;
                let image = r.image_url.as_deref().and_then(|url| Url::parse(url).ok());
                Some(SearchResult {
                    title,
                    url,
                    image,
                    snippet: r.snippet.unwrap_or_default(),
                    source: SOURCE.to_owned(),
                    provider: Provider::Serper,
                })
            })
            .collect();
        let skipped = total - results.len();

        let answer = wire.answer_box.and_then(|a| {
            let content = [a.answer, a.snippet]
                .into_iter()
                .flatten()
                .find(|text| !text.is_empty())?;
            Some(Answer {
                url: a.link.as_deref().and_then(|u| Url::parse(u).ok()),
                content,
                provider: Provider::Serper,
            })
        });

        Ok(SearchResponse {
            query: query.to_owned(),
            answer,
            results,
            providers: vec![Provider::Serper],
            diagnostics: Diagnostics {
                skipped_results: skipped,
                ..Diagnostics::default()
            },
        })
    }
}

#[async_trait]
impl SearchProvider for SerperProvider {
    async fn search(&self, client: &Client, params: &SearchParams) -> Result<SearchResponse> {
        let mut key = HeaderValue::from_str(self.key.expose())?;
        key.set_sensitive(true);

        let response = client
            .post(self.base_url.join("search")?)
            .header("X-API-KEY", key)
            .json(&SerperRequest {
                q: &params.query,
                page: params.page,
            })
            .send()
            .await?;
        let status = response.status();
        let body = response.text().await?;

        if !status.is_success() {
            let Ok(error) = serde_json::from_str::<SerperError>(&body) else {
                bail!("Serper refused the search ({status})");
            };
            // The message reaches the terminal, which would act on escape
            // sequences in it.
            let message: String = error.message.chars().filter(|c| !c.is_control()).collect();
            bail!("Serper refused the search ({status}): {message}");
        }

        self.parse_body(&params.query, &body)
    }
}
