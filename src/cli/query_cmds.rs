use std::path::Path;

use clap::{Args, Subcommand};

use agent_context::config::Config;
use agent_context::error::AppError;
use agent_context::path::{single_entry_name, Segments};
use agent_context::{query, render};

#[derive(Debug, Subcommand)]
pub enum QueryCommand {
    /// List entries in the active profile.
    List(ListArgs),
    /// Show one entry and its values.
    Show { entry: String },
    /// Read one scalar field or raw JSON value.
    Get { path: String },
    /// Search entries and fields in the active profile.
    Find(FindArgs),
    /// Validate the configuration file.
    Validate,
    /// Inspect configured credential definitions.
    Credential(CredentialArgs),
}

#[derive(Debug, Args)]
pub struct ListArgs {
    /// List profiles without selecting one.
    #[arg(long)]
    pub profiles: bool,
    /// Show one entry; nested fields are included in text output.
    pub entry: Option<String>,
}

#[derive(Debug, Args)]
pub struct FindArgs {
    /// Search all profiles without selecting one.
    #[arg(long)]
    pub all_profiles: bool,
    pub needle: String,
}

#[derive(Debug, Args)]
pub struct CredentialArgs {
    #[command(subcommand)]
    pub command: CredentialCommand,
}

#[derive(Debug, Subcommand)]
pub enum CredentialCommand {
    /// List credential definitions without resolving them.
    List,
}

pub struct Invocation {
    pub profile: Option<String>,
    pub json: bool,
    pub command: QueryCommand,
}

pub struct Output {
    pub stdout: String,
    pub stderr: String,
}

pub fn execute(invocation: Invocation) -> Result<Output, AppError> {
    let env = |name: &str| std::env::var(name).ok();
    if matches!(&invocation.command, QueryCommand::Validate) {
        if invocation.json {
            return Err(AppError::Usage(
                "validate does not support --json".to_owned(),
            ));
        }
        return validate_config(&env).map(|()| Output {
            stdout: "Configuration is valid.\n".to_owned(),
            stderr: String::new(),
        });
    }
    let config = Config::load(None, &env)?;
    match invocation.command {
        QueryCommand::Validate => unreachable!("validate returns before loading the configuration"),
        QueryCommand::List(args) if args.profiles => {
            if args.entry.is_some() {
                return Err(AppError::Usage(
                    "list accepts either --profiles or an entry name".to_owned(),
                ));
            }
            let profiles = query::profiles(&config);
            let stdout = if invocation.json {
                json_stdout(render::profiles_json(config.version, &profiles))
            } else {
                render::profiles_text(&profiles)
            };
            Ok(Output {
                stdout,
                stderr: String::new(),
            })
        }
        QueryCommand::List(args) => {
            let profile = select_profile(&config, invocation.profile.as_deref(), &env)?;
            if let Some(entry_argument) = args.entry {
                let entry_name = single_entry_name(&entry_argument)?;
                let entry = query::entry(&config, profile, &entry_name, &env)?;
                let stdout = if invocation.json {
                    json_stdout(render::entry_json(config.version, &entry))
                } else {
                    render::entry_text(&entry, false)
                };
                Ok(Output {
                    stdout,
                    stderr: String::new(),
                })
            } else {
                let listing = query::list(&config, profile, &env);
                let stdout = if invocation.json {
                    json_stdout(render::list_json(config.version, &listing))
                } else {
                    render::list_text(&listing)
                };
                Ok(Output {
                    stdout,
                    stderr: String::new(),
                })
            }
        }
        QueryCommand::Show {
            entry: entry_argument,
        } => {
            let profile = select_profile(&config, invocation.profile.as_deref(), &env)?;
            let entry_name = single_entry_name(&entry_argument)?;
            let entry = query::entry(&config, profile, &entry_name, &env)?;
            let stdout = if invocation.json {
                json_stdout(render::entry_json(config.version, &entry))
            } else {
                render::entry_text(&entry, true)
            };
            Ok(Output {
                stdout,
                stderr: String::new(),
            })
        }
        QueryCommand::Get { path } => {
            let profile = select_profile(&config, invocation.profile.as_deref(), &env)?;
            let path = Segments::parse(&path)?;
            let value = query::get(profile, &path)?;
            if invocation.json {
                Ok(Output {
                    stdout: json_stdout(render::raw_get_json(value)),
                    stderr: String::new(),
                })
            } else if let Some(stdout) = render::get_text(value) {
                Ok(Output {
                    stdout,
                    stderr: String::new(),
                })
            } else {
                Err(AppError::Usage(format!(
                    "'{}' is {} {}; use --json to retrieve it{}",
                    path.render(),
                    if value.type_str() == "array" {
                        "an"
                    } else {
                        "a"
                    },
                    value.type_str(),
                    if value.as_table().is_some() {
                        "; use 'agent-context show <entry>' for a readable entry view"
                    } else {
                        ""
                    }
                )))
            }
        }
        QueryCommand::Find(args) => {
            let selected = if args.all_profiles {
                config.profiles.iter().collect::<Vec<_>>()
            } else {
                vec![select_profile(
                    &config,
                    invocation.profile.as_deref(),
                    &env,
                )?]
            };
            let matches = query::find(&config, selected, &args.needle, &env)?;
            let stderr = if matches.is_empty() && !invocation.json {
                format!("No matches for '{}'.\n", args.needle)
            } else {
                String::new()
            };
            let stdout = if invocation.json {
                json_stdout(render::find_json(config.version, &matches))
            } else {
                render::find_text(&matches)
            };
            Ok(Output { stdout, stderr })
        }
        QueryCommand::Credential(CredentialArgs {
            command: CredentialCommand::List,
        }) => {
            let credentials = query::credentials(&config, &env);
            let stdout = if invocation.json {
                json_stdout(render::credentials_json(config.version, &credentials))
            } else {
                render::credentials_text(&credentials)
            };
            Ok(Output {
                stdout,
                stderr: String::new(),
            })
        }
    }
}

