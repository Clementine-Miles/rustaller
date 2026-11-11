use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::text::Line;

/// Draft values never implement Debug; sensitive values are masked by callers.
#[derive(Clone, Eq, PartialEq)]
pub(super) struct TextValue(pub String);
impl TextValue {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }
    pub fn replace(&mut self, value: String) {
        *self = Self(value);
    }
}
impl Drop for TextValue {
    fn drop(&mut self) {
        let mut bytes = std::mem::take(&mut self.0).into_bytes();
        bytes.fill(0);
    }
}

pub(super) const MAX_INPUT: usize = 8192;
pub(super) struct Editor {
    pub value: TextValue,
    pub cursor: usize,
    pub secret: bool,
}
impl Editor {
    pub fn new(value: &str, secret: bool) -> Self {
        Self {
            value: TextValue::new(value),
            cursor: value.chars().count(),
            secret,
        }
    }
    fn byte_at(&self, character: usize) -> usize {
        self.value
            .0
            .char_indices()
            .nth(character)
            .map_or(self.value.0.len(), |(index, _)| index)
    }
    pub fn paste(&mut self, text: &str) -> Result<(), &'static str> {
        if text.chars().any(char::is_control) {
            return Err("Use one line without newlines or control characters");
        }
        if text.len() > MAX_INPUT.saturating_sub(self.value.0.len()) {
            return Err("Input exceeds 8192 bytes; the paste was not saved");
        }
        let index = self.byte_at(self.cursor);
        self.value.0.insert_str(index, text);
        self.cursor += text.chars().count();
        Ok(())
    }
    pub fn key(&mut self, key: KeyEvent) -> Result<(), &'static str> {
        match key.code {
            KeyCode::Left => self.cursor = self.cursor.saturating_sub(1),
            KeyCode::Right => self.cursor = (self.cursor + 1).min(self.value.0.chars().count()),
            KeyCode::Home => self.cursor = 0,
            KeyCode::End => self.cursor = self.value.0.chars().count(),
            KeyCode::Backspace if self.cursor > 0 => {
                self.cursor -= 1;
                self.value.0.remove(self.byte_at(self.cursor));
            }
            KeyCode::Delete if self.cursor < self.value.0.chars().count() => {
                self.value.0.remove(self.byte_at(self.cursor));
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.value.replace(String::new());
                self.cursor = 0;
            }
            KeyCode::Char(character)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                return self.paste(&character.to_string());
            }
            _ => {}
        }
        Ok(())
    }
    pub fn visible(&self, width: usize) -> (String, usize) {
        let characters: Vec<_> = if self.secret {
            vec!['*'; self.value.0.chars().count()]
        } else {
            self.value
                .0
                .chars()
                .map(|character| {
                    if character.is_control() {
                        '\u{fffd}'
                    } else {
                        character
                    }
                })
                .collect()
        };
        let mut start = self.cursor;
        let mut cursor = 0;
        while start > 0 {
            let character_width = Line::raw(characters[start - 1].to_string()).width();
            if cursor + character_width >= width.max(1) {
                break;
            }
            cursor += character_width;
            start -= 1;
        }
        let mut visible = String::new();
        let mut used = 0;
        for character in &characters[start..] {
            let character_width = Line::raw(character.to_string()).width();
            if used + character_width > width {
                break;
            }
            used += character_width;
            visible.push(*character);
        }
        (visible, cursor)
    }
}
