//! Shared invocation harness for the agentenv integration suites.
//!
//! Each integration test file compiles as its own binary, so every suite
//! (`security_p1` today; `query_p1`, `credential_p2`, `security_p3` later)
//! pulls this module in with `mod helpers;`. Suites use different subsets of
//! the harness, which is why unused items are tolerated here.
#![allow(dead_code)]

use std::fs;
use std::path::{Path, PathBuf};

use assert_cmd::Command;
use tempfile::TempDir;

/// Planted secret values, one per fixture that carries a suspected plaintext
/// secret. Distinct and high-entropy so a leak is unambiguous and no value is
/// a substring of another.
pub const SENTINEL_PLAIN: &str = "sk-sentinel-a1-9f3k7q";
pub const SENTINEL_NESTED: &str = "sk-sentinel-b2-4m8t1x";
pub const SENTINEL_ARRAY: &str = "sk-sentinel-c3-7q2v5z";
pub const SENTINEL_UPPER: &str = "sk-sentinel-d4-1w6y3n";
pub const SENTINEL_PARSE: &str = "sk-sentinel-e5-8h2j4r";

/// Every planted secret. `run_ac` checks all of them on every invocation it
/// captures, so no suite can opt out of the no-secret invariant.
pub const SENTINELS: &[&str] = &[
    SENTINEL_PLAIN,
    SENTINEL_NESTED,
    SENTINEL_ARRAY,
    SENTINEL_UPPER,
    SENTINEL_PARSE,
];

/// Variables the child keeps. PATH is required to locate and execute the
/// binary; everything else — notably `AGENTENV_*`, `XDG_*` and `HOME` —
/// is dropped so the developer's own environment can never steer a test.
#[cfg(unix)]
const PASSTHROUGH_ENV: &[&str] = &["PATH"];
#[cfg(windows)]
const PASSTHROUGH_ENV: &[&str] = &["PATH", "SYSTEMROOT"];

/// The environment variable the project trust store's base directory is
/// derived from on this platform (`$XDG_STATE_HOME/agentenv/trust.toml` on
/// Unix-like systems, `%LOCALAPPDATA%\agentenv\trust.toml` on Windows).
/// Project fixtures set it so trust state lands inside the test tree.
#[cfg(unix)]
pub const STATE_BASE_ENV: &str = "XDG_STATE_HOME";
#[cfg(windows)]
pub const STATE_BASE_ENV: &str = "LOCALAPPDATA";

/// A path as project notices and diagnostics render it: canonicalized, which
/// adds the verbatim prefix and expands short names on Windows and resolves
/// symlinked temp roots on macOS. Raw fixture paths are not substrings of
/// that rendering on Windows, so assertions must compare in this form.
pub fn canonical_display(path: &Path) -> String {
    path.canonicalize()
        .unwrap_or_else(|error| panic!("cannot canonicalize {}: {error}", path.display()))
        .display()
        .to_string()
}

/// What one agentenv invocation produced.
pub struct Run {
    pub stdout: String,
    pub stderr: String,
    pub code: Option<i32>,
}

impl Run {
    /// Both channels joined, for assertions that do not care which one carried
    /// the message.
    pub fn combined(&self) -> String {
        format!("{}{}", self.stdout, self.stderr)
    }
}

