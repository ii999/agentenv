use agentenv::credential::{CapturedSecret, Secret, SecretDomainError};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;
use zeroize::Zeroizing;

pub(super) struct Editor {
    value: Zeroizing<String>,
    cursor: usize,
    pub revealed: bool,
}

impl Editor {
    pub fn new() -> Self {
        Self {
            value: Zeroizing::new(String::new()),
            cursor: 0,
            revealed: false,
        }
    }

    pub fn character_count(&self) -> usize {
        self.value.graphemes(true).count()
    }
    pub fn byte_count(&self) -> usize {
        self.value.len()
    }
    pub fn has_line_breaks(&self) -> bool {
        self.value.contains(['\r', '\n'])
    }

    pub fn secret(&self) -> Result<Secret, SecretDomainError> {
        CapturedSecret::new(self.value.as_bytes().to_vec()).into_secret()
    }

    // A replacement allocation lets the old owner zeroize both removed bytes
    // and spare capacity; growing String in place can free an unzeroized buffer.
    fn replace(&mut self, start: usize, end: usize, inserted: &str) {
        let mut next = Zeroizing::new(String::with_capacity(
            self.value.len() - (end - start) + inserted.len(),
        ));
        next.push_str(&self.value[..start]);
        next.push_str(inserted);
        next.push_str(&self.value[end..]);
        self.value = next;
        let target = start + inserted.len();
        self.cursor = self
            .value
            .grapheme_indices(true)
            .map(|(offset, _)| offset)
            .find(|&offset| offset >= target)
            .unwrap_or(self.value.len());
    }

    pub fn insert(&mut self, text: &str) {
        self.replace(self.cursor, self.cursor, text);
    }

    pub fn clear(&mut self) {
        self.value = Zeroizing::new(String::new());
        self.cursor = 0;
    }

    fn previous(&self) -> usize {
        self.value
            .grapheme_indices(true)
            .map(|(offset, _)| offset)
            .take_while(|&offset| offset < self.cursor)
            .last()
            .unwrap_or(0)
    }

    fn next(&self) -> usize {
        self.value
            .grapheme_indices(true)
            .map(|(offset, _)| offset)
            .find(|&offset| offset > self.cursor)
            .unwrap_or(self.value.len())
    }

    pub fn left(&mut self) {
        self.cursor = self.previous();
    }
    pub fn right(&mut self) {
        self.cursor = self.next();
    }
    pub fn backspace(&mut self) {
        self.replace(self.previous(), self.cursor, "");
    }
    pub fn delete(&mut self) {
        self.replace(self.cursor, self.next(), "");
    }

    fn line_start(&self) -> usize {
        self.value[..self.cursor]
            .rfind(['\r', '\n'])
            .map_or(0, |offset| offset + 1)
    }

    fn line_end(&self) -> usize {
        self.value[self.cursor..]
            .find(['\r', '\n'])
            .map_or(self.value.len(), |offset| self.cursor + offset)
    }

    pub fn home(&mut self, entire: bool) {
        self.cursor = if entire { 0 } else { self.line_start() };
    }
    pub fn end(&mut self, entire: bool) {
        self.cursor = if entire {
            self.value.len()
        } else {
            self.line_end()
        };
    }

    pub fn vertical(&mut self, down: bool) {
        let start = self.line_start();
        let column = self.value[start..self.cursor].graphemes(true).count();
        let (start, end) = if down {
            let end = self.line_end();
            if end == self.value.len() {
                return;
            }
            let start = end
                + if self.value[end..].starts_with("\r\n") {
                    2
                } else {
                    1
                };
            let end = self.value[start..]
                .find(['\r', '\n'])
                .map_or(self.value.len(), |offset| start + offset);
            (start, end)
        } else {
            if start == 0 {
                return;
            }
            let end = start
                - if self.value[..start].ends_with("\r\n") {
                    2
                } else {
                    1
                };
            let start = self.value[..end]
                .rfind(['\r', '\n'])
                .map_or(0, |offset| offset + 1);
            (start, end)
        };
        self.cursor = self.value[start..end]
            .grapheme_indices(true)
            .nth(column)
            .map_or(end, |(offset, _)| start + offset);
    }

    pub fn viewport(&self, width: usize) -> (Zeroizing<String>, usize) {
        let width = width.max(4);
        let graphemes = self.value.grapheme_indices(true).collect::<Vec<_>>();
        let cursor = graphemes
            .iter()
            .position(|(offset, _)| *offset == self.cursor)
            .unwrap_or(graphemes.len());
        let cell_width = |text: &str| {
            if self.revealed {
                let cells = UnicodeWidthStr::width(escaped(text).as_str());
                if cells > width - 2 {
                    1
                } else {
                    cells
                }
            } else {
                1
            }
        };
        let mut start = cursor;
        let mut before = 0;
        while start > 0 {
            let next = cell_width(graphemes[start - 1].1);
            if before + next > width - 3 {
                break;
            }
            start -= 1;
            before += next;
        }
        let left = usize::from(start != 0);
        let mut end = start;
        let mut cells = left;
        while let Some((_, text)) = graphemes.get(end) {
            let next = cell_width(text);
            if cells + next > width - 1 {
                break;
            }
            cells += next;
            end += 1;
        }

        // Allocate once before copying any revealed text into the viewport.
        let capacity = if self.revealed {
            graphemes[start..end]
                .iter()
                .map(|(_, text)| escaped(text).len())
                .sum::<usize>()
                + 2
        } else {
            end - start + 2
        };
        let mut view = Zeroizing::new(String::with_capacity(capacity));
        if left != 0 {
            view.push('<');
        }
        for (_, text) in &graphemes[start..end] {
            if self.revealed {
                let safe = escaped(text);
                if UnicodeWidthStr::width(safe.as_str()) > width - 2 {
                    view.push('?');
                } else {
                    view.push_str(&safe);
                }
            } else {
                view.push('*');
            }
        }
        if end < graphemes.len() {
            view.push('>');
        }
        (view, left + before)
    }
}

/// Escapes terminal commands and invisible formatting without changing stored
/// bytes. A literal backslash is escaped so a newline preview is unambiguous.
pub(super) fn escaped(text: &str) -> Zeroizing<String> {
    let must_escape = |character: char| {
        character.is_control()
            || matches!(character,
                '\\' | '\u{061c}' | '\u{200e}' | '\u{200f}' | '\u{2028}'..='\u{202e}' | '\u{2066}'..='\u{2069}'
            )
    };
    let length = text
        .graphemes(true)
        .map(|grapheme| {
            let zero_width = UnicodeWidthStr::width(grapheme) == 0;
            grapheme
                .chars()
                .map(|c| {
                    if must_escape(c) || zero_width {
                        c.escape_default().count()
                    } else {
                        c.len_utf8()
                    }
                })
                .sum::<usize>()
        })
        .sum();
    let mut safe = Zeroizing::new(String::with_capacity(length));
    for grapheme in text.graphemes(true) {
        let zero_width = UnicodeWidthStr::width(grapheme) == 0;
        for character in grapheme.chars() {
            if must_escape(character) || zero_width {
                safe.extend(character.escape_default());
            } else {
                safe.push(character);
            }
        }
    }
    safe
}
