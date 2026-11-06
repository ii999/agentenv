//! Editable credential capture. Only explicit reveal renders candidate bytes;
//! the editable buffer survives validation and provider failures until exit.

mod editor;
mod input;
#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;

use std::io;
use std::time::Duration;

use agentenv::config::CredentialDef;
use agentenv::credential::provider_for;
use agentenv::error::AppError;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;
use zeroize::Zeroizing;

use super::super::{signals::install_signal_forwarder, Output};
use editor::Editor;
#[cfg(unix)]
use unix::Console;
#[cfg(windows)]
use windows::Console;

const ENTER_SCREEN: &[u8] = b"\x1b[?1049h\x1b[?2004h\x1b[?1004h";
const LEAVE_SCREEN: &[u8] = b"\x1b[?25h\x1b[?1004l\x1b[?2004l\x1b[2J\x1b[H\x1b[?1049l";

enum Input {
    Key(KeyEvent),
    Paste(Zeroizing<String>),
    InvalidPaste,
    Redraw,
    Hide,
}

enum Outcome {
    Stored,
    Cancelled(i32),
}

pub(super) fn edit_and_store(credential: &CredentialDef) -> Result<Output, AppError> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .map_err(terminal_error)?;
    let (cancel, cancellation) = agentenv::cancel::channel();
    install_signal_forwarder(&runtime, cancel, || {
        AppError::Usage(
            "could not install terminal input cancellation handlers; retry at a terminal".into(),
        )
    })?;

    let mut terminal = Terminal::open().map_err(terminal_error)?;
    let result = run(&mut terminal, credential, &cancellation);
    let cleanup = terminal.restore();
    let outcome = match result {
        Ok(outcome) => outcome,
        Err(error) => {
            return Err(AppError::Usage(match cleanup {
                Ok(()) => format!("credential entry failed: {error}; retry at a terminal"),
                Err(cleanup) => format!(
                    "credential entry failed: {error}; terminal restoration also failed: {cleanup}; reset the terminal before retrying"
                ),
            }));
        }
    };
    let mut output = match outcome {
        Outcome::Stored => Output {
            stdout: format!("Credential '{}' stored.\n", credential.name),
            stderr: String::new(),
            status: 0,
        },
        Outcome::Cancelled(signal) => Output {
            stdout: String::new(),
            stderr: "Credential entry cancelled.\n".into(),
            status: 128 + signal,
        },
    };
    if let Err(error) = cleanup {
        output.stderr.push_str(&format!(
            "Could not restore the terminal: {error}; reset the terminal before continuing.\n"
        ));
        if output.status == 0 {
            // The store has committed even if terminal cleanup subsequently fails.
            output.status = 1;
        }
    }
    Ok(output)
}

fn terminal_error(error: io::Error) -> AppError {
    AppError::Usage(format!(
        "could not open the credential editor: {error}; run 'agentenv credential set <name>' in an interactive terminal"
    ))
}

