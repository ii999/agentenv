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

/// Runs agentenv against `config` with a scrubbed environment.
///
/// The child starts from an empty environment plus PATH, then receives
/// `AGENTENV_FILE` and whatever `envs` supplies, so config resolution
/// depends only on what a test states. Before returning, every captured
/// invocation is checked for planted secrets: a leak fails the test that
/// caused it, whatever that test was asserting.
pub fn run_ac(config: &Path, envs: &[(&str, &str)], args: &[&str]) -> Run {
    let mut command = Command::cargo_bin("agentenv")
        .expect("the agentenv binary is built before integration tests run");

    command.env_clear();
    for name in PASSTHROUGH_ENV {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
    command.env("AGENTENV_FILE", config);
    for (key, value) in envs {
        command.env(key, value);
    }
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
