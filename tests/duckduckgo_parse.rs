//! Reading DuckDuckGo's results pages, as captured from the live site.

use url::Url;

use hodis::{Provider, providers::duckduckgo::DuckDuckGoProvider};

const HEALTHY_PAGE: &str = include_str!("fixtures/duckduckgo_healthy.html");
const CHALLENGE_PAGE: &str = include_str!("fixtures/duckduckgo_challenge.html");

fn provider() -> DuckDuckGoProvider {
    DuckDuckGoProvider {
        base_url: Url::parse("https://html.duckduckgo.com/").expect("the url is valid"),
    }
}

#[test]
fn a_results_page_gives_its_results_in_order() {
    let response = provider()
        .parse_body("rust borrow checker", HEALTHY_PAGE)
        .expect("the page parses");

    assert_eq!(response.results.len(), 10);
    let first = &response.results[0];
    assert_eq!(
        first.title,
        "Understanding the Rust borrow checker - LogRocket Blog"
    );
    assert_eq!(
        first.url.as_str(),
        "https://blog.logrocket.com/introducing-rust-borrow-checker/"
    );
    // Entities are decoded and the bold markup around matched words is gone.
    assert_eq!(
        first.snippet,
        "Explore Rust's borrow checker and learn how it enhances code safety, prevents errors, and the principles behind it."
    );
    assert_eq!(first.source, "duckduckgo");
    assert!(
        response
            .results
            .iter()
            .all(|result| result.provider == Provider::DuckDuckGo)
    );
    assert_eq!(response.providers, [Provider::DuckDuckGo]);
    assert_eq!(response.answer, None);
    assert_eq!(response.diagnostics.skipped_results, 0);
    assert_eq!(response.query, "rust borrow checker");
}

#[test]
fn a_captcha_is_an_error_that_says_so() {
    let error = provider()
        .parse_body("rust borrow checker", CHALLENGE_PAGE)
        .expect_err("a challenge page holds no results");

    let message = format!("{error:#}");
    assert!(message.contains("CAPTCHA"), "{message}");
}

#[test]
fn results_without_a_title_or_a_valid_link_are_skipped_and_counted() {
    let page = r#"
        <div id="links">
          <div class="result web-result"><h2><a href="https://example.com/kept">Kept</a></h2></div>
          <div class="result web-result"><h2><a href="https://example.com/untitled"> </a></h2></div>
          <div class="result web-result"><h2><a href="/relative">Relative</a></h2></div>
          <div class="result web-result"><h2>No link</h2></div>
          <div class="result result--ad web-result"><h2><a href="https://ads.example.com/">Ad</a></h2></div>
        </div>"#;

    let response = provider().parse_body("q", page).expect("the page parses");

    assert_eq!(response.results.len(), 1);
    assert_eq!(response.results[0].title, "Kept");
    assert_eq!(
        response.diagnostics.skipped_results, 3,
        "the ad is not counted"
    );
}

#[test]
fn a_page_without_results_is_empty_rather_than_an_error() {
    let response = provider()
        .parse_body("q", "<html><body><div id=\"links\"></div></body></html>")
        .expect("the page parses");
    assert_eq!(response.results.len(), 0);
}
