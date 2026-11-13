//! A line of text being typed, with its cursor. Everything counts characters,
//! never bytes, so accents and other scripts edit like plain letters.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Input {
    chars: Vec<char>,
    /// How many characters are before the cursor.
    cursor: usize,
}

impl Input {
    /// A field holding `text`, with the cursor after it.
    pub fn new(text: &str) -> Self {
        let chars: Vec<char> = text.chars().collect();
        Self {
            cursor: chars.len(),
            chars,
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
        self.cursor = at.min(self.chars.len());
    }

    pub fn insert(&mut self, c: char) {
        self.chars.insert(self.cursor, c);
        self.cursor += 1;
    }

    /// Removes the character before the cursor.
    pub fn backspace(&mut self) {
        if self.cursor > 0 {
            self.cursor -= 1;
            self.chars.remove(self.cursor);
        }
    }

    /// Removes the character at the cursor.
    pub fn delete(&mut self) {
        if self.cursor < self.chars.len() {
            self.chars.remove(self.cursor);
        }
    }

    pub fn left(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
    }

    pub fn right(&mut self) {
        self.set_cursor(self.cursor + 1);
    }

    pub fn home(&mut self) {
        self.cursor = 0;
    }

    pub fn end(&mut self) {
        self.cursor = self.chars.len();
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
}
