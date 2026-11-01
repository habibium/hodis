//! Whole-output snapshots of the text renderers.
//!
//! The `render_*.rs` files pin individual policies; these catch drift in the
//! output as a whole. ANSI escapes are stripped so `.snap` files stay readable.

use anstream::adapter::strip_str;
use url::Url;

use hodis::{
    Answer, Diagnostics, EngineIssue, Provider, SearchResponse, SearchResult,
    output::{OutputFormat, render, render_diagnostics},
};

fn parse_url(raw: &str) -> Url {
    Url::parse(raw).expect("test urls are valid")
}

fn answer(content: &str, url: Option<&str>) -> Answer {
    Answer {
        url: url.map(parse_url),
        content: content.to_owned(),
        provider: Provider::SearXNG,
    }
}

fn search_result(title: &str, url: &str, snippet: &str, source: &str) -> SearchResult {
    SearchResult {
        title: title.to_owned(),
        url: parse_url(url),
        snippet: snippet.to_owned(),
        image: None,
        source: source.to_owned(),
        provider: Provider::SearXNG,
    }
}

fn response(answer: Option<Answer>, results: Vec<SearchResult>) -> SearchResponse {
    SearchResponse {
        query: "rust ownership".to_owned(),
        answer,
        results,
        providers: vec![Provider::SearXNG],
        diagnostics: Diagnostics::default(),
    }
}

fn engine_issue(engine: &str, reason: &str) -> EngineIssue {
    EngineIssue {
        engine: engine.to_owned(),
        reason: reason.to_owned(),
    }
}

fn unstyled(response: &SearchResponse, format: OutputFormat) -> String {
    let mut out = Vec::new();
    render(response, format, &mut out).expect("writing to a Vec cannot fail");
    let text = String::from_utf8(out).expect("the renderer emits utf-8");
    strip_str(&text).to_string()
}

fn unstyled_diagnostics(diagnostics: &Diagnostics) -> String {
    let mut out = Vec::new();
    render_diagnostics(diagnostics, &mut out).expect("writing to a Vec cannot fail");
    let text = String::from_utf8(out).expect("the renderer emits utf-8");
    strip_str(&text).to_string()
}

fn full() -> SearchResponse {
    response(
        Some(answer(
            "Ownership is Rust's memory discipline: every value has one owner, and the value is dropped when that owner goes out of scope.",
            Some("https://doc.rust-lang.org/book/ch04-01-what-is-ownership.html"),
        )),
        vec![
            search_result(
                "What Is Ownership? - The Rust Programming Language",
                "https://doc.rust-lang.org/book/ch04-01-what-is-ownership.html",
                "Ownership is a set of rules that govern how a Rust program manages memory.",
                "duckduckgo",
            ),
            search_result(
                "Scoping rules - Rust By Example",
                "https://doc.rust-lang.org/rust-by-example/scope/move.html",
                "Resources can only have one owner at a time.",
                "google cse",
            ),
            search_result(
                "Rust ownership, borrowing and lifetimes",
                "https://example.com/blog/rust-ownership",
                "A walkthrough of moves, borrows and lifetimes with worked examples.",
                "mojeek",
            ),
        ],
    )
}

#[test]
fn full_response() {
    let rendered = unstyled(&full(), OutputFormat::Pretty);

    insta::assert_snapshot!(rendered);
}

#[test]
fn full_response_as_markdown() {
    let rendered = unstyled(&full(), OutputFormat::Markdown);

    insta::assert_snapshot!(rendered);
}

#[test]
fn full_response_as_plain_text() {
    let rendered = unstyled(&full(), OutputFormat::Plain);

    insta::assert_snapshot!(rendered);
}

#[test]
fn full_response_as_urls() {
    let rendered = unstyled(&full(), OutputFormat::Urls);

    insta::assert_snapshot!(rendered);
}

#[test]
fn results_without_an_answer() {
    let rendered = unstyled(
        &response(
            None,
            vec![
                search_result(
                    "The Rust Programming Language",
                    "https://doc.rust-lang.org/book/",
                    "An introductory book about Rust.",
                    "duckduckgo",
                ),
                search_result(
                    "Rust Forge",
                    "https://forge.rust-lang.org/",
                    "Documentation for the Rust project's infrastructure and processes.",
                    "startpage",
                ),
            ],
        ),
        OutputFormat::Pretty,
    );

    insta::assert_snapshot!(rendered);
}

#[test]
fn minimal_result() {
    let rendered = unstyled(
        &response(
            None,
            vec![search_result(
                "Untitled source",
                "https://example.com/minimal",
                "",
                "",
            )],
        ),
        OutputFormat::Pretty,
    );

    insta::assert_snapshot!(rendered);
}

#[test]
fn diagnostics_with_engines_and_skips() {
    let rendered = unstyled_diagnostics(&Diagnostics {
        failed_providers: Vec::new(),
        unresponsive_engines: vec![
            engine_issue("brave", "CAPTCHA"),
            engine_issue("mojeek", "timeout"),
        ],
        skipped_results: 3,
    });

    insta::assert_snapshot!(rendered);
}
