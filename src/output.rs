use std::io::{self, Write};

use anstyle::{AnsiColor, Color, Style};

use crate::{Diagnostics, SearchResponse};

const ANSWER: Style = Style::new().bold();
const SOURCE: Style = Style::new().dimmed();

const TITLE: Style = Style::new()
    .bold()
    .fg_color(Some(Color::Ansi(AnsiColor::BrightMagenta)));
const URL: Style = Style::new()
    .fg_color(Some(Color::Ansi(AnsiColor::Cyan)))
    .underline();

const WARN: Style = Style::new()
    .bold()
    .fg_color(Some(Color::Ansi(AnsiColor::Yellow)));
const DIAG_TEXT: Style = Style::new().dimmed();

const SETUP: Style = Style::new()
    .bold()
    .fg_color(Some(Color::Ansi(AnsiColor::Green)));
const SETUP_TEXT: Style = Style::new().dimmed();

#[derive(Debug)]
pub enum OutputFormat {
    Text,
    Json,
}

pub fn render(response: &SearchResponse, out: &mut impl Write) -> io::Result<()> {
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

pub fn render_setup_step(message: &str, out: &mut impl Write) -> io::Result<()> {
    writeln!(
        out,
        "{SETUP}setup:{SETUP:#} {SETUP_TEXT}{message}{SETUP_TEXT:#}"
    )
}

pub fn render_diagnostics(diagnostics: &Diagnostics, out: &mut impl Write) -> io::Result<()> {
    if diagnostics.skipped_results == 0 && diagnostics.unresponsive_engines.is_empty() {
        return Ok(());
    }

    writeln!(out)?;

    if !diagnostics.unresponsive_engines.is_empty() {
        write!(
            out,
            "{WARN}warning:{WARN:#} {DIAG_TEXT}unresponsive engines:{DIAG_TEXT:#}"
        )?;
        for (i, engine) in (1..).zip(&diagnostics.unresponsive_engines) {
            let sep = if i > 1 { "," } else { "" };
            write!(
                out,
                "{DIAG_TEXT}{sep} {} ({}){DIAG_TEXT:#}",
                engine.engine, engine.reason
            )?;
        }
        writeln!(out)?;
    }

    if diagnostics.skipped_results > 0 {
        let s = if diagnostics.skipped_results > 1 {
            "s"
        } else {
            ""
        };
        writeln!(
            out,
            "{WARN}warning:{WARN:#} {DIAG_TEXT}skipped {} malformed result{s}{DIAG_TEXT:#}",
            diagnostics.skipped_results
        )?;
    }

    Ok(())
}
