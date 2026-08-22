//! Integration coverage for Phase-2 credential resolution and storage.
//!
//! The whole suite requires the `test-keychain` feature: without it the
//! binary's keychain provider reaches the user's real credential store, so
//! these tests must not run at all in an unfeatured build.
#![cfg(feature = "test-keychain")]

mod helpers;

use std::fs;
#[cfg(unix)]
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
#[cfg(unix)]
use std::sync::mpsc;
#[cfg(unix)]
use std::time::{Duration, Instant};

use assert_cmd::Command;
use helpers::{assert_exit, assert_mentions, assert_omits, run_ac, Run, SENTINELS};
#[cfg(unix)]
use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use tempfile::TempDir;

const COMMAND_SENTINEL: &str = "credential-p2-command-sentinel-7x4m";

#[test]
fn check_reports_env_and_missing_keychain_without_exposing_values() {
    let config = ConfigFile::new(&keychain_and_env_config());

    let env_missing = run_ac(config.path(), &[], &["credential", "check", "company_llm"]);
    assert_exit(&env_missing, 4, "an unset env credential fails resolution");
    assert_mentions(
        &env_missing,
        "COMPANY_LLM_TOKEN",
        "the missing environment variable is named",
    );

    let keychain_missing = run_ac(
        config.path(),
        &[(
            "AGENT_CONTEXT_TEST_KEYCHAIN",
            config.store_path().to_str().unwrap(),
        )],
        &["credential", "check", "openai_personal"],
    );
    assert_exit(
        &keychain_missing,
        4,
        "a missing keychain item fails resolution",
    );
    assert_mentions(&keychain_missing, "agent-context", "the service is named");
    assert_mentions(&keychain_missing, "openai-personal", "the account is named");
}

#[test]
fn command_provider_captures_stdout_without_shell_expansion_or_output_leaks() {
    let directory = TempDir::new().expect("a temp directory is available");
    let recorder = directory.path().join("argv.txt");
    let script = executable_script(
        &directory,
        "argv-recorder",
        "#!/bin/sh\nprintf '%s' \"$2\" > \"$1\"\nprintf 'credential-p2-command-sentinel-7x4m\\n'\n",
    );
    let config = ConfigFile::new(&command_config(
        &script,
        &[recorder.to_str().unwrap(), "$HOME"],
    ));

    let run = run_ac(
        config.path(),
        &[],
        &["credential", "check", "command_value"],
    );
    assert_exit(&run, 0, "a successful command credential resolves");
    assert_mentions(&run, "available", "success is reported");
    assert_omits(
        &run,
        COMMAND_SENTINEL,
        "the captured command value never reaches agent-context output",
    );
    assert_eq!(
        fs::read_to_string(recorder).expect("the provider recorded its argv"),
        "$HOME",
        "the direct process invocation never expands shell variables"
    );
}

#[test]
fn command_provider_failures_redact_captured_candidate_bytes() {
    let directory = TempDir::new().expect("a temp directory is available");
    let cases = [
        (
            "sentinel-exit-1",
            "#!/bin/sh\nprintf 'credential-p2-command-sentinel-7x4m\\n'\nexit 1\n",
            "exited unsuccessfully",
        ),
        (
            "newline-only",
            "#!/bin/sh\nprintf '\\n'\n",
            "no output or only whitespace",
        ),
        (
            "invalid-utf8",
            "#!/bin/sh\nprintf '\\377credential-p2-command-sentinel-7x4m\\n'\n",
            "not valid UTF-8",
        ),
        (
            "nul-byte",
            "#!/bin/sh\nprintf 'a\\000b\\n'\n",
            "contains a NUL byte",
        ),
    ];
    for (name, body, expected) in cases {
        let script = executable_script(&directory, name, body);
        let config = ConfigFile::new(&command_config(&script, &[]));
        let run = run_ac(
            config.path(),
            &[],
            &["credential", "check", "command_value"],
        );
        assert_exit(&run, 4, name);
        assert_mentions(&run, expected, name);
        assert_omits(&run, COMMAND_SENTINEL, name);
    }
}

