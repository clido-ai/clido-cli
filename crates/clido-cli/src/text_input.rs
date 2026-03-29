//! Reusable single-line text input with cursor, word operations, history, and paste.
//!
//! Used by: main chat input, profile overlay fields, permission feedback,
//! settings editor, and any future text input context.

/// Converts a character index to a byte offset in a UTF-8 string.
fn char_to_byte(s: &str, char_idx: usize) -> usize {
    s.char_indices()
        .nth(char_idx)
        .map(|(b, _)| b)
        .unwrap_or(s.len())
}

/// A single-line text input buffer with cursor, editing operations,
/// optional masking (for API keys), and optional history navigation.
#[derive(Debug, Clone, Default)]
pub struct TextInput {
    pub text: String,
    /// Cursor position in *characters* (not bytes).
    pub cursor: usize,
    /// When set, `display_text()` returns this char repeated instead of the real text.
    pub mask: Option<char>,
    pub placeholder: String,
    /// Previously submitted values (oldest first). Optional — only used for chat input.
    pub history: Vec<String>,
    /// Index into `history` while navigating (None = editing current draft).
    pub history_idx: Option<usize>,
    /// Saved draft text when user enters history navigation.
    pub history_draft: String,
    /// Horizontal scroll offset in characters (for inputs wider than the viewport).
    pub scroll: usize,
}

impl TextInput {
    /// Create a new empty text input.
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a text input with placeholder text.
    pub fn with_placeholder(placeholder: impl Into<String>) -> Self {
        Self {
            placeholder: placeholder.into(),
            ..Self::default()
        }
    }

    /// Create a masked text input (e.g. for API keys).
    pub fn masked(mask_char: char) -> Self {
        Self {
            mask: Some(mask_char),
            ..Self::default()
        }
    }

    /// Number of characters in the text.
    pub fn char_count(&self) -> usize {
        self.text.chars().count()
    }

    /// Whether the text is empty.
    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    /// The text to display — masked if `self.mask` is set.
    pub fn display_text(&self) -> String {
        match self.mask {
            Some(c) => c.to_string().repeat(self.char_count()),
            None => self.text.clone(),
        }
    }

    /// Set the text and move cursor to the end.
    pub fn set_text(&mut self, text: impl Into<String>) {
        self.text = text.into();
        self.cursor = self.char_count();
        self.history_idx = None;
    }

    /// Clear text and reset cursor.
    pub fn clear(&mut self) {
        self.text.clear();
        self.cursor = 0;
        self.scroll = 0;
        self.history_idx = None;
    }

    // ── Character operations ──────────────────────────────────────────

    /// Insert a character at the cursor position.
    pub fn insert_char(&mut self, c: char) {
        let byte_pos = char_to_byte(&self.text, self.cursor);
        self.text.insert(byte_pos, c);
        self.cursor += 1;
        self.history_idx = None;
    }

    /// Delete the character before the cursor (Backspace).
    pub fn delete_back(&mut self) {
        if self.cursor == 0 || self.text.is_empty() {
            return;
        }
        self.cursor -= 1;
        let byte_pos = char_to_byte(&self.text, self.cursor);
        self.text.remove(byte_pos);
        self.history_idx = None;
    }

    /// Delete the character at the cursor (Delete key).
    pub fn delete_forward(&mut self) {
        if self.cursor >= self.char_count() {
            return;
        }
        let byte_pos = char_to_byte(&self.text, self.cursor);
        self.text.remove(byte_pos);
        self.history_idx = None;
    }

    // ── Word operations ───────────────────────────────────────────────

    /// Delete the word before the cursor (Ctrl+Backspace / Ctrl+W).
    pub fn delete_word_back(&mut self) {
        if self.cursor == 0 || self.text.is_empty() {
            return;
        }
        let chars: Vec<char> = self.text.chars().collect();
        let mut new_cursor = self.cursor;

        // Skip whitespace
        while new_cursor > 0 && chars[new_cursor - 1].is_whitespace() {
            new_cursor -= 1;
        }
        // Skip word chars
        while new_cursor > 0 && !chars[new_cursor - 1].is_whitespace() {
            new_cursor -= 1;
        }

        let start_byte = char_to_byte(&self.text, new_cursor);
        let end_byte = char_to_byte(&self.text, self.cursor);
        self.text.replace_range(start_byte..end_byte, "");
        self.cursor = new_cursor;
        self.history_idx = None;
    }

