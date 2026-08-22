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
    /// environment, and launches the requested target. This function never
    /// mutates this process's environment.
    pub fn resolve_and_launch(self, cmd: Vec<String>) -> Result<Infallible, AppError> {
        let Some(program) = cmd.first().filter(|program| !program.is_empty()) else {
            return Err(run_usage_error());
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

        let mut environment: Vec<(OsString, OsString)> = std::env::vars_os()
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
