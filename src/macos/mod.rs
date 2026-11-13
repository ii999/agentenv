//! Persistent macOS credential access and local installation identity.

pub mod client;
pub mod helper;
pub mod installation;
pub(crate) mod native;
pub(crate) mod protocol;
pub mod signing;

pub use native::{Diagnostic, LockSettings, SessionStatus, UnlockReport};

/// Inspects the current session without resolving credentials or enrolling an identity.
pub fn diagnose() -> Result<Diagnostic, crate::credential::KeychainFailure> {
    let keychain = native::open_keychain()?;
    native::inspect(&keychain)
}

/// Requests the system-owned unlock window, including before helper setup.
pub fn unlock() -> Result<UnlockReport, crate::credential::KeychainFailure> {
    let mut keychain = native::open_keychain()?;
    native::unlock(&mut keychain)
}
