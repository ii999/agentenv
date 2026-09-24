use crate::credential::shallow::keychain_status;
use crate::credential::{
    CapturedSecret, ConfidentialError, Provider, Secret, SecretDomainError, Status,
};
use crate::error::AppError;

#[cfg(all(feature = "test-keychain", debug_assertions))]
use crate::credential::test_store::TestStore;

pub(crate) struct KeychainProvider {
    credential_name: String,
    service: String,
    account: String,
}

/// Why a keychain read produced no secret. `Read` carries the store's own
/// diagnostic, which names the item but never its value.
enum ReadFailure {
    Read(String),
    Missing,
    Invalid(SecretDomainError),
}

impl KeychainProvider {
    pub(crate) fn new(credential_name: String, service: String, account: String) -> Self {
        Self {
            credential_name,
            service,
            account,
        }
    }

    fn read_error(&self, error: impl std::fmt::Display) -> AppError {
        AppError::Credential(format!(
            "keychain credential '{}' for service '{}' and account '{}' could not be read: {error}; check the platform keychain or use an env/command credential",
            self.credential_name, self.service, self.account
        ))
    }

    fn write_error(&self, error: impl std::fmt::Display) -> AppError {
        AppError::Credential(format!(
            "keychain credential '{}' for service '{}' and account '{}' could not be stored: {error}; check the platform keychain and retry 'agentenv credential set {}'",
            self.credential_name, self.service, self.account, self.credential_name
        ))
    }

    fn read(&self) -> Result<Secret, ReadFailure> {
        #[cfg(all(feature = "test-keychain", debug_assertions))]
        if let Some(store) = TestStore::from_environment() {
            return store
                .read(&self.service, &self.account)
                .map_err(|error| ReadFailure::Read(error.to_string()))?
                .ok_or(ReadFailure::Missing);
        }
        let entry = keyring::Entry::new(&self.service, &self.account)
            .map_err(|error| ReadFailure::Read(error.to_string()))?;
        // Pair `get_password` with `set_password`: Windows stores passwords as
        // UTF-16 credential blobs, while `get_secret` returns those raw bytes.
        // The password API performs the platform-native decoding before the
        // value enters agentenv's UTF-8-only secret container.
        let value = entry
            .get_password()
            .map_err(|error| match error {
                keyring::Error::NoEntry => ReadFailure::Missing,
                other => ReadFailure::Read(other.to_string()),
            })?
            .into_bytes();
        CapturedSecret::new(value)
            .into_secret()
            .map_err(ReadFailure::Invalid)
    }
}

impl Provider for KeychainProvider {
    fn shallow_status(&self) -> Status {
        keychain_status()
    }

    fn resolve(&self) -> Result<Secret, AppError> {
        self.read().map_err(|failure| match failure {
            ReadFailure::Read(error) => self.read_error(error),
            ReadFailure::Missing => AppError::Credential(format!(
                "keychain credential '{}' is missing for service '{}' and account '{}'; set it with 'agentenv credential set {}'",
                self.credential_name, self.service, self.account, self.credential_name
            )),
            ReadFailure::Invalid(error) => AppError::Credential(format!(
                "keychain credential '{}' for service '{}' and account '{}' has an invalid value: {error}; set it again with 'agentenv credential set {}'",
                self.credential_name, self.service, self.account, self.credential_name
            )),
        })
    }

    fn resolve_confidential(&self, _line_oriented: bool) -> Result<Secret, ConfidentialError> {
        // Every read failure, including an unusable stored value, is a
        // provider failure; only stage validation rejects a value.
        self.read().map_err(|_| ConfidentialError::Execution)
    }

    fn store(&self, value: Secret) -> Result<(), AppError> {
        #[cfg(all(feature = "test-keychain", debug_assertions))]
        if let Some(store) = TestStore::from_environment() {
            return store.write(&self.service, &self.account, &value);
        }
        let entry = keyring::Entry::new(&self.service, &self.account)
            .map_err(|error| self.write_error(error))?;
        entry
            .set_password(value.as_str())
            .map_err(|error| self.write_error(error))
    }
}
