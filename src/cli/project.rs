//! Rendering and mutations for the `agentenv project` command group.

use std::path::Path;

use serde_json::{json, Value as JsonValue};

use agentenv::config::{resolve_in_entry, Config, Profile};
use agentenv::error::{AppError, Violation};
use agentenv::path::Segments;
use agentenv::project::{self, model::ProjectFileMeta, ProjectContext, UntrustedReason};
use agentenv::query::entry_table;

use super::{Output, ProjectArgs, ProjectCommand};

/// Executes a project subcommand. Project commands own discovery so the
/// top-level prelude can remain inert for this command group.
pub(super) fn execute(
    args: &ProjectArgs,
    profile_flag: Option<&str>,
    json_output: bool,
    env: &impl Fn(&str) -> Option<String>,
) -> Result<Output, AppError> {
    let cwd = current_dir()?;
    match &args.command {
        ProjectCommand::Status => status(&cwd, profile_flag, json_output, env),
        ProjectCommand::Allow => allow(&cwd, json_output, env),
        ProjectCommand::Revoke => revoke(&cwd, json_output, env),
    }
}

fn status(
    cwd: &Path,
    profile_flag: Option<&str>,
    json_output: bool,
    env: &impl Fn(&str) -> Option<String>,
) -> Result<Output, AppError> {
    let context = project::resolve(cwd, env)?;
    let config = Config::load(None, env).ok();
    let version = config.as_ref().map(|config| config.version);
    let report = StatusReport::from_context(&context, config.as_ref(), profile_flag, env);
    let status = report.exit_status(&context);
    let stdout = if json_output {
        json_stdout(report.json(version))
    } else {
        report.text(version)
    };

    Ok(Output {
        stdout,
        stderr: String::new(),
        status,
    })
}

fn allow(
    cwd: &Path,
    json_output: bool,
    env: &impl Fn(&str) -> Option<String>,
) -> Result<Output, AppError> {
    if json_output {
        return Err(AppError::Usage(
            "project allow does not support --json; run 'agentenv project status --json' for a machine-readable report"
                .to_owned(),
        ));
    }
    let outcome = project::allow(cwd, env)?;
    let stdout = if outcome.already_current {
        format!(
            "Project file {} is already approved for its current contents; its profile pin and requirements are active. Run `agentenv project status` to review them.\n",
            outcome.path.display()
        )
    } else {
        format!(
            "Approved project file {}; its profile pin and requirements are now active. Run `agentenv project status` to review them.\n",
            outcome.path.display()
        )
    };
    Ok(Output::success(stdout, String::new()))
}

fn revoke(
    cwd: &Path,
    json_output: bool,
    env: &impl Fn(&str) -> Option<String>,
) -> Result<Output, AppError> {
    if json_output {
        return Err(AppError::Usage(
            "project revoke does not support --json; run 'agentenv project status --json' for a machine-readable report"
                .to_owned(),
        ));
    }
    let outcome = project::revoke(cwd, env)?;
    let stdout = if outcome.record_existed {
        format!(
            "Revoked approval for project file {}; its profile pin and requirements are inactive. Run `agentenv project status` to review the file.\n",
            outcome.path.display()
        )
    } else {
        format!(
            "Project file {} had no approval record; its profile pin and requirements remain inactive. Run `agentenv project allow` to approve it.\n",
            outcome.path.display()
        )
    };
    Ok(Output::success(stdout, String::new()))
}

fn current_dir() -> Result<std::path::PathBuf, AppError> {
    std::env::current_dir().map_err(|error| {
        AppError::Config(vec![Violation {
            path: "working directory".to_owned(),
            message: format!(
                "could not determine the working directory ({error}); change to an accessible directory and retry"
            ),
        }])
    })
}

struct StatusReport {
    path: Option<String>,
    trust: Option<&'static str>,
    trust_reason: Option<String>,
    violations: Vec<Violation>,
    profile_pin: Option<String>,
    requirements: RequirementReport,
}

