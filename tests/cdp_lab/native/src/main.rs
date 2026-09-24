//! Native Rust CDP filling client for the credential-fill Phase A lab.
//!
//! Implements the client contract in `tests/cdp_lab/README.md` directly over
//! the Chrome DevTools Protocol, with no Node.js or Playwright dependency.
//! The value is read from stdin and only ever leaves this process through
//! `Input.insertText` to the browser.

mod cdp;
mod fill;
mod outcome;
mod scripts;

use std::io::Read;
use std::process::ExitCode;
use std::time::Duration;

use clap::{Parser, ValueEnum};

use crate::outcome::{Outcome, Reason};

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum PageMatch {
    Exact,
    OriginPath,
}

#[derive(Parser, Debug)]
#[command(
    name = "cdp-fill-native",
    about = "Fill one element over CDP; value from stdin"
)]
pub struct Args {
    /// HTTP endpoint of the browser's remote debugging port, e.g. http://127.0.0.1:9222
    #[arg(long)]
    pub endpoint: String,
    /// URL of the page to fill.
    #[arg(long)]
    pub page_url: String,
    /// How to compare the page URL.
    #[arg(long, value_enum, default_value = "exact")]
    pub page_match: PageMatch,
    /// Browser context index: 0 is the default context; created contexts follow.
    #[arg(long)]
    pub context_index: Option<usize>,
    /// Strict chain of iframe selectors from the main frame.
    #[arg(long = "frame-selector")]
    pub frame_selectors: Vec<String>,
    /// CSS selector for the target element in the resolved frame.
    #[arg(long)]
    pub selector: String,
    /// Whole-operation deadline in milliseconds.
    #[arg(long, default_value_t = 10_000)]
    pub timeout_ms: u64,
    /// Lab-only hook: sleep after preparing the target, before revalidation.
    #[arg(long, default_value_t = 0)]
    pub delay_before_insert_ms: u64,
}

fn main() -> ExitCode {
    let args = match Args::try_parse() {
        Ok(args) => args,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::from(1);
        }
    };
    if args.timeout_ms == 0 {
        eprintln!("--timeout-ms must be positive");
        return ExitCode::from(1);
    }
    let mut value = String::new();
    if let Err(error) = std::io::stdin().read_to_string(&mut value) {
        eprintln!("cannot read the value from stdin: {error}");
        return ExitCode::from(1);
    }

    let deadline = fill::Deadline::start(Duration::from_millis(args.timeout_ms));
    let outcome = match fill::run(&args, &value, &deadline) {
        Ok(filled) => filled,
        Err(failure) => Outcome::error(failure),
    };
    drop(value);

    match serde_json::to_string(&outcome) {
        Ok(line) => println!("{line}"),
        Err(error) => {
            eprintln!("cannot encode the outcome: {error}");
            return ExitCode::from(8);
        }
    }
    if outcome.reason.is_none() {
        ExitCode::SUCCESS
    } else {
        if let Some(reason) = outcome.reason {
            eprintln!("credential-fill: {}: {}", reason.code(), outcome.message);
        }
        ExitCode::from(8)
    }
}

impl Reason {
    pub fn code(self) -> &'static str {
        match self {
            Reason::ConnectFailed => "connect-failed",
            Reason::VersionUnsupported => "version-unsupported",
            Reason::ContextAbsent => "context-absent",
            Reason::PageAbsent => "page-absent",
            Reason::PageAmbiguous => "page-ambiguous",
            Reason::FrameAbsent => "frame-absent",
            Reason::FrameAmbiguous => "frame-ambiguous",
            Reason::FrameInvalid => "frame-invalid",
            Reason::TargetAbsent => "target-absent",
            Reason::TargetAmbiguous => "target-ambiguous",
            Reason::TargetHidden => "target-hidden",
            Reason::TargetDisabled => "target-disabled",
            Reason::TargetReadonly => "target-readonly",
            Reason::TargetUnfillable => "target-unfillable",
            Reason::TargetChanged => "target-changed",
            Reason::Timeout => "timeout",
        }
    }
}
