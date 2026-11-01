//! Web results from DuckDuckGo. It offers no search API, so hodis reads the
//! results page it serves to browsers without JavaScript, as SearXNG does.

use std::collections::HashMap;
use std::num::NonZeroU32;
use std::sync::{LazyLock, Mutex, PoisonError};

use anyhow::{Context, Result, bail, ensure};
use async_trait::async_trait;
use reqwest::header::{REFERER, USER_AGENT};
use reqwest::{Client, StatusCode};
use scraper::{ElementRef, Html, Selector};
use url::Url;

use crate::{Diagnostics, Provider, SearchParams, SearchProvider, SearchResponse, SearchResult};

pub const DUCKDUCKGO_BASE_URL: &str = "https://html.duckduckgo.com/";

/// DuckDuckGo names no engine behind its results.
const SOURCE: &str = "duckduckgo";

/// DuckDuckGo turns down longer queries.
const MAX_QUERY_CHARS: usize = 499;

/// The page token below is tied to the user agent, so it must not vary.
const AGENT: &str = concat!(
    "hodis/",
    env!("CARGO_PKG_VERSION"),
    " (+",
    env!("CARGO_PKG_REPOSITORY"),
    ")"
);

/// What DuckDuckGo's own form sends for "all regions".
const ANY_REGION: &str = "wt-wt";

static RESULT: LazyLock<Selector> =
    LazyLock::new(|| selector("#links .web-result:not(.result--ad)"));