#[test]
fn keychain_set_round_trips_exact_piped_bytes_and_rejects_external_providers() {
    let config = ConfigFile::new(&keychain_and_env_config());
    let store = config.store_path();
    let empty = run_with_input(
        config.path(),
        &[("AGENT_CONTEXT_TEST_KEYCHAIN", store.to_str().unwrap())],
        &["credential", "set", "openai_personal"],
        b"\n",
    );
    assert_exit(&empty, 1, "an empty piped keychain value is rejected");
    assert_mentions(&empty, "empty", "the rejected input is explained");

    let set = run_with_input(
        config.path(),
        &[("AGENT_CONTEXT_TEST_KEYCHAIN", store.to_str().unwrap())],
        &["credential", "set", "openai_personal"],
        b"hunter2\n",
    );
    assert_exit(&set, 0, "a piped keychain value is stored");
    assert_omits(&set, "hunter2", "set never echoes a secret");

    let records: serde_json::Value =
        serde_json::from_slice(&fs::read(store).expect("the test keychain file was written"))
            .expect("the test keychain store is valid JSON");
    assert_eq!(
        records[0]["value"],
        serde_json::json!([104, 117, 110, 116, 101, 114, 50]),
        "the test store retains the exact bytes after one newline is stripped"
    );

    let check = run_ac(
        config.path(),
        &[("AGENT_CONTEXT_TEST_KEYCHAIN", store.to_str().unwrap())],
        &["credential", "check", "openai_personal"],
    );
    assert_exit(&check, 0, "the stored value resolves");
    assert_omits(&check, "hunter2", "check never prints a resolved secret");

    let env_set = run_ac(config.path(), &[], &["credential", "set", "company_llm"]);
    assert_exit(&env_set, 1, "env credentials cannot be written by the CLI");
    assert_mentions(
        &env_set,
        "managed externally",
        "the alternate management path is explained",
    );
}

#[test]
fn keychain_set_reports_test_store_write_failures() {
    let config = ConfigFile::new(&keychain_and_env_config());
    let store_path = config
        .store_path()
        .parent()
        .expect("the test store has a parent")
        .join("missing-store-directory")
        .join("test-keychain.json");
    let run = run_with_input(
        config.path(),
        &[("AGENT_CONTEXT_TEST_KEYCHAIN", store_path.to_str().unwrap())],
        &["credential", "set", "openai_personal"],
        b"hunter2\n",
    );
    assert_exit(
        &run,
        4,
        "a test-store write failure is a credential failure",
    );
    assert_mentions(
        &run,
        "cannot write test keychain store",
        "the store failure is named",
    );
    assert_omits(&run, "hunter2", "a failed write never echoes the value");
}

#[test]
fn unknown_credential_lists_defined_names_with_name_resolution_exit_code() {
    let config = ConfigFile::new(&keychain_and_env_config());
    for action in ["check", "set"] {
        let run = run_ac(config.path(), &[], &["credential", action, "nosuch"]);
        assert_exit(
            &run,
            3,
            "unknown credential has a name-resolution exit code",
        );
        assert_mentions(&run, "company_llm", "defined credentials are listed");
        assert_mentions(&run, "openai_personal", "defined credentials are listed");
    }
}

#[test]
fn credential_actions_reject_json_output() {
    let config = ConfigFile::new(&keychain_and_env_config());
    for action in ["check", "set"] {
        let run = run_ac(
            config.path(),
            &[],
            &["credential", action, "openai_personal", "--json"],
        );
        assert_exit(&run, 1, "credential actions do not support JSON output");
        assert_mentions(
            &run,
            "does not support --json",
            "the unsupported flag is explained",
        );
    }
}

