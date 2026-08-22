//! Execution of the credential runtime commands (`credential check` and
//! `credential set`), including the hidden-input path for `credential set`.

use std::io::{self, IsTerminal, Read};

use agentenv::config::{Config, CredentialDef, Provider};
use agentenv::credential::{provider_for, CapturedSecret, Secret};
use agentenv::error::AppError;

use super::Output;

pub(super) fn check(config: &Config, name: &str, json: bool) -> Result<Output, AppError> {
    reject_json_for_credential_action(json, "check")?;
    let credential = credential_for(config, name)?;
    provider_for(credential).resolve()?;
    Ok(Output {
        stdout: format!("Credential '{name}' is available.\n"),
        stderr: String::new(),
    })
}

pub(super) fn set(config: &Config, name: &str, json: bool) -> Result<Output, AppError> {
    reject_json_for_credential_action(json, "set")?;
    let credential = credential_for(config, name)?;
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
