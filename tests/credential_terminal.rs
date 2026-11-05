//! Terminal behavior is exercised through a PTY with synthetic credentials and
//! the test-only store. No test accesses the platform credential store.
#![cfg(all(unix, feature = "test-keychain"))]

use std::fs;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize, SlavePty};
use tempfile::TempDir;

const SAVE: &[u8] = b"\x13";
const REVEAL: &[u8] = b"\x1bOQ";

#[test]
fn hidden_paste_preserves_complete_unicode_and_control_bytes_until_explicit_save() {
    let mut session = Session::new(false, false);
    let value = "  pty-token-49b8e-日本語-🙂-e\u{301}\r\nsecond\tline\x13\x03\n";
    session.paste(value);
    session.wait_for("Pasted");
    assert!(
        !session.store.exists(),
        "pasted Save/Cancel bytes and newlines are data"
    );
    session.send(SAVE);
    session.finish(0);
    assert_eq!(session.stored(), value.as_bytes());
    assert!(!session.text().contains("pty-token-49b8e"));
    assert!(!session.text().contains("second"));
    session.assert_restored();
}

#[test]
fn malformed_utf8_paste_keeps_the_candidate_and_accepts_literal_replacement_characters() {
    let mut session = Session::new(false, false);
    let original = br#"[{"service":"agentenv-test","account":"editor","value":[111,108,100]}]"#;
    fs::write(&session.store, original).unwrap();
    session.paste("kept-");
    session.wait_for("Pasted");
    let rejected = session.transcript.len();
    session.send(b"\x1b[200~a\xffb\x1b[201~");
    session.wait_since(rejected, "Paste rejected");
    assert_eq!(fs::read(&session.store).unwrap(), original);
    let pasted = session.transcript.len();
    session.paste("\u{fffd}");
    session.wait_since(pasted, "Pasted 1 characters");
    session.send(SAVE);
    session.finish(0);
    assert_eq!(session.stored(), "kept-\u{fffd}".as_bytes());
    assert!(!session.text().contains("kept-"));
    session.assert_restored();
}

#[test]
fn invalid_unframed_terminal_bytes_fail_without_saving_and_restore_modes() {
    let mut session = Session::new(false, false);
    session.send(b"candidate\xff");
    session.finish(1);
    assert!(session.text().contains("terminal input is not valid UTF-8"));
    assert!(!session.store.exists());
    assert!(!session.text().contains("candidate"));
    session.assert_restored();
}

#[test]
fn typing_and_unframed_newlines_never_submit() {
    let mut session = Session::new(false, false);
    session.send(b"first\rsecond\r");
    session.wait_for("Newline inserted");
    assert!(!session.store.exists());
    session.send(SAVE);
    session.finish(0);
    assert_eq!(session.stored(), b"first\nsecond\n");
    assert!(!session.text().contains("first"));
    assert!(!session.text().contains("second"));
}

#[test]
fn reveal_escapes_terminal_controls_and_hiding_removes_the_preview() {
    let mut session = Session::new(false, false);
    let value = "key\x1b]0;owned\x07\n\u{202e}tail";
    session.paste(value);
    session.wait_for("Pasted");
    assert!(!session.text().contains("owned"));
    let revealed = session.transcript.len();
    session.send(REVEAL);
    session.wait_since(revealed, "\\u{1b}]0;owned\\u{7}\\n\\u{202e}tail");
    assert!(
        !session.text().contains("\x1b]0;owned"),
        "input cannot emit a terminal command"
    );
    let hidden = session.transcript.len();
    session.send(REVEAL);
    session.wait_since(hidden, "Hidden");
    assert!(!String::from_utf8_lossy(&session.transcript[hidden..]).contains("owned"));
    session.send(SAVE);
    session.finish(0);
    assert_eq!(session.stored(), value.as_bytes());
}

#[test]
fn linux_f2_toggles_visibility_without_appending_a_character() {
    let mut session = Session::new(false, false);
    session.paste("linux-key-fixture");
    session.wait_for("Pasted");
    session.send(b"\x1b[[");
    session.send(b"B");
    session.wait_for("linux-key-fixture");
    let hidden = session.transcript.len();
    session.send(b"\x1b[[A\x1b[[C\x1b[[D\x1b[[E\x1b[[B");
    session.wait_since(hidden, "Hidden");
    session.send(SAVE);
    session.finish(0);
    assert_eq!(session.stored(), b"linux-key-fixture");
}

