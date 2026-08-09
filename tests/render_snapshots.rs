//! Whole-output snapshots of the human-readable text renderer.
//!
//! `render_text.rs` pins individual layout policies; these snapshots catch
//! unintended drift in the output as a whole. ANSI escapes are stripped so the
//! stored `.snap` files stay readable in review.

use anstream::adapter::strip_str;
use hodis::output::{render, render_diagnostics};
use hodis::{Answer, Diagnostics, EngineIssue, Provider, SearchResponse, SearchResult};
use url::Url;

fn parse_url(raw: &str) -> Url {
    Url::parse(raw).expect("test urls are valid")
}

fn answer(content: &str, url: Option<&str>) -> Answer {
    Answer {
        url: url.map(parse_url),
        content: content.to_owned(),
    }
}

fn search_result(title: &str, url: &str, snippet: &str, source: &str) -> SearchResult {
    SearchResult {
        title: title.to_owned(),
        url: parse_url(url),
        snippet: snippet.to_owned(),
        image: None,
        source: source.to_owned(),
    }
}

fn response(answer: Option<Answer>, results: Vec<SearchResult>) -> SearchResponse {
    SearchResponse {
        query: "rust ownership".to_owned(),
        answer,
        results,
        provider: Provider::SearXNG,
        diagnostics: Diagnostics::default(),
    }
}

fn engine_issue(engine: &str, reason: &str) -> EngineIssue {
    EngineIssue {
        engine: engine.to_owned(),
        reason: reason.to_owned(),
    }
}

fn plain(response: &SearchResponse) -> String {
    let mut out = Vec::new();
    render(response, &mut out).expect("writing to a Vec cannot fail");
    let text = String::from_utf8(out).expect("the renderer emits utf-8");
    strip_str(&text).to_string()
}

fn plain_diagnostics(diagnostics: &Diagnostics) -> String {
    let mut out = Vec::new();
    render_diagnostics(diagnostics, &mut out).expect("writing to a Vec cannot fail");
    let text = String::from_utf8(out).expect("the renderer emits utf-8");
    strip_str(&text).to_string()
}

#[test]
fn full_response() {
    let rendered = plain(&response(
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
    ));

    insta::assert_snapshot!(rendered);
}

#[test]
fn results_without_an_answer() {
    let rendered = plain(&response(
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
    ));

    insta::assert_snapshot!(rendered);
}

#[test]
fn minimal_result() {
    let rendered = plain(&response(
        None,
        vec![search_result(
            "Untitled source",
            "https://example.com/minimal",
            "",
            "",
        )],
    ));

    insta::assert_snapshot!(rendered);
}

#[test]
fn diagnostics_with_engines_and_skips() {
    let rendered = plain_diagnostics(&Diagnostics {
        unresponsive_engines: vec![
            engine_issue("brave", "CAPTCHA"),
            engine_issue("mojeek", "timeout"),
        ],
        skipped_results: 3,
    });

    insta::assert_snapshot!(rendered);
}
