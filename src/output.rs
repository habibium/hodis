use anstyle::{AnsiColor, Color, Style};
use std::io::{self, Write};

use crate::SearchResponse;

const ANSWER: Style = Style::new().bold();
const SOURCE: Style = Style::new().dimmed();

const TITLE: Style = Style::new()
    .bold()
    .fg_color(Some(Color::Ansi(AnsiColor::BrightMagenta)));
const URL: Style = Style::new()
    .fg_color(Some(Color::Ansi(AnsiColor::Cyan)))
    .underline();
const SNIPPET: Style = Style::new();

#[derive(Debug)]
pub enum OutputFormat {
    Text,
    Json,
}

pub fn render(response: &SearchResponse, out: &mut impl Write) -> io::Result<()> {
    writeln!(out)?;

    if let Some(answer) = &response.answer {
        let answer_text = &answer.content;
        writeln!(out, "{ANSWER}{answer_text}{ANSWER:#}")?;
        if let Some(url) = &answer.url {
            let url = url.as_str();
            writeln!(out, "{SOURCE}{url}{SOURCE:#}")?;
        }
        writeln!(out)?;
    }

    for (i, result) in (1..).zip(response.results.iter()) {
        let (title, url, snippet, source) = (
            &result.title,
            &result.url.to_string(),
            &result.snippet,
            &result.source,
        );
        writeln!(out, "{TITLE}{i}. {title}{TITLE:#}")?;
        writeln!(out, "{URL}{url}{URL:#}")?;
        if !snippet.is_empty() {
            writeln!(out, "{SNIPPET}{snippet}{SNIPPET:#}")?;
        }
        if !source.is_empty() {
            writeln!(out, "{SOURCE}{source}{SOURCE:#}")?;
        }

        writeln!(out)?;
        writeln!(out)?;
    }

    Ok(())
}