/// Constructs an agentenv command with a scrubbed environment and a
/// test-controlled working directory.
///
/// This is the project-aware variant for tests that need discovery to run. It
/// intentionally leaves `AGENTENV_NO_PROJECT` unset; ordinary tests should use
/// [`run_ac`], which adds the bypass before executing the command.
pub fn command_with_project_discovery(config: &Path) -> Command {
    let mut command = Command::cargo_bin("agentenv")
        .expect("the agentenv binary is built before integration tests run");

    command.env_clear();
    for name in PASSTHROUGH_ENV {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
    command.current_dir(test_working_dir(config));
    command.env("AGENTENV_FILE", config);
    command
}

/// Runs agentenv against `config` with a scrubbed environment.
///
/// The child starts from an empty environment plus PATH, then receives
/// `AGENTENV_FILE`, the project-discovery bypass, and whatever `envs` supplies,
/// so config resolution depends only on what a test states. Before returning,
/// every captured invocation is checked for planted secrets: a leak fails the
/// test that caused it, whatever that test was asserting.
pub fn run_ac(config: &Path, envs: &[(&str, &str)], args: &[&str]) -> Run {
    let mut command = command_with_project_discovery(config);
    for (key, value) in envs {
        command.env(key, value);
    }
    command.env("AGENTENV_NO_PROJECT", "1");
    command.args(args);

    let output = command
        .output()
        .unwrap_or_else(|error| panic!("failed to run agentenv {args:?}: {error}"));

    let run = Run {
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        code: output.status.code(),
    };
    assert_no_sentinels(&run, args);
    run
}

/// Finds the nearest existing directory in the path controlled by the test.
///
/// Most tests pass a file already staged in a `TempDir`; the ancestor walk also
/// handles tests that pass a not-yet-created nested config path or a directory
/// path itself.
fn test_working_dir(config: &Path) -> &Path {
    config
        .ancestors()
        .find(|candidate| candidate.is_dir())
        .unwrap_or_else(|| {
            panic!(
                "test config path has no existing directory: {}",
                config.display()
            )
        })
}

/// Reports a leak by position in [`SENTINELS`] rather than by value, so the
/// secret stays out of the test log too.
fn assert_no_sentinels(run: &Run, args: &[&str]) {
    for (index, sentinel) in SENTINELS.iter().enumerate() {
        for (channel, text) in [("stdout", &run.stdout), ("stderr", &run.stderr)] {
            assert!(
                !text.contains(sentinel),
                "agentenv {args:?} leaked sentinel #{index} on {channel}"
            );
        }
    }
}

/// Asserts the process exit code, quoting the captured output — already known
/// to be free of planted secrets — so a failure explains itself.
pub fn assert_exit(run: &Run, expected: i32, context: &str) {
    assert_eq!(
        run.code,
        Some(expected),
        "{context}: expected exit {expected}\nstdout: {}\nstderr: {}",
        run.stdout,
        run.stderr
    );
}

/// Asserts the invocation said something on either channel.
pub fn assert_mentions(run: &Run, needle: &str, context: &str) {
    assert!(
        run.combined().contains(needle),
        "{context}: expected the output to mention {needle:?}\nstdout: {}\nstderr: {}",
        run.stdout,
        run.stderr
    );
}

/// Asserts the invocation kept something out of both channels.
pub fn assert_omits(run: &Run, needle: &str, context: &str) {
    assert!(
        !run.combined().contains(needle),
        "{context}: expected the output to omit {needle:?}\nstdout: {}\nstderr: {}",
        run.stdout,
        run.stderr
    );
}

/// Stages inline TOML content as a mode-0600 config file in a fresh temp
/// directory. The write suites use this instead of on-disk fixtures because
/// each test mutates its own copy.
pub fn staged_config(content: &str) -> (TempDir, PathBuf) {
    let dir = TempDir::new().expect("a temp dir for a staged config");
    let path = dir.path().join("config.toml");
    fs::write(&path, content).expect("the staged config is written");
    restrict_permissions(&path);
    (dir, path)
}

/// Reads a staged config back for content assertions.
pub fn read_config(path: &Path) -> String {
    fs::read_to_string(path).expect("the staged config reads")
}

/// A fixture config staged for use as a real config file.
///
/// The copy lives in a temp directory at mode 0600, which git cannot record
/// and which the Unix permission gate requires of any config that should load
/// cleanly. The temp directory is removed when the value is dropped, so tests
/// must hold it for as long as they invoke against it.
pub struct Fixture {
    _dir: TempDir,
    path: PathBuf,
}

impl Fixture {
    pub fn new(name: &str) -> Self {
        let source = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("fixtures")
            .join(name);
        let dir = TempDir::new().expect("failed to create a temp dir for a fixture config");
        let path = dir.path().join(name);

        fs::copy(&source, &path)
            .unwrap_or_else(|error| panic!("failed to stage fixture {name}: {error}"));
        restrict_permissions(&path);

        Self { _dir: dir, path }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

#[cfg(unix)]
fn restrict_permissions(path: &Path) {
    use std::os::unix::fs::PermissionsExt;

    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
        .unwrap_or_else(|error| panic!("failed to restrict {}: {error}", path.display()));
}

#[cfg(not(unix))]
fn restrict_permissions(_path: &Path) {}
