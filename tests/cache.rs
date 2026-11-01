//! Searches through a cache in a scratch directory, with a mock SearXNG
//! instance behind them that counts the searches reaching it.

use std::fs;
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use hodis::{
    Provider, Providers, SearchParams, SearchResponse, cache::Cache, config::Config, providers,
};

const HEALTHY_BODY: &str = include_str!("fixtures/healthy.json");

const HOUR: Duration = Duration::from_hours(1);

fn params(query: &str, page: u32) -> SearchParams {
    SearchParams {
        query: query.to_owned(),
        page: NonZeroU32::new(page),
    }
}

/// A search of the SearXNG instance at `instance` through `cache`.
async fn search(
    instance: &str,
    cache: &Cache,
    params: &SearchParams,
) -> anyhow::Result<SearchResponse> {
    let config = Config::parse(&format!("[searxng]\nurl = \"{instance}\"\n"))?.config;
    let client = reqwest::Client::new();
    providers::search(
        &Providers::Listed(vec![Provider::SearXNG]),
        params,
        &config,
        &client,
        Some(cache),
        &mut Vec::new(),
    )
    .await
}

/// A SearXNG instance answering `body`, which must see `searches` searches.
async fn searxng(body: &str, searches: u64) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/search"))
        .respond_with(ResponseTemplate::new(200).set_body_string(body))
        .expect(searches)
        .mount(&server)
        .await;
    server
}

fn entries(dir: &Path) -> Vec<PathBuf> {
    fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .collect()
}

/// Makes `entry` look written `by` ago.
fn age(entry: &Path, by: Duration) -> anyhow::Result<()> {
    let file = fs::File::options().write(true).open(entry)?;
    file.set_modified(SystemTime::now() - by)?;
    Ok(())
}

#[tokio::test]
async fn a_repeated_search_is_answered_from_the_cache() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let cache = Cache::new(dir.path().to_path_buf(), HOUR);
    let instance = searxng(HEALTHY_BODY, 1).await;

    let first = search(&instance.uri(), &cache, &params("dns lookup", 1)).await?;
    // Spacing differs from the first query without changing it.
    let second = search(&instance.uri(), &cache, &params(" dns  lookup ", 1)).await?;

    assert_eq!(first.results, second.results);
    assert_eq!(second.query, " dns  lookup ");
    assert_eq!(entries(dir.path()).len(), 1);
    Ok(())
}

#[tokio::test]
async fn without_reads_the_cache_is_searched_past() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let cache = Cache::new(dir.path().to_path_buf(), HOUR);
    let instance = searxng(HEALTHY_BODY, 2).await;

    search(&instance.uri(), &cache, &params("dns lookup", 1)).await?;
    let past = cache.clone().without_reads();
    search(&instance.uri(), &past, &params("dns lookup", 1)).await?;
    Ok(())
}

#[tokio::test]
async fn without_reads_the_cache_still_keeps_what_was_found() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let cache = Cache::new(dir.path().to_path_buf(), HOUR);
    let instance = searxng(HEALTHY_BODY, 1).await;

    let past = cache.clone().without_reads();
    search(&instance.uri(), &past, &params("dns lookup", 1)).await?;
    search(&instance.uri(), &cache, &params("dns lookup", 1)).await?;
    Ok(())
}

#[tokio::test]
async fn an_entry_older_than_the_ttl_is_not_used() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let cache = Cache::new(dir.path().to_path_buf(), HOUR);
    let instance = searxng(HEALTHY_BODY, 2).await;

    search(&instance.uri(), &cache, &params("dns lookup", 1)).await?;
    age(&entries(dir.path())[0], 2 * HOUR)?;
    search(&instance.uri(), &cache, &params("dns lookup", 1)).await?;
    Ok(())
}

#[tokio::test]
async fn pages_and_instances_are_kept_apart() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let cache = Cache::new(dir.path().to_path_buf(), HOUR);
    let instance = searxng(HEALTHY_BODY, 2).await;
    let other = searxng(HEALTHY_BODY, 1).await;

    search(&instance.uri(), &cache, &params("dns lookup", 1)).await?;
    search(&instance.uri(), &cache, &params("dns lookup", 2)).await?;
    // Page 1 is what a search without a page gets, so it is cached already.
    search(&instance.uri(), &cache, &params("dns lookup", 0)).await?;
    search(&other.uri(), &cache, &params("dns lookup", 1)).await?;
    Ok(())
}

#[tokio::test]
async fn a_search_that_found_nothing_is_not_kept() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let cache = Cache::new(dir.path().to_path_buf(), HOUR);
    let instance = searxng("{}", 2).await;

    search(&instance.uri(), &cache, &params("dns lookup", 1)).await?;
    search(&instance.uri(), &cache, &params("dns lookup", 1)).await?;

    assert_eq!(entries(dir.path()), Vec::<PathBuf>::new());
    Ok(())
}

#[tokio::test]
async fn a_damaged_entry_is_searched_past_and_replaced() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let cache = Cache::new(dir.path().to_path_buf(), HOUR);
    let instance = searxng(HEALTHY_BODY, 2).await;

    search(&instance.uri(), &cache, &params("dns lookup", 1)).await?;
    let entry = entries(dir.path()).remove(0);
    fs::write(&entry, "not json")?;
    search(&instance.uri(), &cache, &params("dns lookup", 1)).await?;

    assert_ne!(fs::read_to_string(&entry)?, "not json");
    Ok(())
}

#[tokio::test]
async fn entries_too_old_to_use_are_deleted_on_the_next_write() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let cache = Cache::new(dir.path().to_path_buf(), HOUR);
    let instance = searxng(HEALTHY_BODY, 2).await;

    search(&instance.uri(), &cache, &params("dns lookup", 1)).await?;
    let stale = entries(dir.path()).remove(0);
    age(&stale, 2 * HOUR)?;
    search(&instance.uri(), &cache, &params("dns lookup", 2)).await?;

    let left = entries(dir.path());
    assert_eq!(left.len(), 1);
    assert!(!left.contains(&stale));
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn only_its_owner_may_read_the_cache() -> anyhow::Result<()> {
    use std::os::unix::fs::PermissionsExt as _;

    let dir = tempfile::tempdir()?;
    let searches = dir.path().join("searches");
    let cache = Cache::new(searches.clone(), HOUR);
    let instance = searxng(HEALTHY_BODY, 1).await;

    search(&instance.uri(), &cache, &params("dns lookup", 1)).await?;

    assert_eq!(fs::metadata(&searches)?.permissions().mode() & 0o777, 0o700);
    Ok(())
}

#[test]
fn a_ttl_of_zero_turns_the_cache_off() {
    assert!(Cache::open(Duration::ZERO).is_none());
}