#[test]
fn cursor_editing_deletes_whole_graphemes_and_inserts_at_the_cursor() {
    let mut session = Session::new(false, false);
    session.paste("ab🙂e\u{301}中");
    session.wait_for("Pasted");
    session.send(b"\x1b[D\x1b[D\x7fZ\x1b[3~\x1b[F\x7f\x1b[H");
    session.send("起".as_bytes());
    session.send(b"\x1b[C\x1b[3~");
    // Move between logical lines at the nearest available grapheme column.
    session.send(b"\x05\rsecond\x1b[A\x7f\x1b[BX\x1b[F\x1b[H\x1b[3~");
    session.send(SAVE);
    session.finish(0);
    assert_eq!(session.stored(), "起a\neXcond".as_bytes());
}

#[test]
fn validation_and_storage_failures_keep_input_available_for_edit_and_retry() {
    let mut session = Session::new(true, false);
    session.send(SAVE);
    session.wait_for("the value is empty");
    session.paste("keep-this-input\0");
    session.wait_for("Pasted");
    let invalid = session.transcript.len();
    session.send(SAVE);
    session.wait_since(invalid, "contains a NUL byte");
    session.send(b"\x7f");
    session.send(SAVE);
    session.wait_for("Save failed. Your input is retained.");
    assert!(!session.store.exists());
    fs::create_dir(session.store.parent().unwrap()).unwrap();
    session.send(SAVE);
    session.finish(0);
    assert_eq!(session.stored(), b"keep-this-input");
    assert!(!session.text().contains("keep-this-input"));
    session.assert_restored();
}

#[test]
fn clear_and_cancel_preserve_the_previously_stored_value() {
    for cancellation in [b"\x03".as_slice(), b"\x1b".as_slice()] {
        let mut session = Session::new(false, false);
        let original = br#"[{"service":"agentenv-test","account":"editor","value":[111,108,100]}]"#;
        fs::write(&session.store, original).unwrap();
        session.paste("discarded-entry");
        session.wait_for("Pasted");
        let cleared = session.transcript.len();
        session.send(b"\x15");
        session.wait_for("Input cleared");
        session.wait_since(cleared, "0 characters");
        session.send(b"another-entry");
        session.send(cancellation);
        session.finish(130);
        assert_eq!(fs::read(&session.store).unwrap(), original);
        assert!(!session.text().contains("discarded-entry"));
        assert!(!session.text().contains("another-entry"));
        session.assert_restored();
    }
}

#[test]
fn termination_during_an_incomplete_paste_restores_terminal_modes() {
    for signal in [libc::SIGINT, libc::SIGTERM, libc::SIGHUP] {
        let mut session = Session::new(false, false);
        session.send(b"\x1b[200~unfinished-paste");
        let pid = session.child.process_id().unwrap() as i32;
        assert_eq!(unsafe { libc::kill(pid, signal) }, 0);
        session.finish((128 + signal) as u32);
        assert!(!session.store.exists());
        assert!(!session.text().contains("unfinished-paste"));
        session.assert_restored();
    }
}

#[test]
fn redirected_streams_do_not_receive_revealed_values_or_terminal_control_sequences() {
    let mut session = Session::new(false, true);
    session.paste("redirected-secret-fixture");
    session.wait_for("Pasted");
    session.send(REVEAL);
    session.wait_for("redirected-secret-fixture");
    session.send(SAVE);
    session.finish(0);
    let stdout = fs::read_to_string(session.directory.path().join("stdout")).unwrap();
    let stderr = fs::read_to_string(session.directory.path().join("stderr")).unwrap();
    assert_eq!(stdout, "Credential 'editor' stored.\n");
    assert_eq!(stderr, "");
    session.assert_restored();
}

#[test]
fn resize_and_visibility_changes_keep_long_unicode_input_editable() {
    let mut session = Session::new(false, false);
    let value = format!("{}🙂", "日本語".repeat(40));
    session.paste(&value);
    session.wait_for("Pasted");
    session.send(REVEAL);
    session.wait_for("Visible");
    let resized = session.transcript.len();
    session.master.resize(size(32, 12)).unwrap();
    session.send(REVEAL);
    session.wait_since(resized, "Hidden");
    session.send(b"\x7f");
    session.send(SAVE);
    session.finish(0);
    assert_eq!(session.stored(), "日本語".repeat(40).as_bytes());
}

struct Session {
    directory: TempDir,
    store: PathBuf,
    master: Box<dyn MasterPty + Send>,
    _slave: Box<dyn SlavePty + Send>,
    child: Box<dyn Child + Send + Sync>,
    writer: Box<dyn Write + Send>,
    reader: Receiver<Vec<u8>>,
    transcript: Vec<u8>,
    original: libc::termios,
}

