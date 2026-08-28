//! Injection planning and transparent target-process launch for `run`.
//!
//! Planning deliberately completes before a provider is constructed or
//! resolved. That keeps injection conflicts side-effect free and confines
//! resolved secrets to the child environment assembled at launch time.

use std::convert::Infallible;
use std::ffi::OsString;
use std::process::Command;

use toml::{Table, Value};

use crate::config::validate::{resolve_in_entry, walk_entry_references};
use crate::config::{Config, CredentialDef, Profile};
use crate::credential::{provider_for, Secret};
use crate::error::AppError;
use crate::path::{single_entry_name, Segments};
use crate::query::{entry_table, scalar_text};

/// A complete, conflict-free set of values to inject into a child process.
pub struct InjectionPlan {
    injections: Vec<Injection>,
}

/// Which parent-environment variables reach the launched target.
///
/// Construct with [`EnvironmentMode::inherit`] or [`EnvironmentMode::pure`].
/// The pure constructor snapshots the parent environment once, reports keeps
/// missing from it, and carries the selection, so the report precedes
/// injection-conflict detection, credential resolution, and launch whatever
/// the run's outcome (SPEC-002).
pub struct EnvironmentMode(ModeInner);

enum ModeInner {
    /// The full parent environment, minus injection-overridden names
    /// (default `run` behavior).
    Inherit,
    /// The already-selected pure environment: curated platform base plus
    /// explicitly kept names, before injections (`run --pure`).
    Pure(Vec<(OsString, OsString)>),
}

impl EnvironmentMode {
    /// The default mode: the target inherits the full parent environment.
    pub fn inherit() -> Self {
        Self(ModeInner::Inherit)
    }

    /// The `--pure` mode. Snapshots the parent environment, selects the
    /// curated base plus `keep` carries, and writes-and-flushes one stderr
    /// line per keep missing from the parent before returning.
    pub fn pure(keep: Vec<String>) -> Result<Self, AppError> {
        let (selected, missing) = select_pure_environment(std::env::vars_os(), &keep);
        report_missing_keeps(&missing)?;
        Ok(Self(ModeInner::Pure(selected)))
    }
}

/// The closed list of parent-variable names a pure run carries. No name
/// reaches the child by prefix or pattern; unlisted names, including other
/// `LC_*` names, are excluded (SPEC-001).
#[cfg(not(windows))]
const PURE_BASE: &[&str] = &[
    "PATH",
    "HOME",
    "TMPDIR",
    "TERM",
    "USER",
    "LOGNAME",
    "SHELL",
    "LANG",
    "TZ",
    "XDG_CONFIG_HOME",
    "XDG_STATE_HOME",
    "AGENTENV_FILE",
    "AGENTENV_PROFILE",
    "AGENTENV_NO_PROJECT",
    "LC_ALL",
    "LC_COLLATE",
    "LC_CTYPE",
    "LC_MESSAGES",
    "LC_MONETARY",
    "LC_NUMERIC",
    "LC_TIME",
    "LC_ADDRESS",
    "LC_IDENTIFICATION",
    "LC_MEASUREMENT",
    "LC_NAME",
    "LC_PAPER",
    "LC_TELEPHONE",
];
#[cfg(windows)]
const PURE_BASE: &[&str] = &[
    "PATH",
    "PATHEXT",
    "SystemRoot",
    "SystemDrive",
    "windir",
    "ComSpec",
    "TEMP",
    "TMP",
    "USERPROFILE",
    "HOMEDRIVE",
    "HOMEPATH",
    "APPDATA",
    "LOCALAPPDATA",
    "ProgramData",
    "ProgramFiles",
    "ProgramFiles(x86)",
    "ProgramW6432",
    "CommonProgramFiles",
    "CommonProgramFiles(x86)",
    "CommonProgramW6432",
    "ALLUSERSPROFILE",
    "PUBLIC",
    "COMPUTERNAME",
    "USERNAME",
    "USERDOMAIN",
    "OS",
    "NUMBER_OF_PROCESSORS",
    "PROCESSOR_ARCHITECTURE",
    "AGENTENV_FILE",
    "AGENTENV_PROFILE",
    "AGENTENV_NO_PROJECT",
];

