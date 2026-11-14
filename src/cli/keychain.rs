//! macOS credential helper setup and nonsecret Keychain diagnostics.

use std::path::PathBuf;

use clap::{Args, Subcommand};

use agentenv::error::AppError;

use super::Output;

#[derive(Debug, Args)]
pub struct KeychainArgs {
    #[command(subcommand)]
    pub command: KeychainCommand,
}

#[derive(Debug, Subcommand)]
pub enum KeychainCommand {
    /// Inspect macOS Keychain lock settings, session access, and helper installation.
    Status,
    /// Open the macOS system unlock window if the Keychain is locked.
    Unlock,
    /// Install the fixed macOS credential helper with a reusable local signature.
    Setup {
        /// Helper source; defaults to the matching executable beside agentenv.
        #[arg(long = "from", value_name = "FILE")]
        source: Option<PathBuf>,
        /// Replace the installed helper; existing items may need authorization again.
        #[arg(long)]
        replace_helper: bool,
    },
    #[command(hide = true)]
    PrepareInstall {
        #[arg(long, value_name = "DIRECTORY")]
        directory: PathBuf,
        #[arg(long, value_name = "FILE")]
        helper: Option<PathBuf>,
    },
    #[cfg(all(feature = "test-support", debug_assertions))]
    #[command(hide = true)]
    DeleteFixture {
        #[arg(long)]
        service: String,
        #[arg(long)]
        account: String,
    },
}

#[cfg(not(target_os = "macos"))]
pub(super) fn execute(_args: KeychainArgs, _json: bool) -> Result<Output, AppError> {
    Err(AppError::Usage(
        "the keychain command is available only on macOS".into(),
    ))
}

#[cfg(target_os = "macos")]
pub(super) fn execute(args: KeychainArgs, json: bool) -> Result<Output, AppError> {
    use agentenv::macos::{self, installation};
    use serde_json::json;

    let stdout = match args.command {
        KeychainCommand::Status => {
            let state = macos::diagnose().map_err(AppError::Keychain)?;
            let helper = installation::status();
            if json {
                super::json_line(json!({
                    "version": 1,
                    "keychain": state,
                    "helper": match helper {
                        Ok(Some(report)) => json!({"status": "ready", "details": report}),
                        Ok(None) => json!({"status": "not-installed", "action": "agentenv keychain setup"}),
                        Err(error) => json!({"status": "unavailable", "reason": error.to_string()}),
                    },
                }))
            } else {
                let helper = match helper {
                    Ok(Some(report)) => format!(
                        "Credential helper: verified at {}\n",
                        report.helper.display(),
                    ),
                    Ok(None) => {
                        "Credential helper: not installed; run 'agentenv keychain setup'.\n".into()
                    }
                    Err(error) => format!("Credential helper: unavailable ({error})\n"),
                };
                format!(
                    "Keychain: {}\nState: {}\nLock on sleep: {}\nAutomatic lock: {}\nDesktop session: {}\nKeychain interaction: {}\n{helper}",
                    state.keychain.display(),
                    if state.unlocked { "unlocked" } else { "locked" },
                    state.lock_settings.as_ref().map_or("unavailable", |settings| if settings.locks_on_sleep { "enabled" } else { "disabled" }),
                    state.lock_settings.as_ref().map_or_else(|| "unavailable".into(), |settings| settings.lock_after_seconds.map_or_else(|| "disabled".into(), |seconds| format!("after {seconds} seconds"))),
                    if state.session.graphical { "available" } else { "unavailable" },
                    if state.session.interaction_allowed { "allowed" } else { "disabled" },
                )
            }
        }
        KeychainCommand::Unlock => {
            let report = macos::unlock().map_err(AppError::Keychain)?;
            if json {
                super::json_line(
                    json!({"version":1, "status": if report.was_locked { "unlocked" } else { "already-unlocked" }, "keychain": report.state}),
                )
            } else if report.was_locked {
                "The Keychain is unlocked.\n".into()
            } else {
                "The Keychain is already unlocked.\n".into()
            }
        }
        KeychainCommand::Setup {
            source,
            replace_helper,
        } => {
            if replace_helper {
                eprintln!("Replacing the credential helper may require authorization for existing Keychain items again.");
            }
            let report = installation::setup(source.as_deref(), replace_helper)?;
            if json {
                super::json_line(
                    json!({"version":1, "status": if report.reused {"ready"} else {"installed"}, "details":report}),
                )
            } else {
                format!(
                    "Credential helper {} at {}.\n{}",
                    if report.reused {
                        "verified"
                    } else {
                        "installed"
                    },
                    report.helper.display(),
                    if report.reused {
                        ""
                    } else {
                        "Existing credentials may ask for access on their first use by this helper.\n"
                    }
                )
            }
        }
        KeychainCommand::PrepareInstall { directory, helper } => {
            let report = installation::prepare_bundle(&directory, helper.as_deref())?;
            if json {
                super::json_line(json!({"version":1, "status":"prepared", "details":report}))
            } else {
                format!(
                    "Signed the staged macOS bundle; credential helper {} at {}.\n",
                    if report.reused {
                        "preserved"
                    } else {
                        "installed"
                    },
                    report.helper.display()
                )
            }
        }
        #[cfg(all(feature = "test-support", debug_assertions))]
        KeychainCommand::DeleteFixture { service, account } => {
            match macos::client::delete_fixture(&service, &account) {
                Ok(()) => "Synthetic Keychain fixture deleted.\n".into(),
                Err(error) if error.reason == agentenv::credential::KeychainReason::Missing => {
                    "Synthetic Keychain fixture is absent.\n".into()
                }
                Err(error) => return Err(AppError::Keychain(error)),
            }
        }
    };
    Ok(Output::success(stdout, String::new()))
}