impl Session {
    fn new(fail_store: bool, redirect: bool) -> Self {
        let directory = TempDir::new().unwrap();
        let config = directory.path().join("config.toml");
        fs::write(&config, "version = 1\n[credentials.editor]\ndescription = \"Terminal editor fixture.\"\nprovider = \"keychain\"\nservice = \"agentenv-test\"\naccount = \"editor\"\ninject_as = \"TEST_VALUE\"\n").unwrap();
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&config, fs::Permissions::from_mode(0o600)).unwrap();
        let store = directory.path().join(if fail_store {
            "missing/keychain.json"
        } else {
            "keychain.json"
        });
        let pty = native_pty_system().openpty(size(100, 24)).unwrap();
        let original = modes(pty.master.as_ref());
        let binary = assert_cmd::cargo::cargo_bin("agentenv");
        let mut command = if redirect {
            let mut command = CommandBuilder::new("/bin/sh");
            command.args([
                "-c",
                "exec \"$1\" credential set editor >\"$2\" 2>\"$3\"",
                "credential-input-test",
            ]);
            command.arg(&binary);
            command.arg(directory.path().join("stdout"));
            command.arg(directory.path().join("stderr"));
            command
        } else {
            let mut command = CommandBuilder::new(binary);
            command.args(["credential", "set", "editor"]);
            command
        };
        command.env_clear();
        if let Some(path) = std::env::var_os("PATH") {
            command.env("PATH", path);
        }
        command.env("TERM", "xterm-256color");
        command.env("AGENTENV_FILE", &config);
        command.env("AGENTENV_TEST_KEYCHAIN", &store);
        command.env("AGENTENV_NO_PROJECT", "1");
        command.cwd(directory.path());
        let child = pty.slave.spawn_command(command).unwrap();
        let writer = pty.master.take_writer().unwrap();
        let mut output = pty.master.try_clone_reader().unwrap();
        let (sender, reader) = mpsc::channel();
        std::thread::spawn(move || {
            let mut buffer = [0_u8; 4096];
            while let Ok(count) = output.read(&mut buffer) {
                if count == 0 || sender.send(buffer[..count].to_vec()).is_err() {
                    break;
                }
            }
        });
        let mut session = Self {
            directory,
            store,
            master: pty.master,
            _slave: pty.slave,
            child,
            writer,
            reader,
            transcript: Vec::new(),
            original,
        };
        session.wait_for("Ctrl+S: Save");
        session
    }

    fn text(&self) -> std::borrow::Cow<'_, str> {
        String::from_utf8_lossy(&self.transcript)
    }

    fn send(&mut self, value: &[u8]) {
        self.writer.write_all(value).unwrap();
        self.writer.flush().unwrap();
    }

    fn paste(&mut self, value: &str) {
        self.send(b"\x1b[200~");
        self.send(value.as_bytes());
        self.send(b"\x1b[201~");
    }

    fn wait_for(&mut self, text: &str) {
        self.wait_since(0, text);
    }

    fn wait_since(&mut self, offset: usize, text: &str) {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if String::from_utf8_lossy(&self.transcript[offset..]).contains(text) {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "missing {text:?} in synthetic terminal output: {}",
                self.text()
            );
            if let Ok(chunk) = self.reader.recv_timeout(Duration::from_millis(20)) {
                self.transcript.extend_from_slice(&chunk);
            }
        }
    }

    fn finish(&mut self, expected: u32) {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                while let Ok(chunk) = self.reader.recv_timeout(Duration::from_millis(30)) {
                    self.transcript.extend_from_slice(&chunk);
                }
                assert_eq!(
                    status.exit_code(),
                    expected,
                    "synthetic terminal output: {}",
                    self.text()
                );
                return;
            }
            assert!(
                Instant::now() < deadline,
                "editor did not exit; synthetic terminal output: {}",
                self.text()
            );
            if let Ok(chunk) = self.reader.recv_timeout(Duration::from_millis(20)) {
                self.transcript.extend_from_slice(&chunk);
            }
        }
    }

    fn stored(&self) -> Vec<u8> {
        let records: serde_json::Value =
            serde_json::from_slice(&fs::read(&self.store).unwrap()).unwrap();
        serde_json::from_value(records[0]["value"].clone()).unwrap()
    }

    fn assert_restored(&self) {
        let current = modes(self.master.as_ref());
        assert_eq!(current.c_iflag, self.original.c_iflag);
        assert_eq!(current.c_oflag, self.original.c_oflag);
        assert_eq!(current.c_lflag, self.original.c_lflag);
        assert_eq!(current.c_cc, self.original.c_cc);
        assert!(self.text().contains("\x1b[?2004l"));
        assert!(self.text().contains("\x1b[?1049l"));
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

fn size(cols: u16, rows: u16) -> PtySize {
    PtySize {
        rows,
        cols,
        pixel_width: 0,
        pixel_height: 0,
    }
}

fn modes(master: &dyn MasterPty) -> libc::termios {
    let mut modes = std::mem::MaybeUninit::uninit();
    assert_eq!(
        unsafe { libc::tcgetattr(master.as_raw_fd().unwrap(), modes.as_mut_ptr()) },
        0
    );
    unsafe { modes.assume_init() }
}
