//! HTTP-layer tests for `SearXNGProvider::search` against a wiremock server.
//!
//! Each test binds a fresh mock server on a random localhost port and scripts
//! its responses, so the request-building and error-mapping behavior of
//! `search()` is exercised with no real network involved.

use std::time::Duration;

use hodis::SearchProvider;
use hodis::providers::searxng::SearXNGProvider;
use url::Url;
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

const HEALTHY_BODY: &str = include_str!("fixtures/healthy.json");

fn provider_for(server: &MockServer) -> SearXNGProvider {
    SearXNGProvider {
        base_url: Url::parse(&server.uri()).expect("mock server uri is a valid url"),
    }
}

#[tokio::test]
async fn sends_query_and_format_to_the_search_path() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/search"))
        .and(query_param("q", "dns lookup"))
        .and(query_param("format", "json"))
        .respond_with(ResponseTemplate::new(200).set_body_string(HEALTHY_BODY))
        .expect(1)
        .mount(&server)
        .await;

    let response = provider_for(&server)
        .search(&reqwest::Client::new(), "dns lookup")
        .await
        .expect("mocked healthy response parses");

    assert_eq!(response.results.len(), 3);
    // .expect(1) is verified when `server` drops: the request was really made,
    // exactly once, with the matched path and query params.
}

#[tokio::test]
async fn respects_a_base_url_with_a_subpath() {
    // Instances are commonly hosted under a path prefix; joining "search"
    // onto the base url must extend the path, not replace its last segment.
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/searxng/search"))
        .respond_with(ResponseTemplate::new(200).set_body_string(HEALTHY_BODY))
        .expect(1)
        .mount(&server)
        .await;

    let provider = SearXNGProvider {
        base_url: Url::parse(&format!("{}/searxng/", server.uri())).expect("valid url"),
    };
    provider
        .search(&reqwest::Client::new(), "dns lookup")
        .await
        .expect("subpath-hosted instance works");
}

#[tokio::test]
async fn http_403_becomes_an_error() {
    // A SearXNG instance without `json` in its formats list answers 403.
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/search"))
        .respond_with(ResponseTemplate::new(403))
        .mount(&server)
        .await;

    let err = provider_for(&server)
        .search(&reqwest::Client::new(), "test")
        .await
        .expect_err("a 403 must not turn into an empty success");
    assert!(
        err.to_string().contains("403"),
        "error should name the status: {err}"
    );
}

#[tokio::test]
async fn non_json_success_body_becomes_an_error() {
    // Anti-bot interstitials return HTML with status 200.
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/search"))
        .respond_with(ResponseTemplate::new(200).set_body_string("<html>anti-bot page</html>"))
        .mount(&server)
        .await;

    provider_for(&server)
        .search(&reqwest::Client::new(), "test")
        .await
        .expect_err("HTML with status 200 must fail parsing, not panic");
}

#[tokio::test]
async fn a_stalled_server_hits_the_client_timeout() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/search"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(HEALTHY_BODY)
                .set_delay(Duration::from_secs(5)),
        )
        .mount(&server)
        .await;

    let client = reqwest::Client::builder()
        .timeout(Duration::from_millis(200))
        .build()
        .expect("client builds");

    // Message text for timeouts varies across reqwest versions; the contract
    // asserted here is only that a stalled server yields Err, not a hang.
    provider_for(&server)
        .search(&client, "test")
        .await
        .expect_err("stalled server must time out");
}
