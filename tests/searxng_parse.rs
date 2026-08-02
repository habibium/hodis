use hodis::Provider;
use hodis::providers::searxng::{SEARXNG_BASE_URL, SearXNGProvider};
use url::Url;

fn provider() -> SearXNGProvider {
    SearXNGProvider {
        base_url: Url::parse(SEARXNG_BASE_URL).expect("const base url is valid"),
    }
}

#[test]
fn healthy_response_keeps_all_results_and_the_answer() -> anyhow::Result<()> {
    let r = provider().parse_body("dns lookup", include_str!("fixtures/healthy.json"))?;

    assert_eq!(r.query, "dns lookup");
    assert_eq!(r.provider, Provider::SearXNG);
    assert_eq!(r.results.len(), 3);
    assert_eq!(r.diagnostics.skipped_results, 0);

    let first = &r.results[0];
    assert_eq!(first.title, "DNS Lookup Tool - DNS Tools - MxToolbox");
    assert_eq!(first.url.as_str(), "https://mxtoolbox.com/DNSLookup.aspx");
    assert_eq!(first.source, "google cse");

    let answer = r.answer.expect("fixture carries one real answer");
    assert!(
        answer
            .content
            .starts_with("In computer networks, a reverse DNS lookup")
    );
    assert_eq!(
        answer.url.map(|u| u.to_string()).as_deref(),
        Some("https://en.wikipedia.org/wiki/Reverse_DNS_lookup")
    );

    assert_eq!(r.diagnostics.unresponsive_engines.len(), 2);
    assert_eq!(r.diagnostics.unresponsive_engines[0].engine, "brave");
    Ok(())
}

#[test]
fn null_fields_degrade_and_null_titles_are_skipped() -> anyhow::Result<()> {
    let r = provider().parse_body("null fields", include_str!("fixtures/null_fields.json"))?;

    assert_eq!(r.results.len(), 2);
    assert_eq!(r.diagnostics.skipped_results, 1);

    let first = &r.results[0];
    assert_eq!(first.title, "rancher/rke2-runtime");
    assert_eq!(first.image, None, "null thumbnail degrades to no image");
    assert_eq!(
        first.snippet, "",
        "missing content degrades to empty snippet"
    );

    let answer = r.answer.expect("second answers entry is non-empty");
    assert!(answer.content.starts_with("Soldering"));

    assert!(r.diagnostics.unresponsive_engines.is_empty());
    Ok(())
}

#[test]
fn empty_string_urls_are_skipped_not_fatal() -> anyhow::Result<()> {
    let r = provider().parse_body(
        "nature wildlife photography",
        include_str!("fixtures/empty_urls.json"),
    )?;

    // Three wire results: one with an empty-string url, two valid.
    assert_eq!(r.results.len(), 2);
    assert_eq!(r.diagnostics.skipped_results, 1);
    assert!(r.answer.is_none());
    Ok(())
}

#[test]
fn total_engine_outage_yields_empty_results_with_diagnostics() -> anyhow::Result<()> {
    let r = provider().parse_body(
        "10 liters to gallons",
        include_str!("fixtures/engines_down.json"),
    )?;

    assert!(r.results.is_empty());
    assert!(r.answer.is_none());
    assert_eq!(r.diagnostics.skipped_results, 0);

    let engines: Vec<&str> = r
        .diagnostics
        .unresponsive_engines
        .iter()
        .map(|e| e.engine.as_str())
        .collect();
    assert_eq!(
        engines,
        [
            "brave",
            "duckduckgo",
            "google cse",
            "mojeek",
            "startpage",
            "wikipedia"
        ]
    );
    assert_eq!(r.diagnostics.unresponsive_engines[1].reason, "CAPTCHA");
    Ok(())
}

#[test]
fn non_json_body_is_an_error_not_a_panic() {
    // An HTML error page, as served when the json format is not enabled.
    let result = provider().parse_body("test", include_str!("fixtures/not_json.html"));
    assert!(result.is_err());
}
