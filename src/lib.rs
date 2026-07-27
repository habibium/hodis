use url::{ParseError, Url};

pub fn build_url(template: &str, query: &str) -> Result<Url, ParseError> {
    let encoded: String = url::form_urlencoded::byte_serialize(query.as_bytes()).collect();
    Url::parse(&template.replace("{query}", &encoded))
}
