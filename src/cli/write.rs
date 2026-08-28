//! Execution of the config-write commands (`set`, `unset`, `init`,
//! `credential add`).

use agentenv::config::write as config_write;
use agentenv::config::write::{CredentialAddRequest, ProviderSpec, SetRequest, ValueSpec};
use agentenv::error::AppError;

use super::{
    trusted_project_pin, Command, CredentialAddArgs, CredentialArgs, CredentialCommand, Invocation,
    Output, ProviderKind, ValueType,
};

/// Runs a config-write command (`set`, `unset`, `init`, `credential add`).
/// Write commands load and validate through `config::write` themselves, so
/// they run before the read-side `Config::load`.
pub(super) fn execute(
    invocation: Invocation,
    env: &impl Fn(&str) -> Option<String>,
) -> Result<Output, AppError> {
    let project_pin = trusted_project_pin(&invocation.project).cloned();
    let command_name = match &invocation.command {
        Command::Set(_) => "set",
        Command::Unset { .. } => "unset",
        Command::Init => "init",
        Command::Credential(_) => "credential add",
        _ => unreachable!("write::execute handles only write commands"),
    };
    if invocation.json {
        return Err(AppError::Usage(format!(
            "{command_name} does not support --json; write commands have no JSON output"
        )));
    }
    let stdout = match invocation.command {
        Command::Set(args) => config_write::set(
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
            project_pin.as_ref(),
        )?,
        Command::Unset { path } => config_write::unset(
            invocation.profile.as_deref(),
            &path,
            env,
            project_pin.as_ref(),
        )?,
        Command::Init => config_write::init(env)?,
        Command::Credential(CredentialArgs {
            command: CredentialCommand::Add(args),
        }) => config_write::credential_add(credential_add_request(args)?, env)?,
        _ => unreachable!("write::execute handles only write commands"),
    };
    Ok(Output {
        stdout,
        stderr: String::new(),
        status: 0,
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