    /// Move cursor one word to the left (Alt+Left).
    pub fn word_left(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let chars: Vec<char> = self.text.chars().collect();
        let mut pos = self.cursor;
        // Skip whitespace
        while pos > 0 && chars[pos - 1].is_whitespace() {
            pos -= 1;
        }
        // Skip word chars
        while pos > 0 && !chars[pos - 1].is_whitespace() {
            pos -= 1;
        }
        self.cursor = pos;
    }

    /// Move cursor one word to the right (Alt+Right).
    pub fn word_right(&mut self) {
        let chars: Vec<char> = self.text.chars().collect();
        let len = chars.len();
        if self.cursor >= len {
            return;
        }
        let mut pos = self.cursor;
        // Skip word chars
        while pos < len && !chars[pos].is_whitespace() {
            pos += 1;
        }
        // Skip whitespace
        while pos < len && chars[pos].is_whitespace() {
            pos += 1;
        }
        self.cursor = pos;
    }

    // ── Cursor movement ───────────────────────────────────────────────

    /// Move cursor one character left.
    pub fn cursor_left(&mut self) {
        if self.cursor > 0 {
            self.cursor -= 1;
        }
    }

    /// Move cursor one character right.
    pub fn cursor_right(&mut self) {
        if self.cursor < self.char_count() {
            self.cursor += 1;
        }
    }

    /// Move cursor to the beginning of the line.
    pub fn home(&mut self) {
        self.cursor = 0;
    }

    /// Move cursor to the end of the line.
    pub fn end(&mut self) {
        self.cursor = self.char_count();
    }

    // ── Paste ─────────────────────────────────────────────────────────

    /// Insert a string at the cursor position (paste).
    /// Strips newlines for single-line input.
    pub fn paste(&mut self, text: &str) {
        // Normalize and flatten to single line
        let clean = text.replace("\r\n", " ").replace(['\r', '\n'], " ");
        let byte_pos = char_to_byte(&self.text, self.cursor);
        self.text.insert_str(byte_pos, &clean);
        self.cursor += clean.chars().count();
        self.history_idx = None;
    }

    /// Insert a string at the cursor, preserving newlines (for multiline-capable inputs).
    pub fn paste_raw(&mut self, text: &str) {
        let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
        let byte_pos = char_to_byte(&self.text, self.cursor);
        self.text.insert_str(byte_pos, &normalized);
        self.cursor += normalized.chars().count();
        self.history_idx = None;
    }

    // ── Clear line ────────────────────────────────────────────────────

    /// Clear from cursor to beginning of line (Ctrl+U).
    pub fn clear_to_start(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let byte_pos = char_to_byte(&self.text, self.cursor);
        self.text.replace_range(..byte_pos, "");
        self.cursor = 0;
        self.history_idx = None;
    }

    // ── History navigation ────────────────────────────────────────────

    /// Navigate to an older history entry (Up arrow).
    /// Returns true if navigation happened.
    pub fn history_up(&mut self) -> bool {
        if self.history.is_empty() {
            return false;
        }
        let new_idx = match self.history_idx {
            None => {
                // Save current text as draft
                self.history_draft = self.text.clone();
                self.history.len() - 1
            }
            Some(0) => return false, // already at oldest
            Some(i) => i - 1,
        };
        self.history_idx = Some(new_idx);
        self.text = self.history[new_idx].clone();
        self.cursor = self.char_count();
        true
    }

    /// Navigate to a newer history entry (Down arrow).
    /// Returns true if navigation happened.
    pub fn history_down(&mut self) -> bool {
        let Some(i) = self.history_idx else {
            return false;
        };
        if i + 1 >= self.history.len() {
            // Restore draft
            self.history_idx = None;
            self.text = self.history_draft.clone();
            self.cursor = self.char_count();
        } else {
            self.history_idx = Some(i + 1);
            self.text = self.history[i + 1].clone();
            self.cursor = self.char_count();
        }
        true
    }

    /// Whether the user is currently browsing history.
    pub fn in_history(&self) -> bool {
        self.history_idx.is_some()
    }

    /// Push a submitted value to history. Skips consecutive duplicates.
    pub fn push_history(&mut self, value: &str) {
        if value.is_empty() {
            return;
        }
        if self.history.last().map(|s| s.as_str()) == Some(value) {
            return; // skip duplicate
        }
        self.history.push(value.to_string());
        // Cap at 1000 entries
        if self.history.len() > 1000 {
            self.history.remove(0);
        }
        self.history_idx = None;
    }