#[cfg(unix)]
#[test]
fn interactive_set_does_not_echo_the_typed_secret() {
    const TYPED_VALUE: &str = "pty-secret-49b8e";
    let config = ConfigFile::new(&keychain_and_env_config());
    let pty = native_pty_system()
        .openpty(PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        })
        .expect("a pseudo-terminal is available");
    let binary = assert_cmd::cargo::cargo_bin("agent-context");
    let mut command = CommandBuilder::new(binary);
    command.env_clear();
    if let Some(path) = std::env::var_os("PATH") {
        command.env("PATH", path);
    }
    command.env("AGENT_CONTEXT_FILE", config.path());
    command.env("AGENT_CONTEXT_TEST_KEYCHAIN", config.store_path());
    command.args(["credential", "set", "openai_personal"]);

    let mut child = pty
        .slave
        .spawn_command(command)
        .expect("the credential command starts in a pseudo-terminal");
    drop(pty.slave);

    let (sender, receiver) = mpsc::channel();
    let mut reader = pty
        .master
        .try_clone_reader()
        .expect("the pseudo-terminal is readable");
    std::thread::spawn(move || {
        let mut buffer = [0_u8; 256];
        while let Ok(read) = reader.read(&mut buffer) {
            if read == 0 || sender.send(buffer[..read].to_vec()).is_err() {
                break;
            }
        }
    });

    let prompt_deadline = Instant::now() + Duration::from_secs(3);
    let mut transcript = Vec::new();
    while Instant::now() < prompt_deadline {
        match receiver.recv_timeout(Duration::from_millis(50)) {
            Ok(chunk) => {
                transcript.extend_from_slice(&chunk);
                if String::from_utf8_lossy(&transcript).contains("Credential value:") {
                    break;
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    assert!(
        String::from_utf8_lossy(&transcript).contains("Credential value:"),
        "credential set did not prompt before the test timeout; terminal output: {}",
        String::from_utf8_lossy(&transcript)
    );

    let settle_deadline = Instant::now() + Duration::from_millis(100);
    while Instant::now() < settle_deadline {
        match receiver.recv_timeout(Duration::from_millis(10)) {
            Ok(chunk) => transcript.extend_from_slice(&chunk),
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }

    let mut writer = pty
        .master
        .take_writer()
        .expect("the pseudo-terminal is writable");
    writer
        .write_all(format!("{TYPED_VALUE}\n").as_bytes())
        .expect("the typed value is written");
    writer.flush().expect("the typed value is flushed");
    drop(writer);

    let status = child.wait().expect("credential set exits");
    drop(pty.master);
    let output_deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < output_deadline {
        match receiver.recv_timeout(Duration::from_millis(50)) {
            Ok(chunk) => transcript.extend_from_slice(&chunk),
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }

    let terminal_output = String::from_utf8_lossy(&transcript);
    assert!(status.success(), "credential set failed: {terminal_output}");
    assert!(
        !terminal_output.contains(TYPED_VALUE),
        "credential set echoed the typed value in the terminal stream: {terminal_output}"
    );

    let check = run_ac(
        config.path(),
        &[(
            "AGENT_CONTEXT_TEST_KEYCHAIN",
            config.store_path().to_str().expect("test paths are UTF-8"),
        )],
        &["credential", "check", "openai_personal"],
    );
    assert_exit(&check, 0, "the interactively stored secret resolves");
    assert_omits(&check, TYPED_VALUE, "check never prints the stored secret");
}

fn keychain_and_env_config() -> String {
    r#"version = 1

[credentials.company_llm]
description = "Company LLM credential."
provider = "env"
name = "COMPANY_LLM_TOKEN"
inject_as = "OPENAI_API_KEY"

[credentials.openai_personal]
description = "Personal credential."
provider = "keychain"
service = "agent-context"
account = "openai-personal"
inject_as = "OPENAI_API_KEY"
"#
    .to_owned()
}

fn command_config(script: &Path, arguments: &[&str]) -> String {
    let mut argv = vec![toml_string(script.to_str().expect("test paths are UTF-8"))];
    argv.extend(arguments.iter().map(|argument| toml_string(argument)));
    format!(
        "version = 1\n\n[credentials.command_value]\ndescription = \"Command credential.\"\nprovider = \"command\"\nargv = [{}]\ninject_as = \"COMMAND_VALUE\"\n",
        argv.join(", ")
    )
}

fn toml_string(value: &str) -> String {
    serde_json::to_string(value).expect("test strings serialize")
}

fn executable_script(directory: &TempDir, name: &str, contents: &str) -> PathBuf {
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    let path = directory.path().join(name);
    fs::write(&path, contents).expect("the provider fixture is written");
    #[cfg(unix)]
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755))
        .expect("the provider fixture is executable");
    path
}

struct ConfigFile {
    _directory: TempDir,
    path: PathBuf,
    store: PathBuf,
}

impl ConfigFile {
    fn new(contents: &str) -> Self {
        #[cfg(unix)]
        use std::os::unix::fs::PermissionsExt;

        let directory = TempDir::new().expect("a temp directory is available");
        let path = directory.path().join("context.toml");
        fs::write(&path, contents).expect("the credential config is written");
        #[cfg(unix)]
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))
            .expect("the credential config permissions are restricted");
        let store = directory.path().join("test-keychain.json");
        Self {
            _directory: directory,
            path,
            store,
        }
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn store_path(&self) -> &Path {
        &self.store
    }
}

fn run_with_input(config: &Path, envs: &[(&str, &str)], args: &[&str], input: &[u8]) -> Run {
    let mut command =
        Command::cargo_bin("agent-context").expect("the agent-context binary is built");
    command.env_clear();
    #[cfg(unix)]
    const PASSTHROUGH_ENV: &[&str] = &["PATH"];
    #[cfg(windows)]
    const PASSTHROUGH_ENV: &[&str] = &["PATH", "SYSTEMROOT"];
    for name in PASSTHROUGH_ENV {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
    command.env("AGENT_CONTEXT_FILE", config);
    for (key, value) in envs {
        command.env(key, value);
    }
    command.args(args);
    command.write_stdin(input.to_vec());
    let output = command.output().expect("the command executes");
    let run = Run {
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        code: output.status.code(),
    };
    for (index, sentinel) in SENTINELS.iter().enumerate() {
        for (channel, text) in [("stdout", &run.stdout), ("stderr", &run.stderr)] {
            assert!(
                !text.contains(sentinel),
                "agent-context {args:?} leaked sentinel #{index} on {channel}"
            );
        }
    }
    run
}
