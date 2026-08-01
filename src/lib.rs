use serde::Serialize;
use url::Url;

#[derive(Debug, Serialize, Clone, PartialEq)]
pub enum Provider {
    DuckDuckGo,
    SearXNG,
}

#[derive(Debug, Serialize, Clone, PartialEq)]
pub struct SearchResponse {
    pub query: String,
    pub answer: Option<Answer>,
    pub results: Vec<SearchResult>,
    pub provider: Provider,
}

#[derive(Debug, Serialize, Clone, PartialEq)]
pub struct SearchResult {
    pub title: String,
    pub url: Url,
    pub snippet: String,
    pub image: Option<Image>,
    pub source: String,
}

#[derive(Debug, Serialize, Clone, PartialEq)]
pub struct Image {
    pub src: Url,
    pub alt: String,
}

#[derive(Debug, Serialize, Clone, PartialEq)]
pub struct Answer {
    pub question: Option<String>,
    pub url: Option<Url>,
    pub content: String,
}
