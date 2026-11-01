pub mod searxng;
pub mod serper;

use std::collections::{HashMap, HashSet};
use std::io::Write;

use anyhow::{Result, anyhow, bail, ensure};
use futures::future::join_all;
use reqwest::Client;
use tokio::time::{Instant, timeout_at};
use url::Url;

use crate::config::{self, Config};
use crate::engine::{Engine, is_reachable};
use crate::keys::{self, KeyedProvider};
use crate::{
    Diagnostics, Provider, ProviderIssue, SearchParams, SearchProvider, SearchResponse,
    SearchResult,
};
use searxng::SearXNGProvider;
use serper::SerperProvider;

/// Reciprocal rank fusion's constant, the value its authors found to work well.
const FUSION_K: f64 = 60.0;

/// Searches `providers` at once and merges their results. Each one has until
/// `config.timeout` to answer; those that fail or run out of time are reported
/// in the diagnostics, and only a search that every one fails is an error.
/// Steps taken to set up the engine hodis runs itself are reported to
/// `progress`.
pub async fn search(
    providers: &[Provider],
    params: &SearchParams,
    config: &Config,
    client: &Client,
    progress: &mut impl Write,
) -> Result<SearchResponse> {
    let providers = distinct(providers);
    ensure!(!providers.is_empty(), "there is no provider to search");

    // One at a time, as setting up hodis's own engine reports to `progress`.
    let mut connections = Vec::with_capacity(providers.len());
    for &provider in &providers {
        connections.push(connect(provider, config, client, progress).await);
    }

    let deadline = Instant::now() + config.timeout;
    let outcomes = join_all(connections.into_iter().map(|connection| async move {
        let provider = connection?;
        timeout_at(deadline, provider.search(client, params))
            .await
            .unwrap_or_else(|_| Err(anyhow!("no answer within {}s", config.timeout.as_secs())))
    }))
    .await;

    let mut answered = Vec::new();
    let mut failed = Vec::new();
    for (provider, outcome) in providers.into_iter().zip(outcomes) {
        match outcome {
            Ok(response) => answered.push(response),
            Err(e) => failed.push((provider, e)),
        }
    }
    if answered.is_empty() {
        return Err(all_failed(failed));
    }

    let failed = failed
        .into_iter()
        .map(|(provider, e)| ProviderIssue {
            provider,
            reason: format!("{e:#}"),
        })
        .collect();
    Ok(merge(params.query.clone(), answered, failed))
}

/// `provider`, set up as `config` says and ready to search. Steps taken to set
/// up the engine hodis runs itself are reported to `progress`.
async fn connect(
    provider: Provider,
    config: &Config,
    client: &Client,
    progress: &mut impl Write,
) -> Result<Box<dyn SearchProvider>> {
    Ok(match provider {
        Provider::SearXNG => Box::new(SearXNGProvider {
            base_url: searxng_url(client, &config.searxng, progress).await?,
        }),
        Provider::Serper => Box::new(SerperProvider {
            base_url: config.serper.url.clone(),
            // The keychain may wait on an unlock prompt, which must not stall the runtime.
            key: tokio::task::spawn_blocking(|| keys::find(KeyedProvider::Serper)).await??,
        }),
        Provider::DuckDuckGo => bail!("hodis cannot search DuckDuckGo yet"),
    })
}

/// The configured instance is one the user may or may not be running; when
/// nothing answers there, hodis falls back to the engine it runs itself, if
/// allowed.
async fn searxng_url(
    client: &Client,
    instance: &config::SearXNG,
    progress: &mut impl Write,
) -> Result<Url> {
    if !instance.embedded || is_reachable(client, &instance.url).await {
        Ok(instance.url.clone())
    } else {
        Engine::resolve()?.ensure_running(client, progress).await
    }
}

/// `providers` in order, each named once.
fn distinct(providers: &[Provider]) -> Vec<Provider> {
    let mut distinct = Vec::with_capacity(providers.len());
    for &provider in providers {
        if !distinct.contains(&provider) {
            distinct.push(provider);
        }
    }
    distinct
}

/// The error for a search no provider answered: the provider's own when only
/// one was asked, or else every provider's reason.
fn all_failed(mut failed: Vec<(Provider, anyhow::Error)>) -> anyhow::Error {
    if failed.len() == 1 {
        let (provider, e) = failed.remove(0);
        return e.context(format!("cannot search {}", provider.name()));
    }
    let reasons: Vec<_> = failed
        .iter()
        .map(|(provider, e)| format!("{}: {e:#}", provider.name()))
        .collect();
    anyhow!("cannot search any provider: {}", reasons.join("; "))
}

