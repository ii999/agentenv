use std::ops::Range;

use agentenv::credential::{CapturedSecret, Secret, SecretDomainError};
use eframe::egui::{epaint::text::CharIndex, TextBuffer};
use zeroize::Zeroizing;

/// Editable text stays in the dialog; only a validated snapshot reaches storage.
pub(super) struct Draft(Zeroizing<String>);

impl Draft {
    pub fn new() -> Self {
        Self(Zeroizing::new(String::new()))
    }

    pub fn secret(&self) -> Result<Secret, SecretDomainError> {
        CapturedSecret::new(self.0.as_bytes().to_vec()).into_secret()
    }

    fn offset(&self, index: CharIndex) -> usize {
        self.0
            .char_indices()
            .nth(index.0)
            .map_or(self.0.len(), |(offset, _)| offset)
    }

    fn replace(&mut self, start: usize, end: usize, text: &str) {
        let mut next = Zeroizing::new(String::with_capacity(
            self.0.len() - (end - start) + text.len(),
        ));
        next.push_str(&self.0[..start]);
        next.push_str(text);
        next.push_str(&self.0[end..]);
        // Replacing the allocation zeroizes deleted text and avoids freeing
        // an unzeroized allocation when the editor needs to grow its buffer.
        self.0 = next;
    }
}

impl TextBuffer for Draft {
    fn type_id(&self) -> std::any::TypeId {
        std::any::TypeId::of::<Self>()
    }
    fn is_mutable(&self) -> bool {
        true
    }
    fn as_str(&self) -> &str {
        &self.0
    }

    fn insert_text(&mut self, text: &str, index: CharIndex) -> usize {
        let offset = self.offset(index);
        self.replace(offset, offset, text);
        text.chars().count()
    }

    fn delete_char_range(&mut self, range: Range<CharIndex>) {
        self.replace(self.offset(range.start), self.offset(range.end), "");
    }

    fn clear(&mut self) {
        self.0 = Zeroizing::new(String::new());
    }

    fn replace_with(&mut self, text: &str) {
        self.0 = Zeroizing::new(text.to_owned());
    }
}

pub(super) fn display_text(text: &str, revealed: bool) -> String {
    text.chars()
        .map(|character| {
            if character == '\n' {
                '\n'
            } else if !revealed {
                '•'
            } else {
                match character {
                    '\r' => '␍',
                    '\t' => '⇥',
                    '\0' => '␀',
                    c if c.is_control() => '�',
                    c => c,
                }
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buffer_edits_preserve_multiline_unicode_and_whitespace() {
        let mut draft = Draft::new();
        let value = "  a🙂e\u{301}\r\n密钥\t\n";
        assert_eq!(
            draft.insert_text(value, CharIndex(0)),
            value.chars().count()
        );
        assert_eq!(draft.as_str(), value);
        draft.delete_char_range(CharIndex(3)..CharIndex(4));
        assert_eq!(draft.as_str(), "  ae\u{301}\r\n密钥\t\n");
        draft.insert_text("中", CharIndex(3));
        assert_eq!(draft.as_str(), "  a中e\u{301}\r\n密钥\t\n");
        assert!(draft.secret().is_ok());
        draft.clear();
        assert!(draft.secret().is_err());
    }

    #[test]
    fn masking_keeps_scalar_indices_and_newline_layout_without_candidate_text() {
        let value = "canary-密钥🙂\r\nsecond\t\n";
        let masked = display_text(value, false);
        assert_eq!(masked.chars().count(), value.chars().count());
        assert_eq!(masked.matches('\n').count(), value.matches('\n').count());
        assert!(masked.chars().all(|c| matches!(c, '•' | '\n')));
    }
}
