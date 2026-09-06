//! The `update` command: report or install a newer release over the
//! running binary and its installed agent-skill copies.

use std::io;

use clap::Args;
use serde_json::{json, Value};

use agentenv::error::AppError;
use agentenv::update::{self, Options, Outcome, Report, Status};

use super::Output;

#[derive(Debug, Args)]
pub struct UpdateArgs {
    /// Report the installed and latest versions without changing anything.
    #[arg(long)]
    pub check: bool,
    /// Install this release tag instead of the latest release; may downgrade.
    #[arg(long, value_name = "TAG")]
    pub version: Option<String>,
    /// Reinstall even when the installed version already matches.
    #[arg(long)]
    pub force: bool,
    /// Update the binary only; leave installed agent skills as they are.
    #[arg(long)]
    pub no_skill: bool,
}

pub(super) fn execute(
    args: UpdateArgs,
    json: bool,
    env: &impl Fn(&str) -> Option<String>,
) -> Result<Output, AppError> {
    let options = Options {
        tag: args.version,
        force: args.force,
        skip_skill: args.no_skill,
    };
    // Progress goes straight to stderr so a long download is visible while
    // it happens; stdout carries the final report.
    let mut progress: Box<dyn io::Write> = if json {
        Box::new(io::sink())
    } else {
        Box::new(io::stderr())
    };

    if args.check {
        let status = update::check(&options, env, &mut progress)?;
        let stdout = if json {
            json_line(status_json(&status))
        } else {
            status_text(&status)
        };
        return Ok(Output::success(stdout, String::new()));
    }

    match update::apply(&options, env, &mut progress)? {
        Outcome::UpToDate(status) => {
            let stdout = if json {
                json_line(status_json(&status))
            } else {
                format!(
                    "agentenv {} is already the latest release.\n",
                    status.current
                )
            };
            Ok(Output::success(stdout, String::new()))
        }
        Outcome::Updated(report) => {
            let stdout = if json {
                json_line(report_json(&report))
            } else {
                report_text(&report)
            };
            let status = if report.skill_failures.is_empty() {
                0
            } else {
                7
            };
            Ok(Output {
                stdout,
                stderr: String::new(),
                status,
            })
        }
    }
}

fn status_text(status: &Status) -> String {
    let mut text = format!(
        "installed: agentenv {}\nlatest:    agentenv {} ({})\n",
        status.current, status.available, status.target
    );
    if status.is_newer() {
        text.push_str("Run 'agentenv update' to install it.\n");
    } else if status.is_current() {
        text.push_str("The installed version is up to date.\n");
    } else {
        text.push_str(&format!(
            "The installed version is newer than the latest release; pass --version {} to move to it.\n",
            status.tag
        ));
    }
    text
}

fn status_json(status: &Status) -> Value {
    json!({
        "current": status.current.to_string(),
        "available": status.available.to_string(),
        "tag": status.tag,
        "target": status.target,
        "update_available": status.is_newer(),
        "binary": status.binary,
        "skills": status.skills,
    })
}

fn report_text(report: &Report) -> String {
    let mut text = format!(
        "Updated agentenv {} -> {} at {}\n",
        report.from,
        report.to,
        report.binary.display()
    );
    for skill in &report.skills {
        text.push_str(&format!(
            "Refreshed the agent skill at {}\n",
            skill.display()
        ));
    }
    for failure in &report.skill_failures {
        text.push_str(&format!(
            "The agent skill at {} was not refreshed: {}. Rerun 'agentenv update --force' after fixing it.\n",
            failure.path.display(),
            failure.error
        ));
    }
    text
}

fn report_json(report: &Report) -> Value {
    json!({
        "from": report.from.to_string(),
        "to": report.to.to_string(),
        "tag": report.tag,
        "binary": report.binary,
        "skills": report.skills,
        "skill_failures": report
            .skill_failures
            .iter()
            .map(|failure| json!({ "path": failure.path, "error": failure.error }))
            .collect::<Vec<_>>(),
    })
}

fn json_line(value: Value) -> String {
    serde_json::to_string(&value).expect("update JSON views are serializable") + "\n"
}
