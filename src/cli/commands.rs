use std::io::{self, IsTerminal, Read};
use std::path::Path;

use clap::{Args, Subcommand};

use agentenv::config::write::{self, CredentialAddRequest, ProviderSpec, SetRequest, ValueSpec};
use agentenv::config::{Config, CredentialDef, Provider};
use agentenv::credential::{provider_for, CapturedSecret, Secret};
use agentenv::error::AppError;
use agentenv::path::{single_entry_name, Segments};
use agentenv::runner::InjectionPlan;
use agentenv::{query, render};

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
}

pub struct Output {
    pub stdout: String,
    pub stderr: String,
}

pub fn execute(invocation: Invocation) -> Result<Output, AppError> {
    let env = |name: &str| std::env::var(name).ok();
    if matches!(&invocation.command, Command::Validate) {
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
    if matches!(
        &invocation.command,
        Command::Set(_)
            | Command::Unset { .. }
            | Command::Init
            | Command::Credential(CredentialArgs {
                command: CredentialCommand::Add(_),
            })
    ) {
        return execute_write(invocation, &env);
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
            let profile = select_profile(&config, invocation.profile.as_deref(), &env)?;
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
            })
        }
        Command::List(args) => {
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
        Command::Show {
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
        Command::Get { path } => {
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
            })
        }
        Command::Credential(CredentialArgs {
            command: CredentialCommand::Check { name },
        }) => {
            reject_json_for_credential_action(invocation.json, "check")?;
            let credential = credential_for(&config, &name)?;
            provider_for(credential).resolve()?;
            Ok(Output {
                stdout: format!("Credential '{name}' is available.\n"),
                stderr: String::new(),
            })
        }
        Command::Credential(CredentialArgs {
            command: CredentialCommand::Set { name },
        }) => {
            reject_json_for_credential_action(invocation.json, "set")?;
            let credential = credential_for(&config, &name)?;
            if !matches!(&credential.provider, Provider::Keychain { .. }) {
                return Err(AppError::Usage(format!(
                    "credential '{}' uses the {} provider and is managed externally; use its provider's tooling, then run 'agentenv credential check {}'",
                    name,
                    credential.provider.kind(),
                    name
                )));
            }
            let secret = read_secret_for_set()?;
            provider_for(credential).store(secret)?;
            Ok(Output {
                stdout: format!("Credential '{name}' stored.\n"),
                stderr: String::new(),
            })
        }
    }
}

/// Runs a config-write command (`set`, `unset`, `init`, `credential add`).
/// Write commands load and validate through `config::write` themselves, so
/// they run before the read-side `Config::load`.
fn execute_write(
    invocation: Invocation,
    env: &impl Fn(&str) -> Option<String>,
) -> Result<Output, AppError> {
    let command_name = match &invocation.command {
        Command::Set(_) => "set",
        Command::Unset { .. } => "unset",
        Command::Init => "init",
        Command::Credential(_) => "credential add",
        _ => unreachable!("execute_write handles only write commands"),
    };
    if invocation.json {
        return Err(AppError::Usage(format!(
            "{command_name} does not support --json; write commands have no JSON output"
        )));
    }
    let stdout = match invocation.command {
        Command::Set(args) => write::set(
            SetRequest {
                profile_flag: invocation.profile,
                path: args.path,
                value: match args.value_type {
                    ValueType::String => ValueSpec::String(args.value),
                    ValueType::Int => ValueSpec::Int(args.value),
                    ValueType::Float => ValueSpec::Float(args.value),
                    ValueType::Bool => ValueSpec::Bool(args.value),
                    ValueType::Json => ValueSpec::Json(args.value),
                },
                description: args.description,
                create_profile: args.create_profile,
            },
            env,
        )?,
        Command::Unset { path } => write::unset(invocation.profile.as_deref(), &path, env)?,
        Command::Init => write::init(env)?,
        Command::Credential(CredentialArgs {
            command: CredentialCommand::Add(args),
        }) => write::credential_add(credential_add_request(args)?, env)?,
        _ => unreachable!("execute_write handles only write commands"),
    };
    Ok(Output {
        stdout,
        stderr: String::new(),
    })
}

