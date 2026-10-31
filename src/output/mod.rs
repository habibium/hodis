mod markdown;

use std::io::{self, Write};

use anstream::ColorChoice;
use anstyle::{AnsiColor, Color, Style};
use clap::ValueEnum;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{Diagnostics, SearchResponse};

const SCHEMA_VERSION: &str = "1.0.0";

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OutputFormat {
    /// Styled text for reading in a terminal
    Pretty,
    /// A versioned JSON document for scripts and agents
    Json,
    /// Markdown for notes and documents
    Markdown,
    /// Text without colors, numbering or engine names
    Plain,
    /// Result URLs, one per line
    Urls,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ColorWhen {
    Auto,
    Always,
    Never,
}

impl From<ColorWhen> for ColorChoice {
    fn from(when: ColorWhen) -> Self {
        match when {
            ColorWhen::Auto => Self::Auto,
            ColorWhen::Always => Self::Always,
            ColorWhen::Never => Self::Never,
        }
    }
}

/// The `json` format's document: a [`SearchResponse`], or a reference to one,
/// stamped with the version of its schema.
#[derive(Debug, Serialize, JsonSchema)]
pub struct Versioned<R> {
    /// A major change removes, renames or retypes a field; a minor one adds one.
    schema_version: &'static str,
    #[serde(flatten)]
    response: R,
}

impl<R> Versioned<R> {
    pub fn new(response: R) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            response,
        }
    }
}

pub fn render(
    response: &SearchResponse,
    format: OutputFormat,
    out: &mut impl Write,
) -> io::Result<()> {
    match format {
        OutputFormat::Pretty => render_pretty(response, out),
        OutputFormat::Json => render_json(response, out),
        OutputFormat::Markdown => markdown::render(response, out),
        OutputFormat::Plain => render_plain(response, out),
        OutputFormat::Urls => render_urls(response, out),
    }
}

fn render_pretty(response: &SearchResponse, out: &mut impl Write) -> io::Result<()> {
    if let Some(answer) = &response.answer {
        let answer_text = one_line(&answer.content);
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

        write!(out, "{TITLE}{i}. {}{TITLE:#}", one_line(&result.title))?;
        let source = one_line(&result.source);
        if !source.is_empty() {
            write!(out, " {SOURCE}[{source}]{SOURCE:#}")?;
        }
        writeln!(out)?;
        writeln!(out, "{URL}{}{URL:#}", result.url)?;
        let snippet = one_line(&result.snippet);
        if !snippet.is_empty() {
            writeln!(out, "{snippet}")?;
        }
    }

    Ok(())
}

fn render_json(response: &SearchResponse, out: &mut impl Write) -> io::Result<()> {
    serde_json::to_writer_pretty(&mut *out, &Versioned::new(response))?;
    writeln!(out)
}

fn render_plain(response: &SearchResponse, out: &mut impl Write) -> io::Result<()> {
    if let Some(answer) = &response.answer {
        writeln!(out, "{}", one_line(&answer.content))?;
        if let Some(url) = &answer.url {
            writeln!(out, "{url}")?;
        }
    }

    for (i, result) in response.results.iter().enumerate() {
        if i > 0 || response.answer.is_some() {
            writeln!(out)?;
        }

        writeln!(out, "{}", one_line(&result.title))?;
        writeln!(out, "{}", result.url)?;
        let snippet = one_line(&result.snippet);
        if !snippet.is_empty() {
            writeln!(out, "{snippet}")?;
        }
    }

    Ok(())
}

fn render_urls(response: &SearchResponse, out: &mut impl Write) -> io::Result<()> {
    for result in &response.results {
        writeln!(out, "{}", result.url)?;
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
                one_line(&engine.engine),
                one_line(&engine.reason)
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

pub fn render_warning(message: &str, out: &mut impl Write) -> io::Result<()> {
    writeln!(
        out,
        "{WARN}warning:{WARN:#} {DIAG_TEXT}{message}{DIAG_TEXT:#}"
    )
}

/// Provider text as a single line. Control characters are dropped because a
/// terminal acts on them, e.g. on escape sequences planted in a page title.
fn one_line(text: &str) -> String {
    text.split_whitespace()
        .map(|word| word.chars().filter(|c| !c.is_control()).collect::<String>())
        .filter(|word| !word.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}
