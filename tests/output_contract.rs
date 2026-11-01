//! Pins the serialized output contract.
//!
//! The `json` format is a machine-readable API consumed by scripts and agents;
//! a failure here means that public contract changed, and its schema version
//! with it.

use serde_json::{Value, json};
use url::Url;

use hodis::{
    Answer, Diagnostics, EngineIssue, Provider, ProviderIssue, SearchProvider, SearchResponse,
    SearchResult,
    output::{OutputFormat, Versioned, render},
};

fn json_output(response: &SearchResponse) -> anyhow::Result<Value> {
    let mut out = Vec::new();
    render(response, OutputFormat::Json, &mut out)?;
    Ok(serde_json::from_slice(&out)?)
}

/// A response with an answer, a result, and diagnostics of every kind.
fn full_response() -> anyhow::Result<SearchResponse> {
    Ok(SearchResponse {
        query: "capital of norway".to_owned(),
        answer: Some(Answer {
            url: Some(Url::parse("https://en.wikipedia.org/wiki/Oslo")?),
            content: "Oslo is the capital of Norway.".to_owned(),
            provider: Provider::SearXNG,
        }),
        results: vec![SearchResult {
            title: "Oslo - Wikipedia".to_owned(),
            url: Url::parse("https://en.wikipedia.org/wiki/Oslo")?,
            snippet: "Oslo is the capital and most populous city of Norway.".to_owned(),
            image: None,
            source: "wikipedia".to_owned(),
            provider: Provider::SearXNG,
        }],
        providers: vec![Provider::SearXNG],
        diagnostics: Diagnostics {
            failed_providers: vec![ProviderIssue {
                provider: Provider::Serper,
                reason: "no answer within 10s".to_owned(),
            }],
            unresponsive_engines: vec![EngineIssue {
                engine: "brave".to_owned(),
                reason: "too many requests".to_owned(),
            }],
            skipped_results: 2,
        },
    })
}

#[test]
fn json_output_has_the_frozen_shape() -> anyhow::Result<()> {
    let expected = json!({
        "schema_version": "2.0.0",
        "query": "capital of norway",
        "answer": {
            "url": "https://en.wikipedia.org/wiki/Oslo",
            "content": "Oslo is the capital of Norway.",
            "provider": "searxng"
        },
        "results": [{
            "title": "Oslo - Wikipedia",
            "url": "https://en.wikipedia.org/wiki/Oslo",
            "snippet": "Oslo is the capital and most populous city of Norway.",
            "image": null,
            "source": "wikipedia",
            "provider": "searxng"
        }],
        "providers": ["searxng"],
        "diagnostics": {
            "failed_providers": [
                { "provider": "serper", "reason": "no answer within 10s" }
            ],
            "unresponsive_engines": [
                { "engine": "brave", "reason": "too many requests" }
            ],
            "skipped_results": 2
        }
    });

    assert_eq!(json_output(&full_response()?)?, expected);
    Ok(())
}

#[test]
fn json_output_conforms_to_its_schema() -> anyhow::Result<()> {
    let schema = serde_json::to_value(schemars::schema_for!(Versioned<SearchResponse>))?;
    let validator = jsonschema::options()
        .should_validate_formats(true)
        .build(&schema)?;

    let mut rich = full_response()?;
    rich.results.push(SearchResult {
        image: Some(Url::parse("https://example.org/oslo.jpg")?),
        ..rich.results[0].clone()
    });
    let mut sparse = full_response()?;
    sparse.answer = None;
    sparse.diagnostics = Diagnostics::default();
    for response in [rich, sparse] {
        let document = json_output(&response)?;
        let errors: Vec<_> = validator
            .iter_errors(&document)
            .map(|e| e.to_string())
            .collect();
        assert!(errors.is_empty(), "{errors:#?}\n{document:#}");
    }
    Ok(())
}

#[test]
fn an_absent_answer_is_null_rather_than_missing() -> anyhow::Result<()> {
    let response = SearchResponse {
        query: "no answer".to_owned(),
        answer: None,
        results: Vec::new(),
        providers: vec![Provider::SearXNG],
        diagnostics: Diagnostics::default(),
    };

    assert_eq!(json_output(&response)?.get("answer"), Some(&Value::Null));
    Ok(())
}

#[test]
fn provider_names_serialize_lowercase() -> anyhow::Result<()> {
    assert_eq!(
        serde_json::to_value(Provider::DuckDuckGo)?,
        json!("duckduckgo")
    );
    assert_eq!(serde_json::to_value(Provider::SearXNG)?, json!("searxng"));
    assert_eq!(serde_json::to_value(Provider::Serper)?, json!("serper"));
    Ok(())
}

#[test]
fn search_provider_trait_stays_dyn_compatible() {
    // Holding providers as trait objects requires the trait to stay
    // dyn-compatible. This fails to compile if that property is lost,
    // e.g. by removing `#[async_trait]`.
    let providers: Vec<Box<dyn SearchProvider>> = Vec::new();
    assert!(providers.is_empty());
}

#[test]
fn every_provider_is_a_valid_value_in_the_schema() -> anyhow::Result<()> {
    let schema = serde_json::to_value(schemars::schema_for!(Versioned<SearchResponse>))?;
    let mut names: Vec<&str> = schema["$defs"]["Provider"]["oneOf"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|variant| variant["const"].as_str())
        .collect();
    names.sort_unstable();
    assert_eq!(names, ["duckduckgo", "searxng", "serper"], "{schema:#}");
    Ok(())
}