/// Maps `credential add` flags onto the provider schema, refusing missing or
/// mismatched provider-specific flags (AC-005.4).
fn credential_add_request(args: CredentialAddArgs) -> Result<CredentialAddRequest, AppError> {
    let CredentialAddArgs {
        name,
        description,
        provider,
        inject_as,
        env_var,
        service,
        account,
        argv,
    } = args;
    let missing = |flag: &str, provider: &str| {
        AppError::Usage(format!(
            "--{flag} is required for the {provider} provider; add --{flag} and retry"
        ))
    };
    let mismatched = |flag: &str, provider: &str| {
        AppError::Usage(format!(
            "--{flag} does not apply to the {provider} provider; drop --{flag} and retry"
        ))
    };
    let spec = match provider {
        ProviderKind::Env => {
            if service.is_some() {
                return Err(mismatched("service", "env"));
            }
            if account.is_some() {
                return Err(mismatched("account", "env"));
            }
            if !argv.is_empty() {
                return Err(mismatched("argv", "env"));
            }
            ProviderSpec::Env {
                var: env_var.ok_or_else(|| missing("env-var", "env"))?,
            }
        }
        ProviderKind::Keychain => {
            if env_var.is_some() {
                return Err(mismatched("env-var", "keychain"));
            }
            if !argv.is_empty() {
                return Err(mismatched("argv", "keychain"));
            }
            ProviderSpec::Keychain {
                service: service.ok_or_else(|| missing("service", "keychain"))?,
                account: account.ok_or_else(|| missing("account", "keychain"))?,
            }
        }
        ProviderKind::Command => {
            if env_var.is_some() {
                return Err(mismatched("env-var", "command"));
            }
            if service.is_some() {
                return Err(mismatched("service", "command"));
            }
            if account.is_some() {
                return Err(mismatched("account", "command"));
            }
            if argv.is_empty() {
                return Err(missing("argv", "command"));
            }
            ProviderSpec::Command { argv }
        }
    };
    Ok(CredentialAddRequest {
        name,
        description,
        provider: spec,
        inject_as,
    })
}

fn reject_json_for_credential_action(json: bool, action: &str) -> Result<(), AppError> {
    if json {
        return Err(AppError::Usage(format!(
            "credential {action} does not support --json; run 'agentenv credential {action} <name>'"
        )));
    }
    Ok(())
}

fn credential_for<'a>(config: &'a Config, name: &str) -> Result<&'a CredentialDef, AppError> {
    config.credential(name).ok_or_else(|| {
        let defined = config
            .credentials
            .iter()
            .map(|credential| credential.name.as_str())
            .collect::<Vec<_>>();
        let names = if defined.is_empty() {
            "(none defined)".to_owned()
        } else {
            defined.join(", ")
        };
        AppError::NotFound(format!(
            "credential '{name}' is not defined; defined credentials: {names}; run 'agentenv credential list' to inspect them"
        ))
    })
}

fn read_secret_for_set() -> Result<Secret, AppError> {
    let bytes = if io::stdin().is_terminal() {
        read_terminal_secret()
            .map_err(|error| {
                AppError::Usage(format!(
                    "could not read the credential value: {error}; retry 'agentenv credential set <name>'"
                ))
            })?
            .into_bytes()
    } else {
        let mut bytes = Vec::new();
        io::stdin().read_to_end(&mut bytes).map_err(|error| {
            AppError::Usage(format!(
                "could not read credential input: {error}; pipe a value and retry 'agentenv credential set <name>'"
            ))
        })?;
        bytes
    };
    CapturedSecret::new(bytes)
        .strip_one_trailing_newline()
        .into_secret()
        .map_err(|error| {
        AppError::Usage(format!(
            "credential value is invalid: {error}; provide a non-empty UTF-8 value and retry 'agentenv credential set <name>'"
        ))
    })
}

#[cfg(unix)]
fn read_terminal_secret() -> io::Result<String> {
    let config = rpassword::ConfigBuilder::new()
        .input_file_path("/dev/stdin")
        .output_writer(io::stderr())
        .build();
    rpassword::prompt_password_with_config("Credential value: ", config)
}

#[cfg(not(unix))]
fn read_terminal_secret() -> io::Result<String> {
    rpassword::prompt_password("Credential value: ")
}

fn select_profile<'a>(
    config: &'a Config,
    flag: Option<&str>,
    env: &impl Fn(&str) -> Option<String>,
) -> Result<&'a agentenv::config::Profile, AppError> {
    let env_profile = env("AGENTENV_PROFILE");
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

        let path = agentenv::config::locate::resolve_path(explicit_file, env)?;
        let metadata = std::fs::metadata(&path).map_err(|error| {
            AppError::Config(vec![agentenv::error::Violation {
                path: path.display().to_string(),
                message: format!("cannot inspect config-file permissions: {error}"),
            }])
        })?;
        let mode = metadata.permissions().mode() & 0o777;
        if mode & !0o600 != 0 {
            return Err(AppError::Config(vec![agentenv::error::Violation {
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