struct Fused {
    score: f64,
    /// Rank and provider position of the copy kept, the earliest provider's
    /// among copies ranked alike.
    best: (usize, usize),
    result: SearchResult,
}

/// Combines the responses of the providers that answered, in the order they
/// were asked, with the failures of the rest. A result's place comes from
/// reciprocal rank fusion, so one that several providers rank well rises above
/// one that a single provider does; the answer is the first provider's that
/// has one.
fn merge(
    query: String,
    answered: Vec<SearchResponse>,
    failed_providers: Vec<ProviderIssue>,
) -> SearchResponse {
    let mut merged = SearchResponse {
        query,
        answer: None,
        results: Vec::new(),
        providers: Vec::new(),
        diagnostics: Diagnostics {
            failed_providers,
            ..Diagnostics::default()
        },
    };
    let mut fused: Vec<Fused> = Vec::new();
    let mut by_url: HashMap<Url, usize> = HashMap::new();

    for (position, response) in answered.into_iter().enumerate() {
        merged.answer = merged.answer.or(response.answer);
        merged.providers.extend(response.providers);
        let diagnostics = response.diagnostics;
        merged
            .diagnostics
            .unresponsive_engines
            .extend(diagnostics.unresponsive_engines);
        merged.diagnostics.skipped_results += diagnostics.skipped_results;

        let mut seen = HashSet::new();
        for (rank, result) in response.results.into_iter().enumerate() {
            // A provider listing a page twice vouches for it once.
            if !seen.insert(result.url.clone()) {
                continue;
            }
            let score = fusion_score(rank);
            let best = (rank, position);
            if let Some(&i) = by_url.get(&result.url) {
                let entry = &mut fused[i];
                entry.score += score;
                if best < entry.best {
                    entry.best = best;
                    entry.result = result;
                }
            } else {
                by_url.insert(result.url.clone(), fused.len());
                fused.push(Fused {
                    score,
                    best,
                    result,
                });
            }
        }
    }

    fused.sort_by(|a, b| b.score.total_cmp(&a.score).then(a.best.cmp(&b.best)));
    merged.results = fused.into_iter().map(|entry| entry.result).collect();
    merged
}

/// What a result at `rank`, counting from 0, adds to its fused score.
fn fusion_score(rank: usize) -> f64 {
    let rank = u32::try_from(rank).unwrap_or(u32::MAX);
    1.0 / (FUSION_K + f64::from(rank) + 1.0)
}

#[cfg(test)]
mod tests {
    use anyhow::anyhow;
    use url::Url;

    use super::{all_failed, distinct, merge};
    use crate::{
        Answer, Diagnostics, EngineIssue, Provider, ProviderIssue, SearchResponse, SearchResult,
    };

    fn response(provider: Provider, urls: &[&str]) -> SearchResponse {
        SearchResponse {
            query: "oslo".to_owned(),
            answer: None,
            results: urls
                .iter()
                .map(|url| SearchResult {
                    title: (*url).to_owned(),
                    url: Url::parse(url).expect("test urls are valid"),
                    snippet: String::new(),
                    image: None,
                    source: String::new(),
                    provider,
                })
                .collect(),
            providers: vec![provider],
            diagnostics: Diagnostics::default(),
        }
    }

    fn answer(provider: Provider, content: &str) -> Answer {
        Answer {
            url: None,
            content: content.to_owned(),
            provider,
        }
    }

    fn urls(response: &SearchResponse) -> Vec<&str> {
        response.results.iter().map(|r| r.url.as_str()).collect()
    }

    #[test]
    fn one_provider_keeps_its_order() {
        let merged = merge(
            "oslo".to_owned(),
            vec![response(
                Provider::SearXNG,
                &["https://a/", "https://b/", "https://c/"],
            )],
            Vec::new(),
        );

        assert_eq!(urls(&merged), ["https://a/", "https://b/", "https://c/"]);
        assert_eq!(merged.providers, [Provider::SearXNG]);
    }

    #[test]
    fn a_page_both_providers_found_rises_above_the_rest() {
        let merged = merge(
            "oslo".to_owned(),
            vec![
                response(
                    Provider::SearXNG,
                    &["https://a/", "https://b/", "https://c/"],
                ),
                response(
                    Provider::Serper,
                    &["https://d/", "https://c/", "https://e/"],
                ),
            ],
            Vec::new(),
        );

        // Ties at a rank go to the provider asked first.
        assert_eq!(
            urls(&merged),
            [
                "https://c/",
                "https://a/",
                "https://d/",
                "https://b/",
                "https://e/"
            ]
        );
        assert_eq!(merged.providers, [Provider::SearXNG, Provider::Serper]);
    }