enum Injection {
    Credential {
        name: String,
        definition: CredentialDef,
        target: String,
    },
    Plain {
        source: String,
        target: String,
        value: String,
    },
}

impl Injection {
    fn source_name(&self) -> &str {
        match self {
            Self::Credential { name, .. } => name,
            Self::Plain { source, .. } => source,
        }
    }

    fn target(&self) -> &str {
        match self {
            Self::Credential { target, .. } | Self::Plain { target, .. } => target,
        }
    }

    fn same_identity(&self, other: &Self) -> bool {
        match (self, other) {
            (
                Self::Credential {
                    name: left_name,
                    target: left_target,
                    ..
                },
                Self::Credential {
                    name: right_name,
                    target: right_target,
                    ..
                },
            ) => left_name == right_name && same_environment_name(left_target, right_target),
            (Self::Plain { source: left, .. }, Self::Plain { source: right, .. }) => left == right,
            _ => false,
        }
    }
}

impl InjectionPlan {
    /// Collects, deduplicates, and conflict-checks the requested entries.
    /// Provider construction and resolution happen only in
    /// [`Self::resolve_and_launch`].
    pub fn build(cfg: &Config, profile: &Profile, entries: &[String]) -> Result<Self, AppError> {
        let mut injections = Vec::new();

        for argument in entries {
            let entry_name = single_entry_name(argument)?;
            let table = entry_table(profile, &entry_name)?;

            collect_references(cfg, table, &mut injections);
            collect_inject_values(&entry_name, table, &mut injections);
        }

        let mut deduplicated = Vec::new();
        for injection in injections {
            if deduplicated
                .iter()
                .any(|existing: &Injection| existing.same_identity(&injection))
            {
                continue;
            }
            if let Some(conflict) = deduplicated.iter().find(|existing: &&Injection| {
                same_environment_name(existing.target(), injection.target())
            }) {
                return Err(AppError::Injection(format!(
                    "injection conflict for environment variable '{}': '{}' and '{}' both target it; change an inject key or credential '?as=' target",
                    injection.target(),
                    conflict.source_name(),
                    injection.source_name()
                )));
            }
            deduplicated.push(injection);
        }

        Ok(Self {
            injections: deduplicated,
        })
    }

    /// Resolves every distinct credential once, builds a fresh child
    /// environment per `mode`, and launches the requested target. This
    /// function never mutates this process's environment.
    pub fn resolve_and_launch(
        self,
        cmd: Vec<String>,
        mode: EnvironmentMode,
    ) -> Result<Infallible, AppError> {
        let Some(program) = cmd.first().filter(|program| !program.is_empty()) else {
            return Err(run_usage_error());
        };

        let inherited: Vec<(OsString, OsString)> = match mode.0 {
            ModeInner::Inherit => std::env::vars_os().collect(),
            ModeInner::Pure(selected) => selected,
        };

        let mut resolved = Vec::<(String, Secret)>::new();
        for injection in &self.injections {
            let Injection::Credential {
                name, definition, ..
            } = injection
            else {
                continue;
            };
            if resolved.iter().any(|(known, _)| known == name) {
                continue;
            }
            resolved.push((name.clone(), provider_for(definition).resolve()?));
        }

        let mut environment: Vec<(OsString, OsString)> = inherited
            .into_iter()
            .filter(|(name, _)| {
                !self
                    .injections
                    .iter()
                    .any(|injection| same_os_environment_name(name, injection.target()))
            })
            .collect();
        for injection in &self.injections {
            let value = match injection {
                Injection::Credential { name, .. } => resolved
                    .iter()
                    .find(|(known, _)| known == name)
                    .expect("every planned credential is resolved before launch")
                    .1
                    .as_str(),
                Injection::Plain { value, .. } => value,
            };
            environment.push((OsString::from(injection.target()), OsString::from(value)));
        }

        launch(program, &cmd[1..], environment)
    }
}

