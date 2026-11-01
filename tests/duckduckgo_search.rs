//! Requests to DuckDuckGo, made against a wiremock server standing in for it.
//!
//! The token a first page carries is remembered for the rest of the process
//! by query, so each test searches for a query of its own.

use std::num::NonZeroU32;

use url::Url;
use wiremock::matchers::{body_string_contains, header_regex, method, path};
use wiremock::{Mock, MockBuilder, MockServer, ResponseTemplate};

use hodis::{SearchParams, SearchProvider, providers::duckduckgo::DuckDuckGoProvider};

const HEALTHY_PAGE: &str = include_str!("fixtures/duckduckgo_healthy.html");
const CHALLENGE_PAGE: &str = include_str!("fixtures/duckduckgo_challenge.html");

/// The token in the healthy page's form for the next page.
const TOKEN: &str = "vqd=4-172064077778158217388983369082370494395";

fn provider_for(server: &MockServer) -> DuckDuckGoProvider {
    DuckDuckGoProvider {
        base_url: Url::parse(&server.uri()).expect("the mock's address is a url"),
    }
}

fn params(query: &str, page: u32) -> SearchParams {
    SearchParams {
        query: query.to_owned(),
        page: NonZeroU32::new(page),
    }
}

fn healthy() -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_string(HEALTHY_PAGE)
}

/// A first page request for `query`, encoded as a form would send it.
fn first_page(query: &str) -> MockBuilder {
    Mock::given(method("POST"))
        .and(path("/html/"))
        .and(body_string_contains(format!("q={query}&b=&kl=wt-wt")))
}

#[tokio::test]
async fn a_search_posts_duckduckgos_own_form_and_says_who_asks() {
    let server = MockServer::start().await;
    first_page("dns+lookup")
        .and(header_regex("user-agent", r"^hodis/\d"))
        .respond_with(healthy())
        .expect(1)
        .mount(&server)
        .await;

    let response = provider_for(&server)
        .search(&reqwest::Client::new(), &params("dns lookup", 1))
        .await
        .expect("the search succeeds");

    assert_eq!(response.results.len(), 10);
}

#[tokio::test]
async fn a_later_page_carries_the_token_of_the_first() {
    let server = MockServer::start().await;
    first_page("second+page")
        .respond_with(healthy())
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/html/"))
        .and(body_string_contains("q=second+page&s=10&"))
        .and(body_string_contains("&dc=11&"))
        .and(body_string_contains(TOKEN))
        .respond_with(healthy())
        .expect(1)
        .mount(&server)
        .await;

    provider_for(&server)
        .search(&reqwest::Client::new(), &params("second page", 2))
        .await
        .expect("the search succeeds");
}

#[tokio::test]
async fn a_remembered_token_spares_fetching_the_first_page_again() {
    let server = MockServer::start().await;
    first_page("third+page")
        .respond_with(healthy())
        .expect(1)
        .mount(&server)
        .await;
    for (s, dc) in [(10, 11), (25, 26)] {
        Mock::given(method("POST"))
            .and(path("/html/"))
            .and(body_string_contains(format!("q=third+page&s={s}&")))
            .and(body_string_contains(format!("&dc={dc}&")))
            .and(body_string_contains(TOKEN))
            .respond_with(healthy())
            .expect(1)
            .mount(&server)
            .await;
    }
    let provider = provider_for(&server);
    let client = reqwest::Client::new();

    provider
        .search(&client, &params("third page", 2))
        .await
        .expect("the second page comes");
    provider
        .search(&client, &params("third page", 3))
        .await
        .expect("the third page comes");
}

#[tokio::test]
async fn a_captcha_is_an_error_that_says_so() {
    let server = MockServer::start().await;
    first_page("captcha")
        .respond_with(ResponseTemplate::new(202).set_body_string(CHALLENGE_PAGE))
        .mount(&server)
        .await;

    let error = provider_for(&server)
        .search(&reqwest::Client::new(), &params("captcha", 1))
        .await
        .expect_err("a challenge holds no results");

    assert!(format!("{error:#}").contains("CAPTCHA"), "{error:#}");
}

#[tokio::test]
async fn an_empty_acceptance_is_not_taken_for_no_results() {
    let server = MockServer::start().await;
    first_page("accepted")
        .respond_with(ResponseTemplate::new(202))
        .mount(&server)
        .await;

    let error = provider_for(&server)
        .search(&reqwest::Client::new(), &params("accepted", 1))
        .await
        .expect_err("202 without results is a refusal");

    assert_eq!(
        format!("{error:#}"),
        "DuckDuckGo turned the search away (202 Accepted)"
    );
}

#[tokio::test]
async fn a_refusal_names_its_status() {
    let server = MockServer::start().await;
    first_page("refused")
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;

    let error = provider_for(&server)
        .search(&reqwest::Client::new(), &params("refused", 1))
        .await
        .expect_err("a 500 is a failure");

    assert_eq!(
        format!("{error:#}"),
        "DuckDuckGo refused the search (500 Internal Server Error)"
    );
}

#[tokio::test]
async fn a_bang_is_quoted_so_duckduckgo_searches_for_it() {
    let server = MockServer::start().await;
    first_page("%27%21w%27+oslo")
        .respond_with(healthy())
        .expect(1)
        .mount(&server)
        .await;

    provider_for(&server)
        .search(&reqwest::Client::new(), &params("!w oslo", 1))
        .await
        .expect("the search succeeds");
}

#[tokio::test]
async fn a_redirect_to_another_site_is_an_error() {
    let server = MockServer::start().await;
    let elsewhere = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(healthy())
        .mount(&elsewhere)
        .await;
    first_page("%5Cfirst+result")
        .respond_with(
            ResponseTemplate::new(303)
                .insert_header("location", format!("{}/page", elsewhere.uri())),
        )
        .mount(&server)
        .await;

    let error = provider_for(&server)
        .search(&reqwest::Client::new(), &params("\\first result", 1))
        .await
        .expect_err("another site's page is not DuckDuckGo's results");

    assert_eq!(
        format!("{error:#}"),
        "DuckDuckGo sent the search on to another site"
    );
}

#[tokio::test]
async fn a_query_too_long_for_duckduckgo_stops_before_any_request() {
    let server = MockServer::start().await;

    let error = provider_for(&server)
        .search(&reqwest::Client::new(), &params(&"a".repeat(500), 1))
        .await
        .expect_err("DuckDuckGo would refuse it");

    assert_eq!(
        format!("{error:#}"),
        "DuckDuckGo takes queries of up to 499 characters"
    );
    assert_eq!(server.received_requests().await.map(|r| r.len()), Some(0));
}