fn select_profile<'a>(
    config: &'a Config,
    flag: Option<&str>,
    env: &impl Fn(&str) -> Option<String>,
) -> Result<&'a agent_context::config::Profile, AppError> {
    let env_profile = env("AGENT_CONTEXT_PROFILE");
    config.select_profile(flag, env_profile.as_deref())
}

fn json_stdout(value: serde_json::Value) -> String {
    serde_json::to_string(&value).expect("query JSON views are serializable") + "\n"
}

fn validate_permissions(
    explicit_file: Option<&Path>,
    env: &impl Fn(&str) -> Option<String>,
) -> Result<(), AppError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        let path = agent_context::config::locate::resolve_path(explicit_file, env)?;
        let metadata = std::fs::metadata(&path).map_err(|error| {
            AppError::Config(vec![agent_context::error::Violation {
                path: path.display().to_string(),
                message: format!("cannot inspect config-file permissions: {error}"),
            }])
        })?;
        let mode = metadata.permissions().mode() & 0o777;
        if mode & !0o600 != 0 {
            return Err(AppError::Config(vec![agent_context::error::Violation {
                path: path.display().to_string(),
                message: format!(
                    "config-file permissions are {mode:04o}; permissions must be a subset of 0600"
                ),
            }]));
        }
    }
    #[cfg(not(unix))]
    {
        let _ = (explicit_file, env);
    }
    Ok(())
}

fn validate_config(env: &impl Fn(&str) -> Option<String>) -> Result<(), AppError> {
    let config_result = Config::load(None, env);
    let permission_result = validate_permissions(None, env);
    match (config_result, permission_result) {
        (Ok(_), Ok(())) => Ok(()),
        (Err(AppError::Config(mut violations)), Err(AppError::Config(permission_violations))) => {
            violations.extend(permission_violations);
            Err(AppError::Config(violations))
        }
        (Err(error), Ok(())) | (Err(error), Err(_)) => Err(error),
        (Ok(_), Err(error)) => Err(error),
    }
}