    // ── Scroll management ─────────────────────────────────────────────

    /// Update horizontal scroll so the cursor stays visible within `visible_width` characters.
    pub fn update_scroll(&mut self, visible_width: usize) {
        if visible_width == 0 {
            return;
        }
        if self.cursor < self.scroll {
            self.scroll = self.cursor;
        } else if self.cursor >= self.scroll + visible_width {
            self.scroll = self.cursor - visible_width + 1;
        }
    }
}

/// Handle a key event for a `TextInput`. Returns `true` if the key was consumed.
///
/// This handles the common text editing keys shared by all single-line inputs.
/// Callers should handle Enter, Esc, and any context-specific keys themselves.
pub fn handle_text_key(input: &mut TextInput, key: crossterm::event::KeyEvent) -> bool {
    use crossterm::event::{KeyCode, KeyModifiers};

    let mods = key.modifiers;
    let code = key.code;

    match (mods, code) {
        // Backspace
        (KeyModifiers::NONE, KeyCode::Backspace) => input.delete_back(),
        // Ctrl+Backspace — delete word
        (m, KeyCode::Backspace) if m.contains(KeyModifiers::CONTROL) => {
            input.delete_word_back();
        }
        // Ctrl+W — delete word (Unix convention)
        (m, KeyCode::Char('w')) if m.contains(KeyModifiers::CONTROL) => {
            input.delete_word_back();
        }
        // Ctrl+U — clear to start
        (m, KeyCode::Char('u')) if m.contains(KeyModifiers::CONTROL) => {
            input.clear_to_start();
        }
        // Delete
        (KeyModifiers::NONE, KeyCode::Delete) => input.delete_forward(),
        // Left
        (KeyModifiers::NONE, KeyCode::Left) => input.cursor_left(),
        // Right
        (KeyModifiers::NONE, KeyCode::Right) => input.cursor_right(),
        // Alt+Left — word left
        (m, KeyCode::Left) if m.contains(KeyModifiers::ALT) => input.word_left(),
        // Alt+Right — word right
        (m, KeyCode::Right) if m.contains(KeyModifiers::ALT) => input.word_right(),
        // Home
        (KeyModifiers::NONE, KeyCode::Home) => input.home(),
        // End
        (KeyModifiers::NONE, KeyCode::End) => input.end(),
        // Character insert
        (m, KeyCode::Char(c))
            if !m.contains(KeyModifiers::CONTROL) && !m.contains(KeyModifiers::ALT) =>
        {
            input.insert_char(c);
        }
        _ => return false,
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insert_and_cursor() {
        let mut ti = TextInput::new();
        ti.insert_char('h');
        ti.insert_char('i');
        assert_eq!(ti.text, "hi");
        assert_eq!(ti.cursor, 2);
    }

    #[test]
    fn delete_back() {
        let mut ti = TextInput::new();
        ti.set_text("hello");
        ti.cursor = 3; // after "hel"
        ti.delete_back();
        assert_eq!(ti.text, "helo");
        assert_eq!(ti.cursor, 2);
    }

    #[test]
    fn delete_back_at_start_is_noop() {
        let mut ti = TextInput::new();
        ti.set_text("x");
        ti.cursor = 0;
        ti.delete_back();
        assert_eq!(ti.text, "x");
        assert_eq!(ti.cursor, 0);
    }

    #[test]
    fn delete_forward() {
        let mut ti = TextInput::new();
        ti.set_text("hello");
        ti.cursor = 1; // after "h"
        ti.delete_forward();
        assert_eq!(ti.text, "hllo");
        assert_eq!(ti.cursor, 1);
    }

    #[test]
    fn delete_forward_at_end_is_noop() {
        let mut ti = TextInput::new();
        ti.set_text("hi");
        ti.cursor = 2;
        ti.delete_forward();
        assert_eq!(ti.text, "hi");
    }

    #[test]
    fn word_operations() {
        let mut ti = TextInput::new();
        ti.set_text("hello world foo");
        ti.end(); // cursor at end (15)

        ti.word_left(); // → before "foo" (12)
        assert_eq!(ti.cursor, 12);

        ti.word_left(); // → before "world" (6)
        assert_eq!(ti.cursor, 6);

        ti.word_right(); // → after "world " (12)
        assert_eq!(ti.cursor, 12);
    }

    #[test]
    fn delete_word_back() {
        let mut ti = TextInput::new();
        ti.set_text("hello world");
        ti.end();
        ti.delete_word_back();
        assert_eq!(ti.text, "hello ");
        assert_eq!(ti.cursor, 6);
    }

    #[test]
    fn clear_to_start() {
        let mut ti = TextInput::new();
        ti.set_text("hello world");
        ti.cursor = 5;
        ti.clear_to_start();
        assert_eq!(ti.text, " world");
        assert_eq!(ti.cursor, 0);
    }

    #[test]
    fn paste_strips_newlines() {
        let mut ti = TextInput::new();
        ti.paste("line1\nline2\r\nline3");
        assert_eq!(ti.text, "line1 line2 line3");
    }

    #[test]
    fn paste_raw_preserves_newlines() {
        let mut ti = TextInput::new();
        ti.paste_raw("line1\r\nline2");
        assert_eq!(ti.text, "line1\nline2");
    }

    #[test]
    fn paste_at_cursor_position() {
        let mut ti = TextInput::new();
        ti.set_text("hd");
        ti.cursor = 1; // between "h" and "d"
        ti.paste("ello worl");
        assert_eq!(ti.text, "hello world");
        assert_eq!(ti.cursor, 10);
    }

    #[test]
    fn masked_display() {
        let mut ti = TextInput::masked('●');
        ti.set_text("secret");
        assert_eq!(ti.display_text(), "●●●●●●");
        assert_eq!(ti.text, "secret"); // real text preserved
    }

    #[test]
    fn history_navigation() {
        let mut ti = TextInput::new();
        ti.push_history("first");
        ti.push_history("second");
        ti.push_history("third");
        ti.set_text("current");

        assert!(ti.history_up()); // → "third"
        assert_eq!(ti.text, "third");

        assert!(ti.history_up()); // → "second"
        assert_eq!(ti.text, "second");

        assert!(ti.history_down()); // → "third"
        assert_eq!(ti.text, "third");

        assert!(ti.history_down()); // → restore "current"
        assert_eq!(ti.text, "current");

        assert!(!ti.history_down()); // noop
    }

    #[test]
    fn history_skips_duplicates() {
        let mut ti = TextInput::new();
        ti.push_history("same");
        ti.push_history("same");
        assert_eq!(ti.history.len(), 1);
    }

    #[test]
    fn update_scroll() {
        let mut ti = TextInput::new();
        ti.set_text("a long input string here");
        ti.cursor = 20;
        ti.update_scroll(10);
        assert!(ti.scroll > 0);
        assert!(ti.cursor >= ti.scroll);
        assert!(ti.cursor < ti.scroll + 10);
    }

    #[test]
    fn unicode_handling() {
        let mut ti = TextInput::new();
        ti.insert_char('日');
        ti.insert_char('本');
        ti.insert_char('語');
        assert_eq!(ti.char_count(), 3);
        assert_eq!(ti.text, "日本語");

        ti.cursor = 1;
        ti.delete_forward(); // delete '本'
        assert_eq!(ti.text, "日語");

        ti.delete_back(); // delete '日'
        assert_eq!(ti.text, "語");
        assert_eq!(ti.cursor, 0);
    }

    #[test]
    fn home_end() {
        let mut ti = TextInput::new();
        ti.set_text("hello");
        ti.home();
        assert_eq!(ti.cursor, 0);
        ti.end();
        assert_eq!(ti.cursor, 5);
    }

    #[test]
    fn handle_text_key_basics() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let mut ti = TextInput::new();

        // Type "hi"
        let consumed = handle_text_key(
            &mut ti,
            KeyEvent::new(KeyCode::Char('h'), KeyModifiers::NONE),
        );
        assert!(consumed);
        handle_text_key(
            &mut ti,
            KeyEvent::new(KeyCode::Char('i'), KeyModifiers::NONE),
        );
        assert_eq!(ti.text, "hi");

        // Backspace
        handle_text_key(
            &mut ti,
            KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE),
        );
        assert_eq!(ti.text, "h");

        // Home
        handle_text_key(&mut ti, KeyEvent::new(KeyCode::Home, KeyModifiers::NONE));
        assert_eq!(ti.cursor, 0);

        // Unhandled key returns false
        let consumed = handle_text_key(&mut ti, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(!consumed);
    }
}
