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
            writeln!(out, "{SOURCE}{url}{SOURCE:#}")?;
        }
        writeln!(out)?;
    }

    for (i, result) in (1..).zip(&response.results) {
        if i > 1 {
            writeln!(out)?;
        }

        write!(out, "{TITLE}{i}. {}{TITLE:#}", result.title)?;
        if !result.source.is_empty() {
            write!(out, " {SOURCE}[{}]{SOURCE:#}", result.source)?;
        }
        writeln!(out)?;
        writeln!(out, "{URL}{}{URL:#}", result.url)?;
        if !result.snippet.is_empty() {
            writeln!(out, "{}", result.snippet)?;
        }
    }

    Ok(())
}