fn run(
    terminal: &mut Terminal,
    credential: &CredentialDef,
    cancellation: &agentenv::cancel::Cancellation,
) -> io::Result<Outcome> {
    let mut editor = Editor::new();
    let provider = provider_for(credential);
    let mut feedback = "Type or paste a value, then press Ctrl+S to save.".to_owned();
    terminal.render(credential, &editor, &feedback)?;
    loop {
        if let Some(signal) = cancellation.pending() {
            return Ok(Outcome::Cancelled(signal));
        }
        let Some(input) = terminal.read(Duration::from_millis(50))? else {
            continue;
        };
        match input {
            Input::Paste(value) => {
                let count = value.graphemes(true).count();
                editor.insert(&value);
                feedback = format!(
                    "Pasted {count} characters. Review the value, then press Ctrl+S to save."
                );
            }
            Input::InvalidPaste => {
                feedback = "Paste rejected: the content is not valid UTF-8. Your input is unchanged; copy UTF-8 text and paste again.".into();
            }
            Input::Hide => editor.revealed = false,
            Input::Redraw => {}
            Input::Key(key) => {
                let control = key.modifiers.contains(KeyModifiers::CONTROL);
                // AltGr can arrive as Ctrl+Alt; its text must remain insertable.
                let shortcut = control && !key.modifiers.contains(KeyModifiers::ALT);
                match key.code {
                    KeyCode::Esc => return Ok(Outcome::Cancelled(2)),
                    KeyCode::Char('c') if shortcut => return Ok(Outcome::Cancelled(2)),
                    KeyCode::Char('s') if shortcut => {
                        editor.revealed = false;
                        match editor.secret() {
                            Err(error) => {
                                feedback = format!("Cannot save: {error}. Edit the value and press Ctrl+S to retry.");
                            }
                            Ok(secret) => {
                                terminal.render(
                                    credential,
                                    &editor,
                                    "Saving to the credential store...",
                                )?;
                                // The provider's synchronous write is the commit boundary.
                                // A pending cancellation is handled after a failed write;
                                // an observed successful write is always reported as stored.
                                match provider.store(secret) {
                                    Ok(()) => return Ok(Outcome::Stored),
                                    Err(error) => feedback = format!(
                                        "Save failed. Your input is retained. Fix the store, then press Ctrl+S to retry, or keep editing. {error}"
                                    ),
                                }
                            }
                        }
                    }
                    KeyCode::F(2) => editor.revealed = !editor.revealed,
                    KeyCode::Char('r') if shortcut => editor.revealed = !editor.revealed,
                    KeyCode::Char('u') if shortcut => {
                        editor.clear();
                        feedback = "Input cleared. Type or paste a new value.".into();
                    }
                    KeyCode::Left => editor.left(),
                    KeyCode::Right => editor.right(),
                    KeyCode::Up => editor.vertical(false),
                    KeyCode::Down => editor.vertical(true),
                    KeyCode::Home => editor.home(control),
                    KeyCode::End => editor.end(control),
                    KeyCode::Char('a') if shortcut => editor.home(true),
                    KeyCode::Char('e') if shortcut => editor.end(true),
                    KeyCode::Backspace => {
                        editor.backspace();
                        feedback = "Edited. Press Ctrl+S to save.".into();
                    }
                    KeyCode::Delete => {
                        editor.delete();
                        feedback = "Edited. Press Ctrl+S to save.".into();
                    }
                    KeyCode::Enter | KeyCode::Char('j')
                        if key.code == KeyCode::Enter || shortcut =>
                    {
                        editor.insert("\n");
                        feedback = "Newline inserted. Ctrl+S saves; Enter adds a newline.".into();
                    }
                    KeyCode::Tab => {
                        editor.insert("\t");
                        feedback = "Tab inserted. Press Ctrl+S to save.".into();
                    }
                    KeyCode::Char(value)
                        if !control || key.modifiers.contains(KeyModifiers::ALT) =>
                    {
                        let mut bytes = Zeroizing::new([0_u8; 4]);
                        editor.insert(value.encode_utf8(&mut *bytes));
                        feedback = "Editing. Press Ctrl+S to save.".into();
                    }
                    _ => continue,
                }
            }
        }
        terminal.render(credential, &editor, &feedback)?;
    }
}

struct Terminal {
    console: Console,
    screen: bool,
}

impl Terminal {
    fn open() -> io::Result<Self> {
        let mut terminal = Self {
            console: Console::open()?,
            screen: false,
        };

        // A partially written setup sequence still needs a cleanup attempt.
        terminal.screen = true;
        terminal.write(ENTER_SCREEN)?;
        Ok(terminal)
    }

    fn write(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.console.write(bytes)
    }

    fn read(&mut self, timeout: Duration) -> io::Result<Option<Input>> {
        self.console.read(timeout)
    }

    fn size(&self) -> io::Result<(u16, u16)> {
        self.console.size()
    }

