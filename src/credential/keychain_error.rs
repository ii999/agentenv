//! Secret-free Keychain failures that survive confidential resolver boundaries.

use std::fmt;

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum KeychainReason {
    Missing,
    Locked,
    InteractionUnavailable,
    Cancelled,
    AccessDenied,
    Unavailable,
    KeychainNotFound,
    UnlockFailed,
    InvalidValue,
    HelperMissing,
    HelperInvalid,
    Transport,
    StoreUnconfirmed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum KeychainOperation {
    Read,
    Store,
    Status,
    Unlock,
    Connect,
}

/// This type never holds provider output, passwords, or system error strings.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KeychainFailure {
    pub reason: KeychainReason,
    pub operation: KeychainOperation,
    pub os_status: Option<i32>,
    pub locked: Option<bool>,
    pub graphical_session: Option<bool>,
    pub interaction_allowed: Option<bool>,
}

impl KeychainFailure {
    pub fn new(reason: KeychainReason, operation: KeychainOperation) -> Self {
        Self {
            reason,
            operation,
            os_status: None,
            locked: None,
            graphical_session: None,
            interaction_allowed: None,
        }
    }

    pub fn store_unconfirmed(&self) -> bool {
        self.reason == KeychainReason::StoreUnconfirmed
    }
}

impl fmt::Display for KeychainFailure {
    fn fmt(&self, output: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (message, action) = match self.reason {
            KeychainReason::Missing => (
                "the Keychain credential does not exist",
                "store the credential with 'agentenv credential set'",
            ),
            KeychainReason::Locked => (
                "the Keychain is still locked",
                "run 'agentenv keychain unlock' in the logged-in desktop session, then retry",
            ),
            KeychainReason::InteractionUnavailable => (
                "Keychain access requires interaction that this session cannot provide",
                "run 'agentenv keychain status --json' and unlock or authorize access in the logged-in desktop session",
            ),
            KeychainReason::Cancelled => (
                "Keychain authorization was cancelled",
                "retry when ready to complete the system authorization window",
            ),
            KeychainReason::AccessDenied => (
                "Keychain access was denied",
                "inspect the item's access control and run 'agentenv keychain status --json'; an unlocked Keychain may still require item authorization",
            ),
            KeychainReason::Unavailable => (
                "the Keychain operation could not complete",
                "run 'agentenv keychain status --json' and check the reported macOS status",
            ),
            KeychainReason::KeychainNotFound => (
                "the default or selected Keychain could not be found",
                "run 'agentenv keychain status --json' and check this session's Keychain selection and HOME setting",
            ),
            KeychainReason::UnlockFailed => (
                "macOS could not complete the system Keychain unlock request",
                "make sure the desktop is unlocked, then retry 'agentenv keychain unlock'; retain the OSStatus if the system window still cannot open",
            ),
            KeychainReason::InvalidValue => (
                "the Keychain credential has an unsupported value",
                "store a nonempty UTF-8 value without NUL bytes using 'agentenv credential set'",
            ),
            KeychainReason::HelperMissing => (
                "the macOS credential helper is not installed",
                "run 'agentenv keychain setup' from a complete release or source build",
            ),
            KeychainReason::HelperInvalid => (
                "the installed macOS credential helper could not be verified",
                "run 'agentenv keychain setup' to inspect installation state; an intentional helper replacement uses '--replace-helper' and may require credential authorization again",
            ),
            KeychainReason::Transport => (
                "the macOS credential helper did not complete its response",
                "run 'agentenv keychain status --json' and retry the read after repairing the helper",
            ),
            KeychainReason::StoreUnconfirmed => (
                "the macOS credential helper did not confirm the save; the value may have been stored",
                "check the credential before deciding whether to save it again",
            ),
        };
        output.write_str(message)?;
        if let Some(status) = self.os_status {
            write!(output, " (OSStatus {status})")?;
        }
        if let Some(locked) = self.locked {
            write!(
                output,
                "; Keychain {}",
                if locked { "locked" } else { "unlocked" }
            )?;
        }
        if let Some(graphical) = self.graphical_session {
            write!(
                output,
                "; graphical session {}",
                if graphical { "present" } else { "absent" }
            )?;
        }
        write!(output, "; {action}")
    }
}

impl std::error::Error for KeychainFailure {}