impl StatusReport {
    fn from_context(
        context: &ProjectContext,
        config: Option<&Config>,
        profile_flag: Option<&str>,
        env: &impl Fn(&str) -> Option<String>,
    ) -> Self {
        match context {
            ProjectContext::None => Self {
                path: None,
                trust: None,
                trust_reason: None,
                violations: Vec::new(),
                profile_pin: None,
                requirements: RequirementReport::not_checked(
                    "no project file discovered; create .agentenv.toml in the working directory or an ancestor, then run `agentenv project status`",
                ),
            },
            ProjectContext::Untrusted { path, reason, meta } => {
                Self::untrusted(path, reason, meta.as_ref())
            }
            ProjectContext::Trusted { path, meta } => Self {
                path: Some(path.display().to_string()),
                trust: Some("trusted"),
                trust_reason: None,
                violations: Vec::new(),
                profile_pin: meta.pin.as_ref().map(|pin| pin.name.clone()),
                requirements: RequirementReport::check(meta, config, profile_flag, env),
            },
        }
    }

    fn untrusted(path: &Path, reason: &UntrustedReason, meta: Option<&ProjectFileMeta>) -> Self {
        let (trust, trust_reason, violations, profile_pin, requirements) = match reason {
            UntrustedReason::New => (
                "untrusted-new",
                None,
                Vec::new(),
                meta.and_then(|meta| meta.pin.as_ref()).map(|pin| pin.name.clone()),
                RequirementReport::not_checked(
                    "the project file is untrusted; run `agentenv project allow` to approve its current contents, then run `agentenv project status`",
                ),
            ),
            UntrustedReason::Changed => (
                "untrusted-changed",
                None,
                Vec::new(),
                meta.and_then(|meta| meta.pin.as_ref()).map(|pin| pin.name.clone()),
                RequirementReport::not_checked(
                    "the project file changed after approval; review it, then run `agentenv project allow` to approve the current contents",
                ),
            ),
            UntrustedReason::Invalid(violations) => (
                "invalid",
                None,
                violations.clone(),
                None,
                RequirementReport::not_checked(
                    "the project file is invalid; fix the listed paths, then run `agentenv project allow`",
                ),
            ),
            UntrustedReason::StateUnavailable(detail) => (
                "unavailable",
                Some(detail.clone()),
                Vec::new(),
                None,
                RequirementReport::not_checked(
                    "project trust could not be determined; set the required state-location variables, then run `agentenv project status`",
                ),
            ),
        };
        Self {
            path: Some(path.display().to_string()),
            trust: Some(trust),
            trust_reason,
            violations,
            profile_pin,
            requirements,
        }
    }

    fn exit_status(&self, context: &ProjectContext) -> i32 {
        match context {
            ProjectContext::None => 0,
            ProjectContext::Untrusted { .. } => 5,
            ProjectContext::Trusted { meta, .. } if meta.requires.is_empty() => 0,
            ProjectContext::Trusted { .. }
                if !self.requirements.checked
                    || self
                        .requirements
                        .entries
                        .iter()
                        .any(|entry| !entry.satisfied) =>
            {
                6
            }
            ProjectContext::Trusted { .. } => 0,
        }
    }

    fn json(&self, version: Option<i64>) -> JsonValue {
        json!({
            "version": version,
            "project": {
                "discovered": self.path.is_some(),
                "path": self.path,
                "trust": self.trust,
                "trust_reason": self.trust_reason,
                "violations": self.violations.iter().map(|violation| json!({
                    "path": violation.path,
                    "message": violation.message,
                })).collect::<Vec<_>>(),
                "profile_pin": self.profile_pin,
                "requirements": self.requirements.json(),
            }
        })
    }

    fn text(&self, version: Option<i64>) -> String {
        let mut output = String::new();
        match (&self.path, self.trust) {
            (None, _) => output.push_str("Project file: none discovered\n"),
            (Some(path), Some(trust)) => {
                output.push_str(&format!("Project file: {path}\nTrust: {trust}\n"));
            }
            _ => unreachable!("a project report has either no path or a trust state"),
        }
        if let Some(reason) = &self.trust_reason {
            output.push_str(&format!("Trust reason: {reason}\n"));
        }
        output.push_str(&format!(
            "Configuration version: {}\n",
            version.map_or_else(|| "unavailable".to_owned(), |version| version.to_string())
        ));
        output.push_str(&format!(
            "Profile pin: {}\n",
            self.profile_pin.as_deref().unwrap_or("none")
        ));
        if !self.violations.is_empty() {
            output.push_str("Violations:\n");
            for violation in &self.violations {
                output.push_str(&format!("  {}: {}\n", violation.path, violation.message));
            }
        }
        output.push_str(&self.requirements.text());
        output
    }
}

