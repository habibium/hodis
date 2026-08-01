use serde::Serialize;
use url::Url;

// Provider / Suppliers
#[derive(Debug, Serialize, Clone, PartialEq)]
pub enum Provider {
    DuckDuckGo,
    SearXNG,
}

// What is SearchResponse? How does it differ from SearchResult? Is it related to provider or consumer?
#[derive(Debug, Serialize, Clone, PartialEq)]
pub struct SearchResponse;

// Consumers
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
    pub src: String,
    pub alt: String,
}

#[derive(Debug, Serialize, Clone, PartialEq)]
pub struct Answer {
    pub question: Option<String>,
    pub url: Url,
    pub content: String,
}