fn collect_references(cfg: &Config, entry: &Table, injections: &mut Vec<Injection>) {
    walk_entry_references(entry, &mut |_, reference| {
        let Ok(reference) = reference else {
            return;
        };
        let definition = cfg
            .credential(&reference.name)
            .expect("validated credential references name defined credentials");
        injections.push(Injection::Credential {
            name: definition.name.clone(),
            definition: definition.clone(),
            target: reference
                .target_override
                .clone()
                .unwrap_or_else(|| definition.inject_as.clone()),
        });
    });
}

fn collect_inject_values(entry_name: &str, entry: &Table, injections: &mut Vec<Injection>) {
    let Some(inject) = entry.get("inject").and_then(Value::as_table) else {
        return;
    };
    for (target, source) in inject {
        let source_path = source
            .as_str()
            .expect("validated inject entries have string field paths");
        let segments =
            Segments::parse(source_path).expect("validated inject entries have valid field paths");
        let value = resolve_in_entry(entry, &segments)
            .expect("validated inject entries resolve inside their entry");
        injections.push(Injection::Plain {
            source: format!("{entry_name}.inject.{target}"),
            target: target.clone(),
            value: scalar_text(value),
        });
    }
}

fn run_usage_error() -> AppError {
    AppError::Usage("run requires '--with <entry>... -- <command> [args...]'".to_owned())
}

/// Selects the parent variables a pure run carries: names matching the
/// curated base or a `--keep`, under the parent's own spelling with the
/// parent's `OsString` value unchanged. Matching uses the platform's
/// environment-name equivalence; a parent name that is not valid Unicode
/// matches nothing and is dropped. Returns the selection and the kept names
/// absent from the parent, deduplicated, in the order given.
fn select_pure_environment(
    parent: impl IntoIterator<Item = (OsString, OsString)>,
    keep: &[String],
) -> (Vec<(OsString, OsString)>, Vec<String>) {
    let parent: Vec<(OsString, OsString)> = parent.into_iter().collect();
    let selected = parent
        .iter()
        .filter(|(name, _)| {
            PURE_BASE
                .iter()
                .any(|base| same_os_environment_name(name, base))
                || keep.iter().any(|kept| same_os_environment_name(name, kept))
        })
        .cloned()
        .collect();
    let mut missing = Vec::new();
    for kept in keep {
        let in_parent = parent
            .iter()
            .any(|(name, _)| same_os_environment_name(name, kept));
        let already_reported = missing
            .iter()
            .any(|known: &String| same_environment_name(known, kept));
        if !in_parent && !already_reported {
            missing.push(kept.clone());
        }
    }
    (selected, missing)
}

/// Writes and flushes one stderr line per missing `--keep` name. The report
/// concerns inheritance only and never changes the exit status; a failure to
/// write it is a hard error, matching the untrusted-project notice contract.
fn report_missing_keeps(missing: &[String]) -> Result<(), AppError> {
    use std::io::Write;

    if missing.is_empty() {
        return Ok(());
    }
    let mut stderr = std::io::stderr();
    let failure = |error: std::io::Error| {
        AppError::Usage(format!(
            "could not write the --keep diagnostic: {error}; retry in a terminal with writable standard error"
        ))
    };
    for name in missing {
        writeln!(
            stderr,
            "--keep {name}: the variable is not set in the parent environment; continuing without it (set it or drop '--keep {name}')"
        )
        .map_err(failure)?;
    }
    stderr.flush().map_err(failure)
}

fn same_environment_name(left: &str, right: &str) -> bool {
    #[cfg(windows)]
    {
        left.eq_ignore_ascii_case(right)
    }
    #[cfg(not(windows))]
    {
        left == right
    }
}

