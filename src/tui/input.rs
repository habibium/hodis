//! The one-line field the query is typed into.

use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

/// Text and a cursor. The cursor stays on a grapheme boundary, so moving or
/// deleting never splits what the terminal draws as one character, such as an
/// emoji or a letter with a combining accent.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct Input {
    text: String,
    /// A byte offset into `text`.
    cursor: usize,
}

impl Input {
    pub(crate) fn text(&self) -> &str {
        &self.text
    }

    /// Replaces the text, leaving the cursor at its end.
    pub(crate) fn set(&mut self, text: &str) {
        text.clone_into(&mut self.text);
        self.cursor = self.text.len();
    }

    pub(crate) fn insert(&mut self, c: char) {
        if !c.is_control() {
            self.text.insert(self.cursor, c);
            self.cursor += c.len_utf8();
        }
    }

    /// Inserts pasted text, with line breaks and tabs as spaces: a query is
    /// one line.
    pub(crate) fn paste(&mut self, text: &str) {
        let line: String = text
            .chars()
            .filter_map(|c| match c {
                '\n' | '\r' | '\t' => Some(' '),
                c if c.is_control() => None,
                c => Some(c),
            })
            .collect();
        self.text.insert_str(self.cursor, &line);
        self.cursor += line.len();
    }

    pub(crate) fn backspace(&mut self) {
        let start = self.previous_boundary();
        self.text.replace_range(start..self.cursor, "");
        self.cursor = start;
    }

    pub(crate) fn delete(&mut self) {
        let end = self.next_boundary();
        self.text.replace_range(self.cursor..end, "");
    }

    pub(crate) fn left(&mut self) {
        self.cursor = self.previous_boundary();
    }

    pub(crate) fn right(&mut self) {
        self.cursor = self.next_boundary();
    }

    pub(crate) fn home(&mut self) {
        self.cursor = 0;
    }

    pub(crate) fn end(&mut self) {
        self.cursor = self.text.len();
    }

    pub(crate) fn word_left(&mut self) {
        self.cursor = self.previous_word_start();
    }

    pub(crate) fn word_right(&mut self) {
        let rest = &self.text[self.cursor..];
        let blanks = rest.len() - rest.trim_start().len();
        let word = rest[blanks..]
            .find(char::is_whitespace)
            .unwrap_or(rest.len() - blanks);
        self.cursor += blanks + word;
    }

    /// Deletes back to the start of the word before the cursor, as Ctrl-W does
    /// in a shell.
    pub(crate) fn delete_word(&mut self) {
        let start = self.previous_word_start();
        self.text.replace_range(start..self.cursor, "");
        self.cursor = start;
    }

    pub(crate) fn delete_to_start(&mut self) {
        self.text.replace_range(..self.cursor, "");
        self.cursor = 0;
    }

    pub(crate) fn delete_to_end(&mut self) {
        self.text.truncate(self.cursor);
    }

    /// The stretch of text to draw in `width` columns, and the cursor's column
    /// in it. The text scrolls only once the cursor would leave the field.
    pub(crate) fn window(&self, width: usize) -> (&str, usize) {
        // The last column is kept for the cursor.
        let room = width.saturating_sub(1);
        let mut start = 0;
        if self.text[..self.cursor].width() > room {
            let mut used = 0;
            start = self.cursor;
            for (offset, grapheme) in self.text[..self.cursor].grapheme_indices(true).rev() {
                used += grapheme.width();
                if used > room {
                    break;
                }
                start = offset;
            }
        }

        let mut end = start;
        let mut used = 0;
        for (offset, grapheme) in self.text[start..].grapheme_indices(true) {
            used += grapheme.width();
            if used > width {
                break;
            }
            end = start + offset + grapheme.len();
        }
        (
            &self.text[start..end],
            self.text[start..self.cursor].width(),
        )
    }

    fn previous_boundary(&self) -> usize {
        self.text[..self.cursor]
            .grapheme_indices(true)
            .next_back()
            .map_or(0, |(offset, _)| offset)
    }

    fn next_boundary(&self) -> usize {
        self.text[self.cursor..]
            .graphemes(true)
            .next()
            .map_or(self.cursor, |grapheme| self.cursor + grapheme.len())
    }

    fn previous_word_start(&self) -> usize {
        self.text[..self.cursor]
            .trim_end()
            .char_indices()
            .rfind(|(_, c)| c.is_whitespace())
            .map_or(0, |(offset, space)| offset + space.len_utf8())
    }
}

