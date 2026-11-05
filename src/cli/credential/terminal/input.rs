//! Decode terminal text and paste framing before interpreting editor shortcuts.

use std::collections::VecDeque;
use std::io;
use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
#[cfg(any(unix, test))]
use zeroize::Zeroize;
use zeroize::Zeroizing;

use super::Input;

const PASTE_START: &str = "\x1b[200~";
const PASTE_END: &str = "\x1b[201~";
const ESCAPE_WAIT: Duration = Duration::from_millis(80);

pub(super) struct Decoder {
    #[cfg(any(windows, test))]
    high_surrogate: Option<(u16, u16, KeyModifiers)>,
    #[cfg(any(unix, test))]
    utf8: Zeroizing<Vec<u8>>,
    sequence: Zeroizing<String>,
    sequence_started: Instant,
    paste: Option<Zeroizing<Vec<u8>>>,
    ready: VecDeque<Input>,
}

impl Decoder {
    pub fn new() -> Self {
        Self {
            #[cfg(any(windows, test))]
            high_surrogate: None,
            #[cfg(any(unix, test))]
            utf8: Zeroizing::new(Vec::with_capacity(4)),
            sequence: Zeroizing::new(String::new()),
            sequence_started: Instant::now(),
            paste: None,
            ready: VecDeque::new(),
        }
    }

    pub fn pop(&mut self) -> Option<Input> {
        self.ready.pop_front()
    }

    pub fn key(&mut self, code: KeyCode, modifiers: KeyModifiers) {
        if self.paste.is_none() {
            self.ready
                .push_back(Input::Key(KeyEvent::new(code, modifiers)));
        }
    }

    #[cfg(any(unix, test))]
    pub fn bytes(&mut self, bytes: &[u8]) -> io::Result<()> {
        for &byte in bytes {
            if self.paste.is_some() {
                self.paste_byte(byte);
                continue;
            }
            self.utf8.push(byte);
            match std::str::from_utf8(&self.utf8) {
                Ok(text) => {
                    let character = text.chars().next();
                    self.utf8.zeroize();
                    if let Some(character) = character {
                        self.character(character, KeyModifiers::NONE);
                    }
                }
                Err(error) if error.error_len().is_none() => {}
                Err(_) => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "terminal input is not valid UTF-8",
                    ))
                }
            }
        }
        Ok(())
    }

    #[cfg(any(windows, test))]
    pub fn utf16(&mut self, unit: u16, repeat: u16, modifiers: KeyModifiers) -> io::Result<()> {
        let repeat = repeat.max(1);
        let decoded = if let Some((high, count, saved_modifiers)) = self.high_surrogate.take() {
            if !(0xdc00..=0xdfff).contains(&unit) || count != repeat || saved_modifiers != modifiers
            {
                return Err(invalid_utf16());
            }
            char::from_u32(0x10000 + ((u32::from(high) - 0xd800) << 10) + u32::from(unit) - 0xdc00)
                .ok_or_else(invalid_utf16)?
        } else if (0xd800..=0xdbff).contains(&unit) {
            self.high_surrogate = Some((unit, repeat, modifiers));
            return Ok(());
        } else {
            char::from_u32(u32::from(unit)).ok_or_else(invalid_utf16)?
        };
        for _ in 0..repeat {
            self.character(decoded, modifiers);
        }
        Ok(())
    }

    fn character(&mut self, value: char, modifiers: KeyModifiers) {
        if self.paste.is_some() {
            let mut encoded = Zeroizing::new([0_u8; 4]);
            for &byte in value.encode_utf8(&mut *encoded).as_bytes() {
                self.paste_byte(byte);
            }
            return;
        }
        if !self.sequence.is_empty() {
            if value.is_control() {
                self.sequence = Zeroizing::new(String::new());
                self.character(value, modifiers);
                return;
            }
            push(&mut self.sequence, value);
            if self.sequence.len() == 2 && matches!(value, '[' | 'O') {
                return;
            }
            // Linux console function keys use ESC[[A through ESC[[E.
            // The second '[' is a prefix, not the CSI final character.
            if self.sequence.as_str() == "\x1b[[" {
                return;
            }
            if self.sequence.starts_with("\x1b[") && !('@'..='~').contains(&value) {
                return;
            }
            if self.sequence.as_str() == PASTE_START {
                self.sequence = Zeroizing::new(String::new());
                self.paste = Some(Zeroizing::new(Vec::new()));
                return;
            }
            let sequence = std::mem::replace(&mut self.sequence, Zeroizing::new(String::new()));
            if sequence.as_str() == "\x1b[O" {
                self.ready.push_back(Input::Hide);
            } else if sequence.as_str() == "\x1b[I" {
                self.ready.push_back(Input::Redraw);
            } else if let Some(key) = vt_key(&sequence) {
                self.ready.push_back(Input::Key(key));
            } else if sequence.chars().count() == 2 {
                self.literal(value, modifiers | KeyModifiers::ALT);
            }
        } else if value == '\u{1b}' {
            push(&mut self.sequence, value);
            self.sequence_started = Instant::now();
        } else {
            self.literal(value, modifiers);
        }
    }

    fn paste_byte(&mut self, byte: u8) {
        if let Some(paste) = &mut self.paste {
            if paste.len() == paste.capacity() {
                let mut replacement = Zeroizing::new(Vec::with_capacity(
                    (paste.len() + 1).max(paste.capacity().saturating_mul(2)),
                ));
                replacement.extend_from_slice(paste);
                *paste = replacement;
            }
            paste.push(byte);
            if paste.ends_with(PASTE_END.as_bytes()) {
                let length = paste.len() - PASTE_END.len();
                paste.truncate(length);
                if let Some(bytes) = self.paste.take() {
                    let input = match std::str::from_utf8(&bytes) {
                        Ok(text) => Input::Paste(Zeroizing::new(text.to_owned())),
                        Err(_) => Input::InvalidPaste,
                    };
                    self.ready.push_back(input);
                }
            }
        }
    }

    fn literal(&mut self, value: char, modifiers: KeyModifiers) {
        let (code, modifiers) = match value {
            '\r' => (KeyCode::Enter, modifiers),
            '\n' => (KeyCode::Char('j'), KeyModifiers::CONTROL),
            '\t' => (KeyCode::Tab, modifiers),
            '\u{8}' | '\u{7f}' => (KeyCode::Backspace, modifiers),
            '\u{1}'..='\u{1a}' => (
                KeyCode::Char(char::from(value as u8 + b'a' - 1)),
                KeyModifiers::CONTROL,
            ),
            _ => (KeyCode::Char(value), modifiers),
        };
        self.ready
            .push_back(Input::Key(KeyEvent::new(code, modifiers)));
    }

    pub fn idle(&mut self) {
        if self.sequence.as_str() == "\x1b" && self.sequence_started.elapsed() >= ESCAPE_WAIT {
            self.sequence = Zeroizing::new(String::new());
            self.key(KeyCode::Esc, KeyModifiers::NONE);
        }
    }
}

