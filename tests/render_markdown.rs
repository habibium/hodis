//! Policies of the `markdown` renderer, checked by parsing its output the way
//! a Markdown viewer would: provider text must display exactly as given, never
//! as markup.

use std::mem;

use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};
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
        provider: Provider::SearXNG,
    }
}

fn search_result(title: &str, url: &str, snippet: &str) -> SearchResult {
    SearchResult {
        title: title.to_owned(),
        url: parse_url(url),
        snippet: snippet.to_owned(),
        image: None,
        source: "mojeek".to_owned(),
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

fn markdown(response: &SearchResponse) -> String {
    let mut out = Vec::new();
    render(response, OutputFormat::Markdown, &mut out).expect("writing to a Vec cannot fail");
    String::from_utf8(out).expect("the renderer emits utf-8")
}

fn link(target: &str, text: &str) -> (String, String) {
    (target.to_owned(), text.to_owned())
}

/// What a viewer displays.
#[derive(Debug, Default)]
struct Viewed {
    /// Paragraphs of the answer quote.
    quote: Vec<String>,
    /// Paragraphs of each list item, the linked title first.
    items: Vec<Vec<String>>,
    /// Every link, as its target and text.
    links: Vec<(String, String)>,
}

/// Panics on any markup the renderer does not write itself, such as emphasis
/// or a heading that provider text slipped in.
fn view(markdown: &str) -> Viewed {
    let options = Options::ENABLE_GFM
        | Options::ENABLE_TABLES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_FOOTNOTES;

    let mut viewed = Viewed::default();
    let mut text = String::new();
    let mut link = None;
    let (mut in_quote, mut in_list, mut in_item) = (false, false, false);

    for event in Parser::new_ext(markdown, options) {
        match event {
            Event::Start(Tag::BlockQuote(None)) if !in_quote && !in_list => in_quote = true,
            Event::End(TagEnd::BlockQuote(None)) => in_quote = false,
            Event::Start(Tag::List(Some(1))) if !in_list && !in_quote => in_list = true,
            Event::End(TagEnd::List(true)) => in_list = false,
            Event::Start(Tag::Item) if in_list && !in_item => {
                in_item = true;
                viewed.items.push(Vec::new());
            }
            // A tight list item holds its text without a paragraph around it.
            Event::End(TagEnd::Item) => {
                if !text.is_empty() {
                    let item = viewed.items.last_mut().expect("an item is open");
                    item.push(mem::take(&mut text));
                }
                in_item = false;
            }
            Event::Start(Tag::Paragraph) => {}
            Event::End(TagEnd::Paragraph) => {
                let paragraph = mem::take(&mut text);
                if in_quote {
                    viewed.quote.push(paragraph);
                } else if in_item {
                    let item = viewed.items.last_mut().expect("an item is open");
                    item.push(paragraph);
                } else {
                    panic!("a paragraph escaped the quote and list: {paragraph:?}\n{markdown}");
                }
            }
            Event::Start(Tag::Link { dest_url, .. }) if link.is_none() => {
                link = Some((dest_url.to_string(), text.len()));
            }
            Event::End(TagEnd::Link) => {
                let (target, start) = link.take().expect("a link ends after it starts");
                viewed.links.push((target, text[start..].to_owned()));
            }
            Event::Text(fragment) => text.push_str(&fragment),
            other => panic!("unexpected markup {other:?} in:\n{markdown}"),
        }
    }

    viewed
}

#[test]
fn results_are_a_numbered_list_of_linked_titles() {
    let viewed = view(&markdown(&response(
        None,
        vec![
            search_result("First", "https://example.com/1", "One."),
            search_result("Second", "https://example.com/2", ""),
        ],
    )));

    assert_eq!(viewed.items, [vec!["First", "One."], vec!["Second"]]);
    assert_eq!(
        viewed.links,
        [
            link("https://example.com/1", "First"),
            link("https://example.com/2", "Second"),
        ]
    );
}

#[test]
fn the_answer_is_a_quote_linking_its_source() {
    let viewed = view(&markdown(&response(
        Some(answer(
            "Oslo is the capital of Norway.",
            Some("https://en.wikipedia.org/wiki/Oslo"),
        )),
        vec![search_result(
            "Visit Oslo",
            "https://www.visitoslo.com/",
            "",
        )],
    )));

    assert_eq!(
        viewed.quote,
        ["Oslo is the capital of Norway.", "en.wikipedia.org"]
    );
    assert_eq!(
        viewed.links,
        [
            link("https://en.wikipedia.org/wiki/Oslo", "en.wikipedia.org"),
            link("https://www.visitoslo.com/", "Visit Oslo"),
        ]
    );
    assert_eq!(viewed.items, [vec!["Visit Oslo"]]);
}

#[test]
fn markup_in_provider_text_displays_literally() {
    let answer_text = "_emphasis_ and **strong** in an answer";
    let title = r"*args, _private_, `code`, [link](x), <b>tag</b>, ~old~, \escape, AT&T &amp; co";
    let snippet = "![image](x) <https://autolink.example> &#35; 2 * 3 * 4 [^note]";

    let viewed = view(&markdown(&response(
        Some(answer(answer_text, None)),
        vec![search_result(title, "https://example.com/markup", snippet)],
    )));

    assert_eq!(viewed.quote, [answer_text]);
    assert_eq!(viewed.items, [vec![title, snippet]]);
}

#[test]
fn text_opening_a_line_never_opens_a_block() {
    for text in [
        "# Heading",
        "###### Deep heading",
        "> Quote",
        "- Bullet",
        "+ Bullet",
        "* Bullet",
        "1. Ordered",
        "2) Ordered",
        "---",
        "***",
        "___",
        "```rust",
        "~~~",
        "<div>",
        "[label]: https://example.com",
        "| a | b |",
        "    Indented",
    ] {
        let viewed = view(&markdown(&response(
            Some(answer(text, None)),
            vec![search_result("Title", "https://example.com/", text)],
        )));

        assert_eq!(viewed.quote, [text.trim()], "{text:?} as an answer");
        assert_eq!(
            viewed.items,
            [vec!["Title", text.trim()]],
            "{text:?} as a snippet"
        );
    }
}

#[test]
fn link_targets_arrive_intact() {
    for raw in [
        "https://en.wikipedia.org/wiki/Rust_(programming_language)",
        "https://example.com/unbalanced)paren",
        "https://example.com/search?q=fish&amp;chips",
        r"https://example.com/search?path=C:\dir\(x)",
        "https://example.com/~user/a_b*c[d]",
        "urn:example:with space",
    ] {
        let url = parse_url(raw);
        let viewed = view(&markdown(&response(
            None,
            vec![search_result("Title", raw, "")],
        )));

        assert_eq!(viewed.links, [link(url.as_str(), "Title")], "{raw}");
    }
}

#[test]
fn snippets_stay_in_items_numbered_past_nine() {
    let results = (1..=10)
        .map(|i| {
            search_result(
                &format!("Title {i}"),
                &format!("https://example.com/{i}"),
                &format!("Snippet {i}."),
            )
        })
        .collect();

    let viewed = view(&markdown(&response(None, results)));

    assert_eq!(viewed.items.len(), 10);
    assert_eq!(viewed.items[9], ["Title 10", "Snippet 10."]);
}

#[test]
fn control_characters_are_dropped() {
    let viewed = view(&markdown(&response(
        None,
        vec![search_result(
            "Title\u{1b}[2J",
            "https://example.com/",
            "Snippet\u{7}",
        )],
    )));

    assert_eq!(viewed.items, [vec!["Title[2J", "Snippet"]]);
}
