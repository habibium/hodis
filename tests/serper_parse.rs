//! Mapping Serper's response onto `SearchResponse`, with no network involved.

use url::Url;

use hodis::{
    Answer, Diagnostics, Provider, SearchResponse,
    keys::ApiKey,
    providers::serper::{SERPER_BASE_URL, SerperProvider},
};

const HEALTHY_BODY: &str = include_str!("fixtures/serper_healthy.json");

fn parse(body: &str) -> anyhow::Result<SearchResponse> {
    let provider = SerperProvider {
        base_url: Url::parse(SERPER_BASE_URL)?,
        key: ApiKey::new("test-key")?,
    };
    provider.parse_body("capital of norway", body)
}

#[test]
fn organic_results_become_results_from_google() -> anyhow::Result<()> {
    let response = parse(HEALTHY_BODY)?;

    assert_eq!(response.provider, Provider::Serper);
    assert_eq!(response.query, "capital of norway");
    assert_eq!(response.results.len(), 3);
    let first = &response.results[0];
    assert_eq!(first.title, "Oslo - Wikipedia");
    assert_eq!(first.url.as_str(), "https://en.wikipedia.org/wiki/Oslo");
    assert!(first.snippet.starts_with("Oslo is the capital"));
    assert_eq!(first.image, None);
    assert!(response.results.iter().all(|r| r.source == "google"));
    assert_eq!(
        response.results[2].image.as_ref().map(Url::as_str),
        Some("https://cdn.britannica.com/oslo-harbour.jpg")
    );
    assert_eq!(response.diagnostics, Diagnostics::default());
    Ok(())
}

#[test]
fn a_short_answer_in_the_answer_box_is_the_answer() -> anyhow::Result<()> {
    let response = parse(HEALTHY_BODY)?;
    assert_eq!(
        response.answer,
        Some(Answer {
            url: None,
            content: "Oslo".to_owned(),
        })
    );
    Ok(())
}

#[test]
fn an_answer_box_quoting_a_page_answers_with_its_link() -> anyhow::Result<()> {
    let response = parse(
        r#"{"answerBox": {
            "title": "Rust (programming language) - Wikipedia",
            "snippet": "Rust is a general-purpose programming language.",
            "link": "https://en.wikipedia.org/wiki/Rust_(programming_language)"
        }}"#,
    )?;

    assert_eq!(
        response.answer,
        Some(Answer {
            url: Some(Url::parse(
                "https://en.wikipedia.org/wiki/Rust_(programming_language)"
            )?),
            content: "Rust is a general-purpose programming language.".to_owned(),
        })
    );
    Ok(())
}

#[test]
fn an_answer_box_without_text_is_no_answer() -> anyhow::Result<()> {
    let response = parse(r#"{"answerBox": {"answer": "", "title": "Norway"}}"#)?;
    assert_eq!(response.answer, None);
    Ok(())
}

#[test]
fn results_without_a_title_or_a_valid_link_are_skipped_and_counted() -> anyhow::Result<()> {
    let response = parse(
        r#"{"organic": [
            {"title": "Kept", "link": "https://example.org/"},
            {"link": "https://example.org/untitled"},
            {"title": "", "link": "https://example.org/blank"},
            {"title": "No link"},
            {"title": "Bad link", "link": "not a url"}
        ]}"#,
    )?;

    assert_eq!(response.results.len(), 1);
    assert_eq!(response.results[0].title, "Kept");
    assert_eq!(response.diagnostics.skipped_results, 4);
    Ok(())
}

#[test]
fn a_response_with_nothing_found_is_empty() -> anyhow::Result<()> {
    let response = parse(r#"{"searchParameters": {"q": "capital of norway"}, "credits": 1}"#)?;
    assert_eq!(response.results, []);
    assert_eq!(response.answer, None);
    Ok(())
}

#[test]
fn a_body_that_is_not_json_is_an_error() {
    parse("<html>Bad gateway</html>").expect_err("HTML is not a Serper response");
}
