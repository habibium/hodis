use anstyle::{AnsiColor, Color, Style};
use std::io::{self, Write};

use crate::{Diagnostics, SearchResponse};

const ANSWER: Style = Style::new().bold();
const SOURCE: Style = Style::new().dimmed();

const TITLE: Style = Style::new()
    .bold()
    .fg_color(Some(Color::Ansi(AnsiColor::BrightMagenta)));
const URL: Style = Style::new()
    .fg_color(Some(Color::Ansi(AnsiColor::Cyan)))
    .underline();

const DIAG_TITLE: Style = Style::new()
    .fg_color(Some(Color::Ansi(AnsiColor::Yellow)))
    .bold()
    .dimmed();
const DIAG_HEADING: Style = Style::new().bold().dimmed();
const DIAG_TEXT: Style = Style::new().dimmed();

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

pub fn render_diagnostics(diagnostics: &Diagnostics, out: &mut impl Write) -> io::Result<()> {
    writeln!(out)?;
    writeln!(out, "{DIAG_TITLE}Diagnostics{DIAG_TITLE:#}")?;
    writeln!(
        out,
        "{DIAG_HEADING}Skipped results: {}{DIAG_HEADING:#}",
        diagnostics.skipped_results
    )?;

    if !diagnostics.unresponsive_engines.is_empty() {
        writeln!(out, "{DIAG_HEADING}Unresponsive engines:{DIAG_HEADING:#}")?;
    }
    for engine in &diagnostics.unresponsive_engines {
        writeln!(
            out,
            "{DIAG_TEXT}[{}] - {}{DIAG_TEXT:#}",
            engine.engine, engine.reason
        )?;
    }

    Ok(())
}
