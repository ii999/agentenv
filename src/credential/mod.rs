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

/// The common interface for a configured credential provider.
pub trait Provider {
    fn shallow_status(&self) -> Status;
    fn resolve(&self) -> Result<Secret, AppError>;
    fn store(&self, value: Secret) -> Result<(), AppError>;
}

/// Resolution I/O is consumer-specific; ordinary commands retain their
/// historical terminal and line-oriented provider behavior.
#[derive(Clone, Copy)]
pub enum ResolutionIo {
    Ordinary,
    Confidential { max_bytes: usize },
}

/// Selects the provider adapter for a validated credential definition.
pub fn provider_for(definition: &CredentialDef) -> Box<dyn Provider> {
    provider_for_with_io(definition, ResolutionIo::Ordinary)
}

pub fn provider_for_with_io(definition: &CredentialDef, io: ResolutionIo) -> Box<dyn Provider> {
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
            io,
        )),
    }
}