    fn render(
        &mut self,
        credential: &CredentialDef,
        editor: &Editor,
        feedback: &str,
    ) -> io::Result<()> {
        use std::fmt::Write as _;
        let (columns, rows) = self.size()?;
        let width = usize::from(columns.saturating_sub(1)).max(1);
        let mut frame = Zeroizing::new(String::new());
        frame.push_str("\x1b[?25l\x1b[H\x1b[2J");
        if rows < 12 || columns < 32 {
            append_line(&mut frame, "Resize to at least 32 x 12 to edit.", width, 0);
            append_line(&mut frame, "Esc cancels; input is retained.", width, 1);
            return self.write(frame.as_bytes());
        }
        append_line(
            &mut frame,
            &format!("Credential: {}", credential.name),
            width,
            0,
        );
        append_line(&mut frame, &credential.description, width, 1);
        let store = if cfg!(target_os = "macos") {
            "macOS Keychain"
        } else if cfg!(windows) {
            "Windows Credential Manager"
        } else {
            "System credential store"
        };
        append_line(&mut frame, &format!("Save to: {store}"), width, 2);
        append_line(
            &mut frame,
            if editor.revealed {
                "Credential value: Visible"
            } else {
                "Credential value: Hidden"
            },
            width,
            3,
        );
        let stats = format!(
            "{} characters | {} bytes | {}",
            editor.character_count(),
            editor.byte_count(),
            if editor.has_line_breaks() {
                "Contains line breaks"
            } else {
                "Single line"
            }
        );
        append_line(&mut frame, &stats, width, 5);
        append_line(
            &mut frame,
            if editor.revealed {
                "Visible text can be recorded."
            } else {
                "Enter: Newline | Ctrl+C: Cancel"
            },
            width,
            6,
        );
        let help_row = usize::from(rows) - 3;
        let hint = if editor.revealed {
            "F2/Ctrl+R: Hide | Ctrl+U: Clear"
        } else {
            "F2/Ctrl+R: Show | Ctrl+U: Clear"
        };
        append_line(&mut frame, hint, width, help_row);
        append_line(
            &mut frame,
            "Arrows/Home/End move; Del edits",
            width,
            help_row + 1,
        );
        append_line(
            &mut frame,
            "Ctrl+S: Save | Esc: Cancel",
            width,
            help_row + 2,
        );
        for (index, line) in wrapped(feedback, width)
            .iter()
            .take(help_row.saturating_sub(7))
            .enumerate()
        {
            append_safe_line(&mut frame, line, width, index + 7);
        }
        let (value, cursor) = editor.viewport(width.saturating_sub(2));
        // Reserve before the first revealed byte so subsequent formatting
        // cannot reallocate a frame that already contains candidate contents.
        frame.reserve(value.len() + 100);
        frame.push_str("\x1b[5;1H> ");
        frame.push_str(&value);
        // The candidate viewport is the only part of a frame that may hold a
        // revealed secret. Both it and this complete frame have zeroizing owners.
        let _ = write!(frame, "\x1b[5;{}H\x1b[?25h", cursor + 3);
        self.write(frame.as_bytes())
    }

    fn restore(&mut self) -> io::Result<()> {
        let screen = if self.screen {
            self.write(LEAVE_SCREEN)
        } else {
            Ok(())
        };
        if screen.is_ok() {
            self.screen = false;
        }
        let modes = self.console.restore();
        screen.and(modes)
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        let _ = self.restore();
    }
}

fn append_line(frame: &mut String, text: &str, width: usize, row: usize) {
    append_safe_line(frame, &editor::escaped(text), width, row);
}

fn append_safe_line(frame: &mut String, text: &str, width: usize, row: usize) {
    use std::fmt::Write as _;
    let _ = write!(frame, "\x1b[{};1H", row + 1);
    let mut cells = 0;
    for grapheme in text.graphemes(true) {
        let next = UnicodeWidthStr::width(grapheme);
        if cells + next > width {
            break;
        }
        frame.push_str(grapheme);
        cells += next;
    }
}

fn wrapped(text: &str, width: usize) -> Vec<String> {
    let safe = editor::escaped(text);
    let mut lines = vec![String::new()];
    let mut cells = 0;
    for grapheme in safe.graphemes(true) {
        let next = UnicodeWidthStr::width(grapheme);
        if cells + next > width {
            lines.push(String::new());
            cells = 0;
        }
        if let Some(line) = lines.last_mut() {
            line.push_str(grapheme);
        }
        cells += next;
    }
    lines
}
