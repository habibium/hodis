//! Layout policies of the human-readable text renderer.
//!
//! These tests pin the rules a reader relies on: where the answer block sits,
//! when optional segments disappear, and how results are separated. Content is
//! compared with ANSI escapes stripped; one test guards the escapes themselves.

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
        query: "test query".to_owned(),
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

fn styled(response: &SearchResponse) -> String {
    let mut out = Vec::new();
    render(response, &mut out).expect("writing to a Vec cannot fail");
    String::from_utf8(out).expect("the renderer emits utf-8")
}

fn plain(response: &SearchResponse) -> String {
    strip_str(&styled(response)).to_string()
}

fn plain_diagnostics(diagnostics: &Diagnostics) -> String {
    let mut out = Vec::new();
    render_diagnostics(diagnostics, &mut out).expect("writing to a Vec cannot fail");
    let text = String::from_utf8(out).expect("the renderer emits utf-8");
    strip_str(&text).to_string()
}

#[test]
fn the_answer_leads_the_results() {
    let text = plain(&response(
        Some(answer(
            "Oslo is the capital of Norway.",
            Some("https://en.wikipedia.org/wiki/Oslo"),
        )),
        vec![search_result(
            "Oslo - Wikipedia",
            "https://en.wikipedia.org/wiki/Oslo",
            "Oslo is the capital and most populous city of Norway.",
            "wikipedia",
        )],
    ));

    let answer_at = text
        .find("Oslo is the capital of Norway.")
        .expect("the answer content is rendered");
    let first_at = text
        .find("1. Oslo - Wikipedia")
        .expect("the first result is rendered");
    assert!(answer_at < first_at);
}

#[test]
fn a_response_without_an_answer_starts_at_the_first_result() {
    let text = plain(&response(
        None,
        vec![search_result(
            "Leading title",
            "https://example.com/leading",
            "A summary.",
            "mojeek",
        )],
    ));

    assert!(
        text.starts_with("1. Leading title"),
        "no blank line precedes the first result"
    );
}

#[test]
fn an_empty_source_omits_the_bracket_segment() {
    let text = plain(&response(
        None,
        vec![search_result(
            "Untagged result",
            "https://example.com/untagged",
            "A summary.",
            "",
        )],
    ));

    let title = text.lines().next().expect("the title line is rendered");
    assert_eq!(title, "1. Untagged result");
    assert!(!text.contains('['), "no source segment is emitted");
}

#[test]
fn an_empty_snippet_omits_the_snippet_line() {
    let text = plain(&response(
        None,
        vec![
            search_result("No snippet", "https://example.com/first", "", "mojeek"),
            search_result(
                "With snippet",
                "https://example.com/second",
                "A summary.",
                "brave",
            ),
        ],
    ));

    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(
        lines,
        [
            "1. No snippet [mojeek]",
            "https://example.com/first",
            "",
            "2. With snippet [brave]",
            "https://example.com/second",
            "A summary.",
        ]
    );
}

#[test]
fn results_are_separated_by_a_single_blank_line() {
    let text = plain(&response(
        None,
        vec![
            search_result("First", "https://example.com/1", "One.", "mojeek"),
            search_result("Second", "https://example.com/2", "Two.", "brave"),
            search_result("Third", "https://example.com/3", "Three.", "startpage"),
        ],
    ));

    assert!(
        !text.contains("\n\n\n"),
        "results are never separated by two blank lines"
    );
    assert_eq!(
        text.matches("\n\n").count(),
        2,
        "one blank line per result boundary"
    );
    assert!(
        text.ends_with("Three.\n"),
        "the output ends on the last content line, not a blank one"
    );
}

#[test]
fn clean_diagnostics_render_nothing() {
    let mut out = Vec::new();
    render_diagnostics(&Diagnostics::default(), &mut out).expect("writing to a Vec cannot fail");
    assert!(out.is_empty(), "a clean run writes no diagnostics");
}

#[test]
fn the_skipped_result_count_is_pluralized() {
    let one = plain_diagnostics(&Diagnostics {
        unresponsive_engines: Vec::new(),
        skipped_results: 1,
    });
    assert!(one.contains("warning: skipped 1 malformed result"));
    assert!(!one.contains("malformed results"));

    let many = plain_diagnostics(&Diagnostics {
        unresponsive_engines: Vec::new(),
        skipped_results: 2,
    });
    assert!(many.contains("warning: skipped 2 malformed results"));
}

#[test]
fn unresponsive_engines_are_comma_separated() {
    let text = plain_diagnostics(&Diagnostics {
        unresponsive_engines: vec![
            engine_issue("brave", "CAPTCHA"),
            engine_issue("mojeek", "timeout"),
        ],
        skipped_results: 0,
    });

    let line = text
        .lines()
        .find(|line| line.starts_with("warning:"))
        .expect("the warning line is rendered");
    assert_eq!(
        line,
        "warning: unresponsive engines: brave (CAPTCHA), mojeek (timeout)"
    );
}

#[test]
fn styles_are_always_emitted() {
    // Whether colour reaches the terminal is the caller's decision, made by the
    // AutoStream it renders into, so the renderer writes escapes unconditionally.
    let raw = styled(&response(
        None,
        vec![search_result(
            "Styled title",
            "https://example.com/styled",
            "A summary.",
            "mojeek",
        )],
    ));

    assert!(raw.contains("\x1b["), "ANSI escapes reach the writer");
}
