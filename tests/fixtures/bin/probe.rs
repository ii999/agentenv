//! `test-probe`: the target process used by the `run` integration tests.
//!
//! `run` replaces agentenv with its target, so the only way a test can
//! observe what was injected is to have the target report it. This binary
//! writes its argv and its environment to the file named by `TEST_PROBE_OUT`,
//! one tab-separated record per line:
//!
//! ```text
//! argv\t<argument>      once per argument, argv[0] first
//! env\t<NAME>=<VALUE>   once per variable, in the order the OS reports
//! ```
//!
//! It then prints `out` on stdout and `err` on stderr — the markers the
//! process-transparency criteria assert byte-for-byte — and exits with the
//! code named by `TEST_PROBE_EXIT` (default `0`). With `TEST_PROBE_OUT` unset
//! the probe writes no file, so it stays inert for tests that care only about
//! stdio or the exit status.
//!
//! The record format assumes single-line ASCII values, which is what the
//! suites inject; a value carrying a newline would split across records.
//! A probe report is the target's own file — the target's channels are outside
//! the no-secret boundary — so the injected values it contains are expected
//! there and nowhere else.
//!
//! With `TEST_PROBE_VERSION` set and `--version` as the only argument, the
//! probe instead prints `agentenv <TEST_PROBE_VERSION>` and exits 0, which
//! lets the `update` suite package it as a stand-in release binary.
//!
//! The binary depends on `std` only, so it builds in every profile the crate
//! builds in.

use std::env;
use std::fs;
use std::io::{self, Write};
use std::process;

fn main() {
    if fixture_mode() {
        return;
    }
    if let Some(version) = env::var_os("TEST_PROBE_VERSION") {
        if env::args_os().nth(1).is_some_and(|arg| arg == "--version") {
            println!("agentenv {}", version.to_string_lossy());
            process::exit(0);
        }
    }
    if let Some(destination) = env::var_os("TEST_PROBE_OUT") {
        let mut report = String::new();
        for argument in env::args_os() {
            report.push_str("argv\t");
            report.push_str(&argument.to_string_lossy());
            report.push('\n');
        }
        for (name, value) in env::vars_os() {
            report.push_str("env\t");
            report.push_str(&name.to_string_lossy());
            report.push('=');
            report.push_str(&value.to_string_lossy());
            report.push('\n');
        }
        if let Err(error) = fs::write(&destination, report) {
            panic!(
                "test-probe could not write {}: {error}",
                destination.to_string_lossy()
            );
        }
    }

    print!("out");
    io::stdout()
        .flush()
        .expect("test-probe could not flush stdout");
    eprint!("err");
    io::stderr()
        .flush()
        .expect("test-probe could not flush stderr");

    process::exit(requested_exit_code());
}

/// The code `TEST_PROBE_EXIT` asks for, or 0 when it is unset. An unparsable
/// value is a fault in the calling test, so it fails loudly instead of
/// silently standing in for success.
fn requested_exit_code() -> i32 {
    let Some(requested) = env::var_os("TEST_PROBE_EXIT") else {
        return 0;
    };
    let requested = requested.to_string_lossy().into_owned();

    requested.parse().unwrap_or_else(|error| {
        panic!("TEST_PROBE_EXIT={requested:?} is not an exit code: {error}")
    })
}

// Synthetic provider/askpass subprocesses: no shell or platform-specific
// scripting runtime is needed to exercise cancellation on Windows.
fn fixture_mode() -> bool {
    let args: Vec<String> = env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("--resolver-fixture") => {
            match args.get(1).map(String::as_str) {
                Some("value") => {
                    print!("{}", args[2]);
                }
                Some("repeat") => {
                    print!("{}", "x".repeat(args[2].parse().unwrap()));
                }
                Some("delayed") => {
                    fs::write(&args[3], b"resolving").unwrap();
                    std::thread::sleep(std::time::Duration::from_millis(1500));
                    print!("{}", args[2]);
                }
                Some("failure") => {
                    eprint!("SYNTHETIC-PROVIDER-ERROR");
                    process::exit(1);
                }
                Some("sleep") => {
                    fs::write(&args[2], process::id().to_string()).unwrap();
                    std::thread::sleep(std::time::Duration::from_secs(60));
                }
                Some("descendant") => {
                    let mut child = process::Command::new(env::current_exe().unwrap())
                        .args(["--resolver-fixture", "sleep", &args[2]])
                        .stdin(process::Stdio::null())
                        .stdout(process::Stdio::null())
                        .stderr(process::Stdio::null())
                        .spawn()
                        .unwrap();
                    let _ = child.wait();
                }
                _ => process::exit(2),
            }
            io::stdout().flush().unwrap();
            true
        }
        Some("--askpass-fixture") => {
            let output = process::Command::new(env::var_os("SSH_ASKPASS").unwrap())
                .arg(&args[1])
                .stdin(process::Stdio::null())
                .output()
                .unwrap();
            io::stdout().write_all(&output.stdout).unwrap();
            process::exit(output.status.code().unwrap_or(1));
        }
        _ => false,
    }
}
