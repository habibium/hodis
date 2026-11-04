//! The questions the setup wizard asks, and how a terminal asks them.

use std::io::{self, Write};
use std::process;

use anstream::AutoStream;
use anstyle::{AnsiColor, Color, Style};
use crossterm::cursor::{MoveToColumn, MoveUp};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{self, Clear, ClearType};

use crate::output::fit;

const QUESTION: Style = Style::new().bold();
const SELECTED: Style = Style::new()
    .bold()
    .fg_color(Some(Color::Ansi(AnsiColor::Cyan)));
const HINT: Style = Style::new().dimmed();

/// Exit status of a program ended by Ctrl-C.
const INTERRUPTED: i32 = 130;

/// One of the answers to a question.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Choice {
    pub label: String,
    pub hint: String,
}

impl Choice {
    #[must_use]
    pub fn new(label: impl Into<String>, hint: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            hint: hint.into(),
        }
    }
}

/// Asks the user things. `None` means they went back.
pub trait Prompts {
    /// Which of `choices`, by position.
    fn choose(&mut self, question: &str, choices: &[Choice]) -> io::Result<Option<usize>>;
    fn confirm(&mut self, question: &str) -> io::Result<bool>;
    fn text(&mut self, question: &str) -> io::Result<Option<String>>;
    /// Like `text`, without showing what is typed.
    fn secret(&mut self, question: &str) -> io::Result<Option<String>>;
}

/// What a key does to a menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Key {
    Moved,
    Chosen(usize),
    Back,
    Interrupted,
    Ignored,
}

/// The state of a menu: which of `len` choices is selected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Menu {
    selected: usize,
    len: usize,
}

impl Menu {
    fn press(&mut self, key: KeyEvent) -> Key {
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            return if key.code == KeyCode::Char('c') {
                Key::Interrupted
            } else {
                Key::Ignored
            };
        }
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                self.selected = self.selected.checked_sub(1).unwrap_or(self.len - 1);
                Key::Moved
            }
            KeyCode::Down | KeyCode::Char('j') | KeyCode::Tab => {
                self.selected = (self.selected + 1) % self.len;
                Key::Moved
            }
            KeyCode::Enter => Key::Chosen(self.selected),
            KeyCode::Esc | KeyCode::Char('q') => Key::Back,
            KeyCode::Char(digit) => match digit.to_digit(10).and_then(|d| d.checked_sub(1)) {
                Some(index) if (index as usize) < self.len => Key::Chosen(index as usize),
                _ => Key::Ignored,
            },
            _ => Key::Ignored,
        }
    }
}

/// How a yes-or-no question reads a key: `None` until it is answered.
fn yes_or_no(key: KeyEvent) -> Option<Key> {
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        return (key.code == KeyCode::Char('c')).then_some(Key::Interrupted);
    }
    match key.code {
        KeyCode::Enter | KeyCode::Char('y' | 'Y') => Some(Key::Chosen(1)),
        KeyCode::Esc | KeyCode::Char('n' | 'N') => Some(Key::Chosen(0)),
        _ => None,
    }
}

/// Puts the terminal in raw mode for as long as it lives.
struct RawMode;

impl RawMode {
    fn enter() -> io::Result<Self> {
        terminal::enable_raw_mode()?;
        Ok(Self)
    }
}

impl Drop for RawMode {
    fn drop(&mut self) {
        let _ = terminal::disable_raw_mode();
    }
}

/// The next key pressed. Windows reports releases too, which must not count.
fn next_key() -> io::Result<Option<KeyEvent>> {
    Ok(match event::read()? {
        Event::Key(key) if key.kind != KeyEventKind::Release => Some(key),
        _ => None,
    })
}

/// Asks on the terminal, through standard error.
#[derive(Debug)]
pub struct TerminalPrompts {
    /// For text. With colors off it removes every escape sequence, cursor
    /// movement included, so those go to standard error directly.
    out: AutoStream<io::Stderr>,
}

impl TerminalPrompts {
    #[must_use]
    pub fn new() -> Self {
        Self {
            out: AutoStream::auto(io::stderr()),
        }
    }

    fn draw(&mut self, question: &str, choices: &[Choice], selected: usize) -> io::Result<()> {
        let width = usize::from(terminal::size().map_or(80, |(columns, _)| columns));
        let room = width.saturating_sub(1);
        let name_width = choices
            .iter()
            .map(|choice| choice.label.chars().count())
            .max()
            .unwrap_or(0);

        execute!(
            io::stderr(),
            MoveToColumn(0),
            Clear(ClearType::FromCursorDown)
        )?;
        write!(
            self.out,
            "{QUESTION}{}{QUESTION:#}\r\n",
            fit(question, room)
        )?;
        for (index, choice) in choices.iter().enumerate() {
            let label = format!("{:name_width$}", choice.label);
            let line = fit(&format!("  > {label}  {}", choice.hint), room);
            // Styled after fitting, so that the escapes do not count as width.
            let (marker, rest) = line.split_at(4);
            let rest = rest.strip_prefix(label.as_str()).unwrap_or(rest);
            if index == selected {
                write!(self.out, "{SELECTED}{marker}{label}{SELECTED:#}")?;
            } else {
                write!(self.out, "    {label}")?;
            }
            write!(self.out, "{HINT}{rest}{HINT:#}\r\n")?;
        }
        let keys = "up/down to move, enter to choose, esc to go back";
        write!(self.out, "{HINT}{}{HINT:#}\r\n", fit(keys, room))?;

        self.out.flush()?;
        let drawn = u16::try_from(choices.len() + 2).unwrap_or(u16::MAX);
        execute!(io::stderr(), MoveUp(drawn))
    }

