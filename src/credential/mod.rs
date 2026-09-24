//! Credential resolution and storage behind a secret-safe provider seam.

mod command;
mod env;
mod keychain;
pub mod resolver;
mod secret;
mod shallow;
#[cfg(all(feature = "test-keychain", debug_assertions))]
mod test_store;

#[cfg(all(feature = "test-keychain", not(debug_assertions)))]
compile_error!("the test-keychain feature is restricted to debug builds");

pub use secret::{CapturedSecret, Secret, SecretDomainError};
pub use shallow::{shallow_status, Status};

use crate::config::{CredentialDef, Provider as ProviderDef};
use crate::error::AppError;

/// The largest value the fill resolution stage accepts, in UTF-8 bytes.
/// Tokens and keys routinely exceed the 255-byte authentication limit.
pub const FILL_VALUE_LIMIT: usize = 8192;

/// Why a confidential resolution produced no value. Variants carry no bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConfidentialError {
    /// The provider could not run or complete, or produced no usable value:
    /// spawn failure, read failure, unsuccessful exit, a store that could
    /// not be read, or output that is empty, contains NUL, or is not UTF-8.
    Execution,
    /// The provider produced a value larger than the confidential capture
    /// bound for the requested stage.
    Value,
}

/// The common interface for a configured credential provider.
///
/// Ordinary resolution keeps the historical terminal and line-oriented
/// provider behavior that `run` and `credential check` rely on. Confidential
/// resolution serves the sudo, SSH, and fill stages: a provider gets no
/// terminal input, its stderr is discarded, its output is captured into a
/// fixed buffer of at most [`FILL_VALUE_LIMIT`] + 2 bytes, and failures are
/// classified without carrying candidate bytes.
pub trait Provider {
    fn shallow_status(&self) -> Status;
    fn resolve(&self) -> Result<Secret, AppError>;
    fn store(&self, value: Secret) -> Result<(), AppError>;
    /// Resolves for a confidential consumer. `line_oriented` removes exactly
    /// one trailing line ending from a captured line, as ordinary resolution
    /// does for environment credentials; authentication stages keep the raw
    /// bytes and reject line endings.
    fn resolve_confidential(&self, line_oriented: bool) -> Result<Secret, ConfidentialError>;
}

/// Selects the provider adapter for a validated credential definition.
pub fn provider_for(definition: &CredentialDef) -> Box<dyn Provider> {
    match &definition.provider {
        ProviderDef::Env { name } => {
            Box::new(env::EnvProvider::new(definition.name.clone(), name.clone()))
        }
        ProviderDef::Keychain { service, account } => Box::new(keychain::KeychainProvider::new(
            definition.name.clone(),
            service.clone(),
            account.clone(),
        )),
        ProviderDef::Command { argv } => Box::new(command::CommandProvider::new(
            definition.name.clone(),
            argv.clone(),
        )),
    }
}