fn vt_key(sequence: &str) -> Option<KeyEvent> {
    if sequence.starts_with("\x1b[[") {
        return (sequence == "\x1b[[B").then(|| KeyEvent::new(KeyCode::F(2), KeyModifiers::NONE));
    }
    let body = sequence
        .strip_prefix("\x1b[")
        .or_else(|| sequence.strip_prefix("\x1bO"))?;
    let ending = body.chars().last()?;
    let parameters = &body[..body.len() - ending.len_utf8()];
    let mut parameters = parameters.split(';');
    let first = parameters.next().unwrap_or("");
    let modifier = parameters
        .next()
        .map(str::parse::<u8>)
        .transpose()
        .ok()?
        .unwrap_or(1);
    let bits = modifier.checked_sub(1)?;
    let mut modifiers = KeyModifiers::NONE;
    if bits & 1 != 0 {
        modifiers |= KeyModifiers::SHIFT;
    }
    if bits & 2 != 0 {
        modifiers |= KeyModifiers::ALT;
    }
    if bits & 4 != 0 {
        modifiers |= KeyModifiers::CONTROL;
    }
    let code = match ending {
        'A' => KeyCode::Up,
        'B' => KeyCode::Down,
        'C' => KeyCode::Right,
        'D' => KeyCode::Left,
        'H' => KeyCode::Home,
        'F' => KeyCode::End,
        'Q' => KeyCode::F(2),
        '~' => match first {
            "1" | "7" => KeyCode::Home,
            "4" | "8" => KeyCode::End,
            "3" => KeyCode::Delete,
            "12" => KeyCode::F(2),
            _ => return None,
        },
        _ => return None,
    };
    Some(KeyEvent::new(code, modifiers))
}

#[cfg(any(windows, test))]
fn invalid_utf16() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "the terminal supplied an incomplete UTF-16 character",
    )
}