    /// Replaces a question with the answer it got.
    fn answered(&mut self, question: &str, answer: &str) -> io::Result<()> {
        execute!(
            io::stderr(),
            MoveToColumn(0),
            Clear(ClearType::FromCursorDown)
        )?;
        write!(self.out, "{QUESTION}{question}{QUESTION:#} {answer}\r\n")?;
        self.out.flush()
    }
}

impl Default for TerminalPrompts {
    fn default() -> Self {
        Self::new()
    }
}

impl Prompts for TerminalPrompts {
    fn choose(&mut self, question: &str, choices: &[Choice]) -> io::Result<Option<usize>> {
        let mut menu = Menu {
            selected: 0,
            len: choices.len(),
        };
        let raw = RawMode::enter()?;
        let answer = loop {
            self.draw(question, choices, menu.selected)?;
            let Some(key) = next_key()? else { continue };
            match menu.press(key) {
                Key::Chosen(index) => break Some(index),
                Key::Back => break None,
                Key::Interrupted => {
                    drop(raw);
                    let _ = execute!(io::stderr(), Clear(ClearType::FromCursorDown));
                    process::exit(INTERRUPTED);
                }
                Key::Moved | Key::Ignored => {}
            }
        };
        match answer {
            Some(index) => self.answered(question, &choices[index].label)?,
            None => self.answered(question, "(back)")?,
        }
        drop(raw);
        Ok(answer)
    }

    fn confirm(&mut self, question: &str) -> io::Result<bool> {
        write!(self.out, "{QUESTION}{question}{QUESTION:#} [Y/n] ")?;
        self.out.flush()?;
        let raw = RawMode::enter()?;
        let answer = loop {
            let Some(key) = next_key()? else { continue };
            match yes_or_no(key) {
                Some(Key::Chosen(answer)) => break answer == 1,
                Some(Key::Interrupted) => {
                    drop(raw);
                    process::exit(INTERRUPTED);
                }
                _ => {}
            }
        };
        write!(self.out, "{}\r\n", if answer { "yes" } else { "no" })?;
        self.out.flush()?;
        drop(raw);
        Ok(answer)
    }

    fn text(&mut self, question: &str) -> io::Result<Option<String>> {
        write!(self.out, "{QUESTION}{question}{QUESTION:#} ")?;
        self.out.flush()?;
        let mut line = String::new();
        io::stdin().read_line(&mut line)?;
        let line = line.trim();
        Ok((!line.is_empty()).then(|| line.to_owned()))
    }

    fn secret(&mut self, question: &str) -> io::Result<Option<String>> {
        let line = rpassword::prompt_password(format!("{question} "))?;
        let line = line.trim();
        Ok((!line.is_empty()).then(|| line.to_owned()))
    }
}

#[cfg(test)]
mod tests {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    use super::{Key, Menu, yes_or_no};

    fn press(menu: &mut Menu, code: KeyCode) -> Key {
        menu.press(KeyEvent::new(code, KeyModifiers::NONE))
    }

    fn menu() -> Menu {
        Menu {
            selected: 0,
            len: 3,
        }
    }

    #[test]
    fn the_arrows_move_and_wrap_around() {
        let mut menu = menu();
        assert_eq!(press(&mut menu, KeyCode::Up), Key::Moved);
        assert_eq!(menu.selected, 2);
        assert_eq!(press(&mut menu, KeyCode::Down), Key::Moved);
        assert_eq!(menu.selected, 0);
        press(&mut menu, KeyCode::Char('j'));
        assert_eq!(menu.selected, 1);
        press(&mut menu, KeyCode::Char('k'));
        assert_eq!(menu.selected, 0);
    }

    #[test]
    fn enter_chooses_what_is_selected() {
        let mut menu = menu();
        press(&mut menu, KeyCode::Down);
        assert_eq!(press(&mut menu, KeyCode::Enter), Key::Chosen(1));
    }

    #[test]
    fn a_number_chooses_directly_if_there_is_such_a_choice() {
        assert_eq!(press(&mut menu(), KeyCode::Char('3')), Key::Chosen(2));
        assert_eq!(press(&mut menu(), KeyCode::Char('4')), Key::Ignored);
        assert_eq!(press(&mut menu(), KeyCode::Char('0')), Key::Ignored);
    }

    #[test]
    fn esc_goes_back_and_ctrl_c_interrupts() {
        assert_eq!(press(&mut menu(), KeyCode::Esc), Key::Back);
        assert_eq!(
            menu().press(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
            Key::Interrupted
        );
    }

    #[test]
    fn a_yes_or_no_defaults_to_yes_on_enter() {
        let key = |code| KeyEvent::new(code, KeyModifiers::NONE);
        assert_eq!(yes_or_no(key(KeyCode::Enter)), Some(Key::Chosen(1)));
        assert_eq!(yes_or_no(key(KeyCode::Char('n'))), Some(Key::Chosen(0)));
        assert_eq!(yes_or_no(key(KeyCode::Esc)), Some(Key::Chosen(0)));
        assert_eq!(yes_or_no(key(KeyCode::Char('x'))), None);
    }
}
