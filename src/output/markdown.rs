//! Markdown output. Provider text is escaped so a viewer shows it literally:
//! a page title is never meant as markup.

use std::io::{self, Write};

use url::Url;

use super::one_line;
use crate::SearchResponse;

pub(super) fn render(response: &SearchResponse, out: &mut impl Write) -> io::Result<()> {
    if let Some(answer) = &response.answer {
        writeln!(out, "> {}", line_start(&one_line(&answer.content)))?;
        if let Some(url) = &answer.url {
            let text = url.host_str().unwrap_or(url.as_str());
            writeln!(out, ">")?;
            writeln!(out, "> {}", link(text, url))?;
        }
    }

    for (i, result) in (1..).zip(&response.results) {
        if i > 1 || response.answer.is_some() {
            writeln!(out)?;
        }

        let marker = format!("{i}. ");
        writeln!(
            out,
            "{marker}{}",
            link(&one_line(&result.title), &result.url)
        )?;
        let snippet = one_line(&result.snippet);
        if !snippet.is_empty() {
            // Continuation paragraphs belong to the item only when indented
            // to its text, which starts one column later from item 10 on.
            writeln!(out)?;
            writeln!(
                out,
                "{:width$}{}",
                "",
                line_start(&snippet),
                width = marker.len()
            )?;
        }
    }

    Ok(())
}

fn link(text: &str, url: &Url) -> String {
    format!("[{}]({})", inline(text), destination(url))
}

/// For text opening a line, where a leading `#`, `>`, `-`, `+` or `1.` would
/// start a heading, quote or list.
fn line_start(text: &str) -> String {
    let escaped = inline(text);
    let digits = escaped.bytes().take_while(u8::is_ascii_digit).count();
    if digits > 0 && escaped[digits..].starts_with(['.', ')']) {
        format!("{}\\{}", &escaped[..digits], &escaped[digits..])
    } else if escaped.starts_with(['#', '>', '-', '+']) {
        format!("\\{escaped}")
    } else {
        escaped
    }
}

fn inline(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for (i, c) in text.char_indices() {
        if matches!(c, '\\' | '`' | '*' | '_' | '[' | ']' | '<' | '~')
            || (c == '&' && opens_reference(&text[i + 1..]))
        {
            escaped.push('\\');
        }
        escaped.push(c);
    }
    escaped
}

/// Parentheses, spaces and backslashes need the `<...>` destination form, in
/// which only `<`, `>` and `\` take escapes.
fn destination(url: &Url) -> String {
    let url = url.as_str();
    let pointy = url.contains(['(', ')', '<', '>', '\\', ' ']);
    let mut escaped = String::with_capacity(url.len() + 2);
    if pointy {
        escaped.push('<');
    }
    for (i, c) in url.char_indices() {
        if (pointy && matches!(c, '<' | '>' | '\\')) || (c == '&' && opens_reference(&url[i + 1..]))
        {
            escaped.push('\\');
        }
        escaped.push(c);
    }
    if pointy {
        escaped.push('>');
    }
    escaped
}

/// Whether `rest`, following an `&`, would make it a character reference such
/// as `&amp;` or `&#38;`, which viewers decode even inside link targets.
fn opens_reference(rest: &str) -> bool {
    let name = rest
        .bytes()
        .take_while(|b| b.is_ascii_alphanumeric() || *b == b'#')
        .count();
    name > 0 && rest.as_bytes().get(name) == Some(&b';')
}
