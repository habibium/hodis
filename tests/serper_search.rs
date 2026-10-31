//! HTTP-layer tests for `SerperProvider::search` against a wiremock server.

use std::num::NonZeroU32;

use serde_json::json;
use url::Url;
use wiremock::matchers::{body_json, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use hodis::{SearchParams, SearchProvider, keys::ApiKey, providers::serper::SerperProvider};

const HEALTHY_BODY: &str = include_str!("fixtures/serper_healthy.json");

fn params(q: &str) -> SearchParams {
    SearchParams {
        query: q.to_owned(),
        page: None,
    }
}

fn provider_for(server: &MockServer) -> anyhow::Result<SerperProvider> {
    Ok(SerperProvider {
        base_url: Url::parse(&format!("{}/", server.uri()))?,
        key: ApiKey::new("test-key")?,
    })
}

async fn refusal(status: u16, body: &str) -> anyhow::Result<String> {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/search"))
        .respond_with(ResponseTemplate::new(status).set_body_string(body))
        .mount(&server)
        .await;

    let error = provider_for(&server)?
        .search(&reqwest::Client::new(), &params("capital of norway"))
        .await
        .expect_err("a refusal is an error, not an empty success");
    Ok(error.to_string())
}

#[tokio::test]
async fn posts_the_query_with_the_key_in_a_header() -> anyhow::Result<()> {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/search"))
        .and(header("X-API-KEY", "test-key"))
        // Without a page, `page` stays off the wire rather than pinning one.
        .and(body_json(json!({"q": "capital of norway"})))
        .respond_with(ResponseTemplate::new(200).set_body_string(HEALTHY_BODY))
        .expect(1)
        .mount(&server)
        .await;

    let response = provider_for(&server)?
        .search(&reqwest::Client::new(), &params("capital of norway"))
        .await?;

    assert_eq!(response.results.len(), 3);
    Ok(())
}

#[tokio::test]
async fn sends_the_page_when_one_is_requested() -> anyhow::Result<()> {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/search"))
        .and(body_json(json!({"q": "capital of norway", "page": 2})))
        .respond_with(ResponseTemplate::new(200).set_body_string(HEALTHY_BODY))
        .expect(1)
        .mount(&server)
        .await;

    let params = SearchParams {
        query: "capital of norway".to_owned(),
        page: NonZeroU32::new(2),
    };
    provider_for(&server)?
        .search(&reqwest::Client::new(), &params)
        .await?;
    Ok(())
}

#[tokio::test]
async fn a_refusal_names_the_status_and_serpers_reason() -> anyhow::Result<()> {
    let error = refusal(403, r#"{"message": "Unauthorized.", "statusCode": 403}"#).await?;
    assert!(error.contains("403"), "{error}");
    assert!(error.contains("Unauthorized."), "{error}");
    Ok(())
}

#[tokio::test]
async fn a_refusal_without_a_reason_names_the_status() -> anyhow::Result<()> {
    let error = refusal(502, "<html>Bad gateway</html>").await?;
    assert!(error.contains("502"), "{error}");
    Ok(())
}

#[tokio::test]
async fn control_characters_are_dropped_from_serpers_reason() -> anyhow::Result<()> {
    let error = refusal(400, r#"{"message": "Not enough \u001b[31mcredits"}"#).await?;
    assert!(error.contains("Not enough [31mcredits"), "{error:?}");
    assert!(!error.contains('\u{1b}'), "{error:?}");
    Ok(())
}