fn same_os_environment_name(left: &OsString, right: &str) -> bool {
    left.to_str()
        .is_some_and(|left| same_environment_name(left, right))
}

fn launch(
    program: &str,
    args: &[String],
    environment: Vec<(OsString, OsString)>,
) -> Result<Infallible, AppError> {
    let mut command = Command::new(program);
    command.args(args).env_clear().envs(environment);

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;

        let error = command.exec();
        Err(AppError::TargetNotExecutable(format!(
            "'{program}' could not be executed: {error}; verify the target command and try 'agentenv run --with <entry> -- <command> [args...]'"
        )))
    }

    #[cfg(not(unix))]
    {
        let status = command.status().map_err(|error| {
            AppError::TargetNotExecutable(format!(
                "'{program}' could not be executed: {error}; verify the target command and try 'agentenv run --with <entry> -- <command> [args...]'"
            ))
        })?;
        std::process::exit(
            status
                .code()
                .expect("a Windows process always reports an exit code"),
        );
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;

    use super::select_pure_environment;

    fn pairs(entries: &[(&str, &str)]) -> Vec<(OsString, OsString)> {
        entries
            .iter()
            .map(|(name, value)| (OsString::from(name), OsString::from(value)))
            .collect()
    }

    #[test]
    fn base_names_are_carried_and_unlisted_names_dropped() {
        let parent = pairs(&[("PATH", "/bin"), ("TERM", "xterm"), ("STRAY_SECRET", "x")]);
        let (selected, missing) = select_pure_environment(parent, &[]);
        let names: Vec<_> = selected.iter().map(|(n, _)| n.clone()).collect();
        assert!(names.contains(&OsString::from("PATH")));
        assert!(names.contains(&OsString::from("TERM")));
        assert!(!names.contains(&OsString::from("STRAY_SECRET")));
        assert!(missing.is_empty());
    }

    #[test]
    fn unlisted_lc_names_are_excluded() {
        // AC-001.6 (seam level): the base is a closed list, not an LC_ prefix.
        let parent = pairs(&[("LC_ALL", "C"), ("LC_SECRET_TOKEN", "sentinel-lc")]);
        let (selected, _) = select_pure_environment(parent, &[]);
        let names: Vec<_> = selected.iter().map(|(n, _)| n.clone()).collect();
        assert!(names.contains(&OsString::from("LC_ALL")));
        assert!(!names.contains(&OsString::from("LC_SECRET_TOKEN")));
    }

    #[test]
    fn keeps_carry_parent_values_and_missing_keeps_are_deduplicated() {
        let parent = pairs(&[("AWS_REGION", "eu-west-1")]);
        let keep = [
            "AWS_REGION".to_owned(),
            "ABSENT".to_owned(),
            "ABSENT".to_owned(),
        ];
        let (selected, missing) = select_pure_environment(parent, &keep);
        assert_eq!(
            selected,
            pairs(&[("AWS_REGION", "eu-west-1")]),
            "the kept variable carries the parent value"
        );
        assert_eq!(missing, ["ABSENT"], "missing keeps are reported once");
    }

    #[cfg(unix)]
    #[test]
    fn non_unicode_values_are_carried_unchanged_and_non_unicode_names_dropped() {
        // EDGE-005: values are OS strings end to end; a non-Unicode name can
        // match no base or keep name and is dropped like any unlisted name.
        use std::os::unix::ffi::OsStringExt;

        let raw_value = OsString::from_vec(vec![b'/', b'b', 0x80, b'n']);
        let raw_name = OsString::from_vec(vec![b'P', 0x80, b'X']);
        let parent = vec![
            (OsString::from("HOME"), raw_value.clone()),
            (raw_name, OsString::from("dropped")),
        ];
        let (selected, _) = select_pure_environment(parent, &[]);
        assert_eq!(selected, vec![(OsString::from("HOME"), raw_value)]);
    }
}
