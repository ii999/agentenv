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
//! The binary depends on `std` only, so it builds in every profile the crate
//! builds in.

use std::env;
use std::fs;
use std::io::{self, Write};
use std::process;

fn main() {
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
