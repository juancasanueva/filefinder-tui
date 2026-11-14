//! A line of text being typed, with its cursor and what is selected.
//! Everything counts characters, never bytes, so accents and other scripts
//! edit like plain letters.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use unicode_width::UnicodeWidthChar;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Input {
    chars: Vec<char>,
    /// How many characters are before the cursor.
    cursor: usize,
    /// Where a selection starts; it runs to the cursor.
    anchor: Option<usize>,
}

impl Input {
    /// A field holding `text`, with the cursor after it.
    pub fn new(text: &str) -> Self {
        let chars: Vec<char> = text.chars().collect();
        Self {
            cursor: chars.len(),
            chars,
            anchor: None,
        }
    }

    pub fn text(&self) -> String {
        self.chars.iter().collect()
    }

    pub fn chars(&self) -> &[char] {
        &self.chars
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    /// Puts the cursor before the character at `at`; past the end is the end.
    pub fn set_cursor(&mut self, at: usize) {
        self.anchor = None;
        self.cursor = at.min(self.chars.len());
    }

    /// Selects the characters from `start` to `end`, the cursor at `end`.
    pub fn select(&mut self, start: usize, end: usize) {
        self.set_cursor(end);
        self.anchor = Some(start.min(self.chars.len()));
    }

    pub fn select_all(&mut self) {
        self.select(0, self.chars.len());
    }

    /// Where the selection starts and ends, when something is selected.
    pub fn selection(&self) -> Option<(usize, usize)> {
        let anchor = self.anchor.filter(|anchor| *anchor != self.cursor)?;
        Some((anchor.min(self.cursor), anchor.max(self.cursor)))
    }

    /// Removes what is selected. False when nothing was.
    fn take_selection(&mut self) -> bool {
        let Some((start, end)) = self.selection() else {
            self.anchor = None;
            return false;
        };
        self.chars.drain(start..end);
        self.set_cursor(start);
        true
    }

    /// Types a character, in place of what is selected.
    pub fn insert(&mut self, c: char) {
        self.take_selection();
        self.chars.insert(self.cursor, c);
        self.cursor += 1;
    }

    /// Removes what is selected, or else the character before the cursor.
    pub fn backspace(&mut self) {
        if !self.take_selection() && self.cursor > 0 {
            self.cursor -= 1;
            self.chars.remove(self.cursor);
        }
    }

    /// Removes what is selected, or else the character at the cursor.
    pub fn delete(&mut self) {
        if !self.take_selection() && self.cursor < self.chars.len() {
            self.chars.remove(self.cursor);
        }
    }

    /// One character left; a selection is left at its start.
    pub fn left(&mut self) {
        match self.selection() {
            Some((start, _)) => self.set_cursor(start),
            None => self.set_cursor(self.cursor.saturating_sub(1)),
        }
    }

    /// One character right; a selection is left at its end.
    pub fn right(&mut self) {
        match self.selection() {
            Some((_, end)) => self.set_cursor(end),
            None => self.set_cursor(self.cursor + 1),
        }
    }

    pub fn home(&mut self) {
        self.set_cursor(0);
    }

    pub fn end(&mut self) {
        self.set_cursor(self.chars.len());
    }

    /// The part of the text that fits in `width` cells with the cursor in
    /// sight, and the cell the cursor is on.
    pub fn window(&self, width: usize) -> (String, u16) {
        let cells = |c: &char| c.width().unwrap_or(0);
        let start = self.first_shown(width);
        let before: usize = self.chars[start..self.cursor].iter().map(cells).sum();
        let mut text = String::new();
        let mut used = 0;
        for c in &self.chars[start..] {
            used += cells(c);
            if used > width {
                break;
            }
            text.push(*c);
        }
        (text, before as u16)
    }

    /// The first character shown in `width` cells, late enough for the
    /// cursor's own cell to be inside.
    pub fn first_shown(&self, width: usize) -> usize {
        let cells = |c: &char| c.width().unwrap_or(0);
        let mut before: usize = self.chars[..self.cursor].iter().map(cells).sum();
        let mut start = 0;
        while before >= width && start < self.cursor {
            before -= cells(&self.chars[start]);
            start += 1;
        }
        start
    }

    /// Applies a key that edits the text. False for any other key, which is
    /// left for whoever owns the field: Enter, Escape, chords.
    pub fn handle_key(&mut self, key: KeyEvent) -> bool {
        let chord = KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER;
        match key.code {
            KeyCode::Char(c) if !key.modifiers.intersects(chord) && !c.is_control() => {
                self.insert(c)
            }
            KeyCode::Backspace => self.backspace(),
            KeyCode::Delete => self.delete(),
            KeyCode::Left => self.left(),
            KeyCode::Right => self.right(),
            KeyCode::Home => self.home(),
            KeyCode::End => self.end(),
            _ => return false,
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn a_new_field_holds_its_text_with_the_cursor_after_it() {
        let input = Input::new("añb");
        assert_eq!(input.text(), "añb");
        assert_eq!(input.cursor(), 3, "characters, not bytes");
        assert_eq!(Input::default().text(), "");
    }

    #[test]
    fn typing_goes_in_at_the_cursor_and_moves_it_along() {
        let mut input = Input::new("ac");
        input.left();
        input.insert('b');
        input.insert('日');
        assert_eq!(input.text(), "ab日c");
        assert_eq!(input.cursor(), 3);
    }

    #[test]
    fn backspace_and_delete_take_one_character_on_either_side() {
        let mut input = Input::new("añ日b");
        input.left();
        input.backspace();
        assert_eq!((input.text().as_str(), input.cursor()), ("añb", 2));
        input.backspace();
        assert_eq!((input.text().as_str(), input.cursor()), ("ab", 1));
        input.delete();
        assert_eq!((input.text().as_str(), input.cursor()), ("a", 1));
        // Nothing is left on that side: both do nothing.
        input.delete();
        input.home();
        input.backspace();
        assert_eq!((input.text().as_str(), input.cursor()), ("a", 0));
    }

    #[test]
    fn left_right_home_and_end_move_the_cursor_and_stop_at_the_ends() {
        let mut input = Input::new("añb");
        input.right();
        assert_eq!(input.cursor(), 3);
        input.left();
        input.left();
        assert_eq!(input.cursor(), 1);
        input.home();
        input.left();
        assert_eq!(input.cursor(), 0);
        input.end();
        assert_eq!(input.cursor(), 3);
        input.set_cursor(1);
        assert_eq!(input.cursor(), 1);
        input.set_cursor(99);
        assert_eq!(input.cursor(), 3, "past the end is the end");
    }

    #[test]
    fn editing_keys_are_taken_and_the_others_are_left_to_the_caller() {
        let mut input = Input::new("ab");
        for code in [
            KeyCode::Home,
            KeyCode::Char('q'),
            KeyCode::Right,
            KeyCode::Delete,
            KeyCode::End,
            KeyCode::Char('!'),
            KeyCode::Left,
            KeyCode::Backspace,
        ] {
            assert!(input.handle_key(key(code)), "{code:?}");
        }
        // "qab" lost its b to Delete, got "!" at the end, then lost its a to Backspace.
        assert_eq!((input.text().as_str(), input.cursor()), ("q!", 1));
        // A shifted letter is still a letter.
        let shifted = KeyEvent::new(KeyCode::Char('Q'), KeyModifiers::SHIFT);
        assert!(input.handle_key(shifted));
        assert_eq!(input.text(), "qQ!");

        let chord = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        for other in [chord, key(KeyCode::Enter), key(KeyCode::Esc)] {
            assert!(!input.handle_key(other), "{other:?}");
        }
        assert_eq!(input.text(), "qQ!");
    }

    #[test]
    fn a_selection_is_replaced_by_typing_and_removed_by_backspace() {
        let mut input = Input::new("report.txt");
        input.select(0, 6);
        assert_eq!(input.selection(), Some((0, 6)));
        assert_eq!(input.cursor(), 6);
        input.insert('n');
        assert_eq!((input.text().as_str(), input.cursor()), ("n.txt", 1));
        assert_eq!(input.selection(), None);

        input.select_all();
        assert_eq!(input.selection(), Some((0, 5)));
        input.backspace();
        assert_eq!((input.text().as_str(), input.cursor()), ("", 0));

        let mut input = Input::new("abcdef");
        input.select(1, 4);
        input.delete();
        assert_eq!(input.text(), "aef");
        // Left and Right leave a selection at its start or its end.
        let mut input = Input::new("abcdef");
        input.select(1, 4);
        input.left();
        assert_eq!((input.cursor(), input.selection()), (1, None));
        input.select(1, 4);
        input.right();
        assert_eq!((input.cursor(), input.selection()), (4, None));
        input.select(1, 4);
        input.home();
        assert_eq!((input.cursor(), input.selection()), (0, None));
    }

    #[test]
    fn a_text_longer_than_its_room_shows_the_part_around_the_cursor() {
        let mut input = Input::new("abcdefghij");
        assert_eq!(input.window(20), ("abcdefghij".to_string(), 10));
        // The cursor after the last letter needs a cell of its own.
        assert_eq!(input.window(6), ("fghij".to_string(), 5));
        input.set_cursor(7);
        assert_eq!(input.window(6), ("cdefgh".to_string(), 5));
        input.set_cursor(3);
        assert_eq!(input.window(6), ("abcdef".to_string(), 3));
        // Wide letters take two cells each.
        let mut wide = Input::new("日本語のファイル");
        assert_eq!(wide.window(7), ("ァイル".to_string(), 6));
        wide.home();
        assert_eq!(wide.window(7), ("日本語".to_string(), 0));
    }
}
