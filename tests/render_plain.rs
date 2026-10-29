//! Layout policies of the undecorated `plain` and `urls` renderers.

use url::Url;

use hodis::{
    Answer, Diagnostics, Provider, SearchResponse, SearchResult,
    output::{OutputFormat, render},
};

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

fn rendered(response: &SearchResponse, format: OutputFormat) -> String {
    let mut out = Vec::new();
    render(response, format, &mut out).expect("writing to a Vec cannot fail");
    String::from_utf8(out).expect("the renderer emits utf-8")
}

fn oslo() -> SearchResponse {
    response(
        Some(answer(
            "Oslo is the capital of Norway.",
            Some("https://www.britannica.com/place/Oslo"),
        )),
        vec![
            search_result(
                "Oslo - Wikipedia",
                "https://en.wikipedia.org/wiki/Oslo",
                "Oslo is the capital and most populous city of Norway.",
                "wikipedia",
            ),
            search_result("Visit Oslo", "https://www.visitoslo.com/", "", "brave"),
        ],
    )
}

#[test]
fn plain_text_is_the_content_alone() {
    let text = rendered(&oslo(), OutputFormat::Plain);

    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(
        lines,
        [
            "Oslo is the capital of Norway.",
            "https://www.britannica.com/place/Oslo",
            "",
            "Oslo - Wikipedia",
            "https://en.wikipedia.org/wiki/Oslo",
            "Oslo is the capital and most populous city of Norway.",
            "",
            "Visit Oslo",
            "https://www.visitoslo.com/",
        ]
    );
    assert!(
        text.ends_with("visitoslo.com/\n"),
        "the output ends on the last content line, not a blank one"
    );
}

#[test]
fn plain_fields_are_single_lines_without_control_characters() {
    let text = rendered(
        &response(
            None,
            vec![search_result(
                "Split\ntitle",
                "https://example.com/split",
                "First line.\n\nSecond\u{1b}[2J line.",
                "mojeek",
            )],
        ),
        OutputFormat::Plain,
    );

    assert_eq!(
        text,
        "Split title\nhttps://example.com/split\nFirst line. Second[2J line.\n"
    );
}

#[test]
fn urls_lists_each_result_url_and_nothing_else() {
    let text = rendered(&oslo(), OutputFormat::Urls);

    assert_eq!(
        text,
        "https://en.wikipedia.org/wiki/Oslo\nhttps://www.visitoslo.com/\n"
    );
}
