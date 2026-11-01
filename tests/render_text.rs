//! Layout policies of the `pretty` renderer.
//!
//! Content is compared with ANSI escapes stripped; two tests guard the escapes
//! themselves.

use anstream::adapter::strip_str;
use url::Url;

use hodis::{
    Answer, Diagnostics, EngineIssue, Provider, ProviderIssue, SearchResponse, SearchResult,
    output::{OutputFormat, render, render_diagnostics, render_warning},
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
        query: "test query".to_owned(),
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

fn styled(response: &SearchResponse) -> String {
    let mut out = Vec::new();
    render(response, OutputFormat::Pretty, &mut out).expect("writing to a Vec cannot fail");
    String::from_utf8(out).expect("the renderer emits utf-8")
}

fn unstyled(response: &SearchResponse) -> String {
    strip_str(&styled(response)).to_string()
}

fn unstyled_diagnostics(diagnostics: &Diagnostics) -> String {
    let mut out = Vec::new();
    render_diagnostics(diagnostics, &mut out).expect("writing to a Vec cannot fail");
    let text = String::from_utf8(out).expect("the renderer emits utf-8");
    strip_str(&text).to_string()
}

#[test]
fn the_answer_leads_the_results() {
    let text = unstyled(&response(
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
    let text = unstyled(&response(
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
    let text = unstyled(&response(
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
    let text = unstyled(&response(
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
    let text = unstyled(&response(
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
    let one = unstyled_diagnostics(&Diagnostics {
        failed_providers: Vec::new(),
        unresponsive_engines: Vec::new(),
        skipped_results: 1,
    });
    assert!(one.contains("warning: skipped 1 malformed result"));
    assert!(!one.contains("malformed results"));

    let many = unstyled_diagnostics(&Diagnostics {
        failed_providers: Vec::new(),
        unresponsive_engines: Vec::new(),
        skipped_results: 2,
    });
    assert!(many.contains("warning: skipped 2 malformed results"));
}

#[test]
fn unresponsive_engines_are_comma_separated() {
    let text = unstyled_diagnostics(&Diagnostics {
        failed_providers: Vec::new(),
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
fn each_failed_provider_gets_a_warning_with_its_reason() {
    let text = unstyled_diagnostics(&Diagnostics {
        failed_providers: vec![ProviderIssue {
            provider: Provider::Serper,
            reason: "no answer\nwithin 10s".to_owned(),
        }],
        ..Diagnostics::default()
    });

    assert_eq!(
        text,
        "\nwarning: cannot search Serper: no answer within 10s\n"
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

#[test]
fn control_characters_in_provider_text_never_reach_the_writer() {
    // With colour on, the stream passes escapes through untouched, so any
    // planted in a page title would be acted on by the terminal.
    let raw = styled(&response(
        Some(answer("Answer\u{1b}]0;retitled\u{7}", None)),
        vec![search_result(
            "Title\u{1b}[2J",
            "https://example.com/hostile",
            "Snippet\u{9b}5m",
            "mojeek\u{1b}[8m",
        )],
    ));

    for planted in ["\u{1b}]0;", "\u{7}", "\u{1b}[2J", "\u{9b}", "\u{1b}[8m"] {
        assert!(!raw.contains(planted), "{planted:?} must not be written");
    }
}

#[test]
fn line_breaks_in_provider_text_collapse_to_spaces() {
    let text = unstyled(&response(
        None,
        vec![search_result(
            "Split\ntitle",
            "https://example.com/split",
            "First line.\n\nSecond line.",
            "mojeek",
        )],
    ));

    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(
        lines,
        [
            "1. Split title [mojeek]",
            "https://example.com/split",
            "First line. Second line.",
        ]
    );
}

#[test]
fn a_warning_is_a_single_prefixed_line() {
    let mut out = Vec::new();
    render_warning("unknown key `fromat` in config.toml", &mut out)
        .expect("writing to a Vec cannot fail");
    let text = String::from_utf8(out).expect("the renderer emits utf-8");

    assert_eq!(
        strip_str(&text).to_string(),
        "warning: unknown key `fromat` in config.toml\n"
    );
}