static TITLE: LazyLock<Selector> = LazyLock::new(|| selector("h2 a"));
static SNIPPET: LazyLock<Selector> = LazyLock::new(|| selector(".result__snippet"));
static TOKEN: LazyLock<Selector> = LazyLock::new(|| selector(r#"input[name="vqd"]"#));
static CHALLENGE: LazyLock<Selector> = LazyLock::new(|| selector("#challenge-form"));

/// The token DuckDuckGo hands out with a query's first page, by instance and
/// query. Asking for a later page without one looks like a bot to DuckDuckGo,
/// so a process remembers them rather than fetch the first page each time.
static TOKENS: LazyLock<Mutex<HashMap<(String, String), String>>> = LazyLock::new(Mutex::default);

#[derive(Debug, Clone)]
pub struct DuckDuckGoProvider {
    pub base_url: Url,
}

/// A results page, and the token it carries for the pages after it.
struct Page {
    response: SearchResponse,
    token: Option<String>,
}

impl DuckDuckGoProvider {
    pub fn parse_body(&self, query: &str, body: &str) -> Result<SearchResponse> {
        Ok(read(query, body)?.response)
    }

    async fn fetch(&self, client: &Client, query: &str, form: &[(&str, &str)]) -> Result<Page> {
        let response = client
            .post(self.base_url.join("html/")?)
            .header(USER_AGENT, AGENT)
            .header(REFERER, self.base_url.as_str())
            .form(form)
            .send()
            .await?;
        // DuckDuckGo answers some queries, such as one starting with a
        // backslash, by redirecting to the site of the first result.
        if response.url().origin() != self.base_url.origin() {
            bail!("DuckDuckGo sent the search on to another site");
        }
        let status = response.status();
        let body = response.text().await?;
        if !status.is_success() {
            bail!("DuckDuckGo refused the search ({status})");
        }
        let page = read(query, &body)?;
        // DuckDuckGo turns a search away with 202 Accepted, usually showing a
        // CAPTCHA, which `read` reports.
        ensure!(
            status != StatusCode::ACCEPTED || !page.response.results.is_empty(),
            "DuckDuckGo turned the search away ({status})"
        );
        Ok(page)
    }

    fn token_key(&self, query: &str) -> (String, String) {
        (self.base_url.to_string(), query.to_owned())
    }
}

#[async_trait]
impl SearchProvider for DuckDuckGoProvider {
    async fn search(&self, client: &Client, params: &SearchParams) -> Result<SearchResponse> {
        ensure!(
            params.query.chars().count() <= MAX_QUERY_CHARS,
            "DuckDuckGo takes queries of up to {MAX_QUERY_CHARS} characters"
        );
        let query = quote_bangs(&params.query);
        let key = self.token_key(&query);
        let first = [("q", query.as_str()), ("b", ""), ("kl", ANY_REGION)];

        let page = match params.page.map_or(1, NonZeroU32::get) {
            1 => self.fetch(client, &params.query, &first).await?,
            page => {
                let remembered = tokens().get(&key).cloned();
                let token = match &remembered {
                    Some(token) => token.clone(),
                    None => self
                        .fetch(client, &params.query, &first)
                        .await?
                        .token
                        .context("DuckDuckGo's first page offered no way to the next")?,
                };
                // The second page starts after the first ten results, and each
                // page after it holds fifteen.
                let offset = 10 + u64::from(page - 2) * 15;
                let (s, dc) = (offset.to_string(), (offset + 1).to_string());
                let later = [
                    ("q", query.as_str()),
                    ("s", &s),
                    ("nextParams", ""),
                    ("v", "l"),
                    ("o", "json"),
                    ("dc", &dc),
                    ("api", "d.js"),
                    ("vqd", &token),
                    ("kl", ANY_REGION),
                ];
                match self.fetch(client, &params.query, &later).await {
                    Ok(page) => page,
                    Err(e) => {
                        // A remembered token may have gone stale.
                        if remembered.is_some() {
                            tokens().remove(&key);
                        }
                        return Err(e);
                    }
                }
            }
        };

        if let Some(token) = page.token {
            tokens().insert(key, token);
        }
        Ok(page.response)
    }
}

fn tokens() -> std::sync::MutexGuard<'static, HashMap<(String, String), String>> {
    TOKENS.lock().unwrap_or_else(PoisonError::into_inner)
}

/// `query` with its `!bang` words quoted: unquoted, DuckDuckGo sends the
/// search to the site a bang names rather than answer it.
fn quote_bangs(query: &str) -> String {
    query
        .split_whitespace()
        .map(|word| {
            if word.starts_with('!') {
                format!("'{word}'")
            } else {
                word.to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn read(query: &str, body: &str) -> Result<Page> {
    let document = Html::parse_document(body);
    if document.select(&CHALLENGE).next().is_some() {
        bail!(
            "DuckDuckGo took the search for a bot's and asked for a CAPTCHA; try again later, or search SearXNG with --provider searxng"
        );
    }

    let mut total = 0;
    let mut results = Vec::new();
    for element in document.select(&RESULT) {
        total += 1;
        results.extend(result(element));
    }
    let skipped = total - results.len();
    let token = document
        .select(&TOKEN)
        .find_map(|input| input.value().attr("value"))
        .filter(|token| !token.is_empty())
        .map(str::to_owned);

    Ok(Page {
        response: SearchResponse {
            query: query.to_owned(),
            answer: None,
            results,
            providers: vec![Provider::DuckDuckGo],
            diagnostics: Diagnostics {
                skipped_results: skipped,
                ..Diagnostics::default()
            },
        },
        token,
    })
}

/// A result's title and link, or None when either is missing.
fn result(element: ElementRef) -> Option<SearchResult> {
    let link = element.select(&TITLE).next()?;
    let title = Some(text(link)).filter(|title| !title.is_empty())?;
    let url = Url::parse(link.value().attr("href")?).ok()?;
    Some(SearchResult {
        title,
        url,
        snippet: element
            .select(&SNIPPET)
            .next()
            .map_or_else(String::new, text),
        image: None,
        source: SOURCE.to_owned(),
        provider: Provider::DuckDuckGo,
    })
}

/// An element's text, with the markup's line breaks and indentation folded
/// into single spaces.
fn text(element: ElementRef) -> String {
    element
        .text()
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn selector(css: &str) -> Selector {
    Selector::parse(css).expect("hodis's selectors are valid CSS")
}

#[cfg(test)]
mod tests {
    use super::{CHALLENGE, RESULT, SNIPPET, TITLE, TOKEN, quote_bangs};

    #[test]
    fn the_selectors_parse() {
        for selector in [&RESULT, &TITLE, &SNIPPET, &TOKEN, &CHALLENGE] {
            std::sync::LazyLock::force(selector);
        }
    }

    #[test]
    fn bangs_are_quoted() {
        assert_eq!(quote_bangs("!w  oslo"), "'!w' oslo");
        assert_eq!(quote_bangs("rust !important"), "rust '!important'");
        assert_eq!(quote_bangs("a!b"), "a!b");
    }
}