    #[test]
    fn a_page_both_found_is_listed_once_as_the_better_ranked_copy() {
        let merged = merge(
            "oslo".to_owned(),
            vec![
                response(Provider::SearXNG, &["https://a/", "https://x/"]),
                response(Provider::Serper, &["https://x/"]),
            ],
            Vec::new(),
        );

        let copies: Vec<_> = merged
            .results
            .iter()
            .filter(|r| r.url.as_str() == "https://x/")
            .collect();
        assert_eq!(copies.len(), 1);
        assert_eq!(copies[0].provider, Provider::Serper);
    }

    #[test]
    fn copies_ranked_alike_keep_the_first_providers() {
        let merged = merge(
            "oslo".to_owned(),
            vec![
                response(Provider::SearXNG, &["https://x/"]),
                response(Provider::Serper, &["https://x/"]),
            ],
            Vec::new(),
        );

        assert_eq!(merged.results.len(), 1);
        assert_eq!(merged.results[0].provider, Provider::SearXNG);
    }

    #[test]
    fn a_page_one_provider_lists_twice_counts_once() {
        let merged = merge(
            "oslo".to_owned(),
            vec![
                response(
                    Provider::SearXNG,
                    &["https://x/", "https://a/", "https://x/"],
                ),
                response(Provider::Serper, &["https://a/"]),
            ],
            Vec::new(),
        );

        assert_eq!(urls(&merged), ["https://a/", "https://x/"]);
    }

    #[test]
    fn the_answer_is_the_first_providers_that_has_one() {
        let mut searxng = response(Provider::SearXNG, &[]);
        let mut serper = response(Provider::Serper, &[]);
        serper.answer = Some(answer(Provider::Serper, "Oslo"));
        let merged = merge(
            "oslo".to_owned(),
            vec![searxng.clone(), serper.clone()],
            Vec::new(),
        );
        assert_eq!(merged.answer, Some(answer(Provider::Serper, "Oslo")));

        searxng.answer = Some(answer(Provider::SearXNG, "Oslo, Norway"));
        let merged = merge("oslo".to_owned(), vec![searxng, serper], Vec::new());
        assert_eq!(
            merged.answer,
            Some(answer(Provider::SearXNG, "Oslo, Norway"))
        );
    }

    #[test]
    fn diagnostics_add_up() {
        let mut searxng = response(Provider::SearXNG, &[]);
        searxng.diagnostics.unresponsive_engines = vec![EngineIssue {
            engine: "brave".to_owned(),
            reason: "CAPTCHA".to_owned(),
        }];
        searxng.diagnostics.skipped_results = 2;
        let mut serper = response(Provider::Serper, &[]);
        serper.diagnostics.skipped_results = 1;
        let failed = vec![ProviderIssue {
            provider: Provider::DuckDuckGo,
            reason: "no answer within 10s".to_owned(),
        }];

        let merged = merge("oslo".to_owned(), vec![searxng, serper], failed.clone());

        assert_eq!(merged.diagnostics.failed_providers, failed);
        assert_eq!(merged.diagnostics.unresponsive_engines.len(), 1);
        assert_eq!(merged.diagnostics.skipped_results, 3);
    }

    #[test]
    fn a_provider_named_twice_is_searched_once() {
        assert_eq!(
            distinct(&[Provider::Serper, Provider::SearXNG, Provider::Serper]),
            [Provider::Serper, Provider::SearXNG]
        );
    }

    #[test]
    fn a_lone_failure_keeps_its_own_error_under_the_providers_name() {
        let error = all_failed(vec![(Provider::Serper, anyhow!("no Serper API key"))]);
        assert_eq!(
            format!("{error:#}"),
            "cannot search Serper: no Serper API key"
        );
    }

    #[test]
    fn failures_together_name_every_provider() {
        let error = all_failed(vec![
            (Provider::SearXNG, anyhow!("no answer within 10s")),
            (Provider::Serper, anyhow!("no Serper API key")),
        ]);
        assert_eq!(
            format!("{error:#}"),
            "cannot search any provider: SearXNG: no answer within 10s; Serper: no Serper API key"
        );
    }
}