struct RequirementReport {
    checked: bool,
    reason: Option<String>,
    profile: Option<String>,
    entries: Vec<RequirementEntry>,
}

impl RequirementReport {
    fn not_checked(reason: impl Into<String>) -> Self {
        Self {
            checked: false,
            reason: Some(reason.into()),
            profile: None,
            entries: Vec::new(),
        }
    }

    fn check(
        meta: &ProjectFileMeta,
        config: Option<&Config>,
        profile_flag: Option<&str>,
        env: &impl Fn(&str) -> Option<String>,
    ) -> Self {
        let Some(config) = config else {
            return Self::not_checked(
                "the user configuration is unavailable; create or repair it, then run `agentenv project status`",
            );
        };
        let env_profile = env("AGENTENV_PROFILE");
        let profile = match config.select_profile(
            profile_flag,
            env_profile.as_deref(),
            meta.pin.as_ref(),
        ) {
            Ok(profile) => profile,
            Err(error) => {
                return Self::not_checked(format!(
                    "requirements could not be checked because {error}; fix the selection and run `agentenv project status`"
                ));
            }
        };

        Self {
            checked: true,
            reason: None,
            profile: Some(profile.name.clone()),
            entries: meta
                .requires
                .iter()
                .map(|requirement| RequirementEntry::check(requirement, profile))
                .collect(),
        }
    }

    fn json(&self) -> JsonValue {
        json!({
            "checked": self.checked,
            "reason": self.reason,
            "profile": self.profile,
            "entries": self.entries.iter().map(RequirementEntry::json).collect::<Vec<_>>(),
        })
    }

    fn text(&self) -> String {
        if !self.checked {
            return format!(
                "Requirements: not checked — {}\n",
                self.reason.as_deref().unwrap_or("unknown reason")
            );
        }
        let mut output = format!(
            "Requirements: checked against profile {}\n",
            self.profile.as_deref().unwrap_or("unknown")
        );
        if self.entries.is_empty() {
            output.push_str("  No requirements declared.\n");
        }
        for entry in &self.entries {
            let status = if entry.satisfied {
                "satisfied"
            } else {
                "unsatisfied"
            };
            output.push_str(&format!("  {} — {status}: {}\n", entry.entry, entry.reason));
            for missing in &entry.missing {
                output.push_str(&format!("    Missing: {missing}\n"));
            }
        }
        output
    }
}

struct RequirementEntry {
    entry: String,
    reason: String,
    satisfied: bool,
    missing: Vec<String>,
}

impl RequirementEntry {
    fn check(requirement: &agentenv::project::model::Requirement, profile: &Profile) -> Self {
        let Ok(table) = entry_table(profile, &requirement.entry) else {
            return Self {
                entry: requirement.entry.clone(),
                reason: requirement.reason.clone(),
                satisfied: false,
                missing: vec![format!("entry {}", requirement.entry)],
            };
        };
        let missing = requirement
            .fields
            .iter()
            .filter(|field| {
                let segments = Segments::parse(field).expect(
                    "validated project requirement fields use the accepted segment grammar",
                );
                resolve_in_entry(table, &segments).is_none()
            })
            .cloned()
            .collect::<Vec<_>>();
        Self {
            entry: requirement.entry.clone(),
            reason: requirement.reason.clone(),
            satisfied: missing.is_empty(),
            missing,
        }
    }

    fn json(&self) -> JsonValue {
        json!({
            "entry": self.entry,
            "reason": self.reason,
            "satisfied": self.satisfied,
            "missing": self.missing,
        })
    }
}

fn json_stdout(value: JsonValue) -> String {
    serde_json::to_string(&value).expect("project status JSON is serializable") + "\n"
}