fn push(buffer: &mut Zeroizing<String>, value: char) {
    if buffer.capacity() - buffer.len() < value.len_utf8() {
        let mut replacement = Zeroizing::new(String::with_capacity(
            (buffer.len() + value.len_utf8()).max(buffer.capacity().saturating_mul(2)),
        ));
        replacement.push_str(buffer);
        *buffer = replacement;
    }
    buffer.push(value);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed(decoder: &mut Decoder, text: &str) {
        for unit in text.encode_utf16() {
            decoder.utf16(unit, 1, KeyModifiers::NONE).unwrap();
        }
    }

    #[test]
    fn utf8_input_preserves_split_characters_and_rejects_malformed_paste() {
        let mut decoder = Decoder::new();
        decoder.bytes(b"\xf0\x9f").unwrap();
        assert!(decoder.pop().is_none());
        decoder.bytes(b"\x99\x82").unwrap();
        assert!(matches!(
            decoder.pop(),
            Some(Input::Key(KeyEvent {
                code: KeyCode::Char('🙂'),
                ..
            }))
        ));
        decoder.bytes(b"\x1b[200~a\xffb\x1b[201~").unwrap();
        assert!(matches!(decoder.pop(), Some(Input::InvalidPaste)));
        decoder
            .bytes("\x1b[200~\u{fffd}\x1b[201~".as_bytes())
            .unwrap();
        assert!(matches!(decoder.pop(), Some(Input::Paste(value)) if value.as_str() == "\u{fffd}"));
    }

    #[test]
    fn linux_function_keys_consume_their_whole_encoding_across_reads() {
        let mut decoder = Decoder::new();
        for byte in b"\x1b[[" {
            decoder.bytes(&[*byte]).unwrap();
            assert!(decoder.pop().is_none());
        }
        decoder.bytes(b"B").unwrap();
        assert!(matches!(
            decoder.pop(),
            Some(Input::Key(KeyEvent {
                code: KeyCode::F(2),
                ..
            }))
        ));
        for suffix in b"ACDE" {
            decoder.bytes(b"\x1b[[").unwrap();
            decoder.bytes(&[*suffix]).unwrap();
            assert!(
                decoder.pop().is_none(),
                "unsupported function keys cannot insert their suffix"
            );
        }
    }

    #[test]
    fn framed_paste_preserves_utf16_and_controls_across_input_records() {
        let mut decoder = Decoder::new();
        feed(&mut decoder, "\x1b[20");
        assert!(decoder.pop().is_none());
        feed(&mut decoder, "0~日本語🙂\r\n\t\x13\x03\0\x1b[201");
        assert!(decoder.pop().is_none());
        feed(&mut decoder, "~");
        match decoder.pop() {
            Some(Input::Paste(value)) => assert_eq!(value.as_str(), "日本語🙂\r\n\t\x13\x03\0"),
            _ => panic!("a complete framed paste must be one paste event"),
        }
        feed(&mut decoder, "\x13");
        assert!(matches!(
            decoder.pop(),
            Some(Input::Key(KeyEvent {
                code: KeyCode::Char('s'),
                modifiers: KeyModifiers::CONTROL,
                ..
            }))
        ));
    }

    #[test]
    fn surrogate_pairs_and_altgr_characters_survive_console_decoding() {
        let mut decoder = Decoder::new();
        decoder.utf16(0xd83d, 2, KeyModifiers::NONE).unwrap();
        assert!(decoder.pop().is_none());
        decoder.utf16(0xde42, 2, KeyModifiers::NONE).unwrap();
        for _ in 0..2 {
            assert!(matches!(
                decoder.pop(),
                Some(Input::Key(KeyEvent {
                    code: KeyCode::Char('🙂'),
                    ..
                }))
            ));
        }
        let modifiers = KeyModifiers::CONTROL | KeyModifiers::ALT;
        decoder.utf16('@' as u16, 1, modifiers).unwrap();
        assert!(
            matches!(decoder.pop(), Some(Input::Key(KeyEvent { code: KeyCode::Char('@'), modifiers: actual, .. })) if actual == modifiers)
        );
        assert!(decoder.utf16(0xdc00, 1, KeyModifiers::NONE).is_err());
    }

    #[test]
    fn native_and_vt_keys_remain_separate_from_pasted_data() {
        let mut decoder = Decoder::new();
        decoder.key(KeyCode::Left, KeyModifiers::CONTROL);
        assert!(matches!(
            decoder.pop(),
            Some(Input::Key(KeyEvent {
                code: KeyCode::Left,
                modifiers: KeyModifiers::CONTROL,
                ..
            }))
        ));
        feed(&mut decoder, "\x1bOQ");
        assert!(matches!(
            decoder.pop(),
            Some(Input::Key(KeyEvent {
                code: KeyCode::F(2),
                ..
            }))
        ));
        feed(&mut decoder, "\x1b[1;5H");
        assert!(matches!(
            decoder.pop(),
            Some(Input::Key(KeyEvent {
                code: KeyCode::Home,
                modifiers: KeyModifiers::CONTROL,
                ..
            }))
        ));
        feed(&mut decoder, "\x1b[99;99~");
        assert!(
            decoder.pop().is_none(),
            "unsupported key sequences must not insert their suffix"
        );
        feed(&mut decoder, "\x1b");
        decoder.sequence_started = Instant::now() - ESCAPE_WAIT;
        decoder.idle();
        assert!(matches!(
            decoder.pop(),
            Some(Input::Key(KeyEvent {
                code: KeyCode::Esc,
                ..
            }))
        ));
        feed(&mut decoder, PASTE_START);
        decoder.key(KeyCode::Left, KeyModifiers::NONE);
        assert!(decoder.pop().is_none());
        feed(&mut decoder, PASTE_END);
        assert!(matches!(decoder.pop(), Some(Input::Paste(value)) if value.is_empty()));
    }
}
