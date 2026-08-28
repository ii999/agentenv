//! CLI command definitions and dispatch.
//!
//! This module owns the full clap grammar and the top-level dispatch in
//! [`execute`]. Read commands are thin glue over `query` and `query::render`
//! and stay inline; the config-write flow lives in [`write`], the credential
//! runtime commands in [`credential`], and whole-file validation in
//! [`validate`].

mod credential;
mod project;
mod validate;
mod write;

use clap::{Args, Subcommand};

use agentenv::config::Config;
use agentenv::error::AppError;
use agentenv::path::{single_entry_name, Segments};
use agentenv::project::{model::ProjectPin, ProjectContext};
use agentenv::query::{self, render};
use agentenv::runner::InjectionPlan;

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Run a command with the selected entries' injected environment.
    Run(RunArgs),
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
    /// Inspect and manage the discovered project configuration file.
    Project(ProjectArgs),
    /// Write one value at a profile-scoped path.
    Set(SetArgs),
    /// Remove a field or table at a profile-scoped path.
    Unset { path: String },
    /// Create the config file at the resolved path.
    Init,
}

#[derive(Debug, Args)]
pub struct SetArgs {
    /// Target path in the segment grammar (same as `get`).
    pub path: String,
    /// The value to write.
    pub value: String,
    /// The TOML type the value is written as.
    #[arg(
        long = "type",
        value_enum,
        value_name = "TYPE",
        default_value = "string"
    )]
    pub value_type: ValueType,
    /// Entry description, written to the entry named by the first path
    /// segment (overwrites an existing description).
    #[arg(long, value_name = "TEXT")]
    pub description: Option<String>,
    /// Create the profile named by --profile, with this description.
    #[arg(long, value_name = "TEXT")]
    pub create_profile: Option<String>,
}

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub enum ValueType {
    /// A TOML string (the default).
    String,
    /// A TOML integer.
    Int,
    /// A TOML float.
    Float,
    /// A TOML boolean (`true` or `false`).
    Bool,
    /// A JSON value converted to TOML (arrays and objects allowed).
    Json,
}