#[cfg(test)]
mod tests {
    use super::Input;

    fn typed(text: &str) -> Input {
        let mut input = Input::default();
        for c in text.chars() {
            input.insert(c);
        }
        input
    }

    /// The text with `|` where the cursor is.
    fn shown(input: &Input) -> String {
        let mut text = input.text.clone();
        text.insert(input.cursor, '|');
        text
    }

    #[test]
    fn typing_inserts_at_the_cursor() {
        let mut input = typed("rust");
        input.home();
        input.insert('a');
        input.right();
        input.insert('!');
        assert_eq!(shown(&input), "ar!|ust");
    }

    #[test]
    fn control_characters_are_not_typed() {
        let mut input = typed("a\u{1b}b");
        input.insert('\n');
        assert_eq!(input.text(), "ab");
    }

    #[test]
    fn moving_and_deleting_keep_graphemes_whole() {
        // A family emoji is one grapheme made of seven code points, and the é
        // here is an e followed by a combining accent.
        let mut input = typed("a👨‍👩‍👧‍👦e\u{301}");
        input.left();
        assert_eq!(shown(&input), "a👨‍👩‍👧‍👦|e\u{301}");
        input.backspace();
        assert_eq!(shown(&input), "a|e\u{301}");
        input.delete();
        assert_eq!(shown(&input), "a|");
    }

    #[test]
    fn edges_do_not_move_the_cursor_further() {
        let mut input = typed("ab");
        input.right();
        input.delete();
        assert_eq!(shown(&input), "ab|");
        input.home();
        input.left();
        input.backspace();
        assert_eq!(shown(&input), "|ab");
    }

    #[test]
    fn words_are_jumped_and_deleted_whole() {
        let mut input = typed("dns  lookup tool ");
        input.word_left();
        assert_eq!(shown(&input), "dns  lookup |tool ");
        input.word_left();
        input.word_left();
        assert_eq!(shown(&input), "|dns  lookup tool ");
        input.word_right();
        assert_eq!(shown(&input), "dns|  lookup tool ");
        input.word_right();
        assert_eq!(shown(&input), "dns  lookup| tool ");

        let mut input = typed("dns  lookup tool ");
        input.delete_word();
        assert_eq!(shown(&input), "dns  lookup |");
        input.delete_word();
        input.delete_word();
        assert_eq!(shown(&input), "|");
    }

    #[test]
    fn a_line_is_cut_on_either_side_of_the_cursor() {
        let mut input = typed("capital of norway");
        input.word_left();
        let mut before = input.clone();
        before.delete_to_start();
        assert_eq!(shown(&before), "|norway");
        input.delete_to_end();
        assert_eq!(shown(&input), "capital of |");
    }

    #[test]
    fn pasted_line_breaks_become_spaces() {
        let mut input = typed("ab");
        input.left();
        input.paste("one\ntwo\tthree\u{7}");
        assert_eq!(shown(&input), "aone two three|b");
    }

    #[test]
    fn set_replaces_the_text_and_moves_to_its_end() {
        let mut input = typed("old");
        input.home();
        input.set("new query");
        assert_eq!(shown(&input), "new query|");
    }

    #[test]
    fn a_short_line_is_drawn_whole() {
        let input = typed("rust");
        assert_eq!(input.window(10), ("rust", 4));
    }

    #[test]
    fn a_long_line_scrolls_to_keep_the_cursor_in_view() {
        let mut input = typed("abcdefghij");
        assert_eq!(input.window(5), ("ghij", 4));
        input.home();
        assert_eq!(input.window(5), ("abcde", 0));
        // Once the start is back in view, the field shows it again.
        for _ in 0..4 {
            input.right();
        }
        assert_eq!(input.window(5), ("abcde", 4));
        input.right();
        assert_eq!(input.window(5), ("bcdef", 4));
    }

    #[test]
    fn wide_characters_count_two_columns() {
        let input = typed("日本語の検索");
        // Each character takes two columns, and the cursor needs one more.
        assert_eq!(input.window(7), ("の検索", 6));
        assert_eq!(input.window(13), ("日本語の検索", 12));
    }

    #[test]
    fn no_room_draws_nothing() {
        let input = typed("rust");
        assert_eq!(input.window(0), ("", 0));
    }
}