#[derive(Debug, Args)]
pub struct RunArgs {
    /// Entry to inject; repeat this option to combine entries.
    #[arg(long = "with", value_name = "ENTRY", required = true, num_args = 1)]
    pub entries: Vec<String>,
    /// The command to launch, after `--`.
    #[arg(last = true, required = true, value_name = "COMMAND")]
    pub command: Vec<String>,
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

#[derive(Debug, Args)]
pub struct ProjectArgs {
    #[command(subcommand)]
    pub command: ProjectCommand,
}

#[derive(Debug, Subcommand)]
pub enum ProjectCommand {
    /// Report the discovered project's trust state and requirements.
    Status,
    /// Approve the discovered project's current contents.
    Allow,
    /// Remove approval for the discovered project.
    Revoke,
}

#[derive(Debug, Subcommand)]
pub enum CredentialCommand {
    /// List credential definitions without resolving them.
    List,
    /// Resolve a credential without printing its value.
    Check { name: String },
    /// Store a keychain credential from a terminal prompt or standard input.
    Set { name: String },
    /// Add a credential definition to the config file.
    Add(CredentialAddArgs),
}

#[derive(Debug, Args)]
pub struct CredentialAddArgs {
    /// The credential name ([A-Za-z0-9_-]+).
    pub name: String,
    /// The credential's description.
    #[arg(long, value_name = "TEXT")]
    pub description: String,
    /// The provider holding the credential value.
    #[arg(long, value_enum, value_name = "PROVIDER")]
    pub provider: ProviderKind,
    /// The environment variable this credential injects by default.
    #[arg(long = "inject-as", value_name = "ENV")]
    pub inject_as: String,
    /// env provider: the environment variable holding the value.
    #[arg(long = "env-var", value_name = "NAME")]
    pub env_var: Option<String>,
    /// keychain provider: the credential-store service.
    #[arg(long, value_name = "SERVICE")]
    pub service: Option<String>,
    /// keychain provider: the credential-store account.
    #[arg(long, value_name = "ACCOUNT")]
    pub account: Option<String>,
    /// command provider: one argv element per flag, in order.
    #[arg(long = "argv", value_name = "ARG")]
    pub argv: Vec<String>,
}

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub enum ProviderKind {
    /// Read an environment variable.
    Env,
    /// Read the platform credential store.
    Keychain,
    /// Run an external command; its stdout supplies the value.
    Command,
}

pub struct Invocation {
    pub profile: Option<String>,
    pub json: bool,
    pub command: Command,
    pub project: ProjectContext,
}

pub struct Output {
    pub stdout: String,
    pub stderr: String,
    pub status: i32,
}

impl Output {
    pub fn success(stdout: String, stderr: String) -> Self {
        Self {
            stdout,
            stderr,
            status: 0,
        }
    }
}

pub fn execute(invocation: Invocation) -> Result<Output, AppError> {
    let env = |name: &str| std::env::var(name).ok();
    if let Command::Project(args) = &invocation.command {
        return project::execute(args, invocation.profile.as_deref(), invocation.json, &env);
    }
    if matches!(&invocation.command, Command::Validate) {
        if invocation.json {
            return Err(AppError::Usage(
                "validate does not support --json".to_owned(),
            ));
        }
        return validate::validate_config(&env)
            .map(|()| Output::success("Configuration is valid.\n".to_owned(), String::new()));
    }
    if matches!(
        &invocation.command,
        Command::Set(_)
            | Command::Unset { .. }
            | Command::Init
            | Command::Credential(CredentialArgs {
                command: CredentialCommand::Add(_),
            })
    ) {
        return write::execute(invocation, &env);
    }
    let config = Config::load(None, &env)?;
    match invocation.command {
        Command::Validate => unreachable!("validate returns before loading the configuration"),
        Command::Set(_)
        | Command::Unset { .. }
        | Command::Init
        | Command::Credential(CredentialArgs {
            command: CredentialCommand::Add(_),
        }) => {
            unreachable!("write commands return before loading the configuration")
        }
        Command::Run(args) => {
            if invocation.json {
                return Err(AppError::Usage(
                    "run does not support --json; use 'agentenv run --with <entry> -- <command> [args...]'"
                        .to_owned(),
                ));
            }
            let profile = select_profile(
                &config,
                invocation.profile.as_deref(),
                &env,
                &invocation.project,
            )?;
            let plan = InjectionPlan::build(&config, profile, &args.entries)?;
            match plan.resolve_and_launch(args.command) {
                Ok(never) => match never {},
                Err(error) => Err(error),
            }
        }
        Command::List(args) if args.profiles => {
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
                status: 0,
            })
        }
        Command::List(args) => {
            let profile = select_profile(
                &config,
                invocation.profile.as_deref(),
                &env,
                &invocation.project,
            )?;
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
                    status: 0,
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
                    status: 0,
                })
            }
        }
        Command::Show {
            entry: entry_argument,
        } => {
            let profile = select_profile(
                &config,
                invocation.profile.as_deref(),
                &env,
                &invocation.project,
            )?;
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
                status: 0,
            })
        }
        Command::Get { path } => {
            let profile = select_profile(
                &config,
                invocation.profile.as_deref(),
                &env,
                &invocation.project,
            )?;
            let path = Segments::parse(&path)?;
            let value = query::get(profile, &path)?;
            if invocation.json {
                Ok(Output {
                    stdout: json_stdout(render::raw_get_json(value)),
                    stderr: String::new(),
                    status: 0,
                })
            } else if let Some(stdout) = render::get_text(value) {
                Ok(Output {
                    stdout,
                    stderr: String::new(),
                    status: 0,
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
                        "; use 'agentenv show <entry>' for a readable entry view"
                    } else {
                        ""
                    }
                )))
            }
        }
        Command::Find(args) => {
            let selected = if args.all_profiles {
                config.profiles.iter().collect::<Vec<_>>()
            } else {
                vec![select_profile(
                    &config,
                    invocation.profile.as_deref(),
                    &env,
                    &invocation.project,
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
            Ok(Output {
                stdout,
                stderr,
                status: 0,
            })
        }
        Command::Credential(CredentialArgs {
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
                status: 0,
            })
        }
        Command::Credential(CredentialArgs {
            command: CredentialCommand::Check { name },
        }) => credential::check(&config, &name, invocation.json),
        Command::Credential(CredentialArgs {
            command: CredentialCommand::Set { name },
        }) => credential::set(&config, &name, invocation.json),
        Command::Project(_) => {
            unreachable!("project commands return before loading the configuration")
        }
    }
}

fn select_profile<'a>(
    config: &'a Config,
    flag: Option<&str>,
    env: &impl Fn(&str) -> Option<String>,
    project: &ProjectContext,
) -> Result<&'a agentenv::config::Profile, AppError> {
    let env_profile = env("AGENTENV_PROFILE");
    config.select_profile(flag, env_profile.as_deref(), trusted_project_pin(project))
}

pub(super) fn trusted_project_pin(project: &ProjectContext) -> Option<&ProjectPin> {
    match project {
        ProjectContext::Trusted { meta, .. } => meta.pin.as_ref(),
        ProjectContext::None | ProjectContext::Untrusted { .. } => None,
    }
}

fn json_stdout(value: serde_json::Value) -> String {
    serde_json::to_string(&value).expect("query JSON views are serializable") + "\n"
}
