use crate::credential::shallow::keychain_status;
use crate::credential::{CapturedSecret, Provider, Secret, Status};
use crate::error::AppError;

#[cfg(all(feature = "test-keychain", debug_assertions))]
use crate::credential::test_store::TestStore;

pub(crate) struct KeychainProvider {
    credential_name: String,
    service: String,
    account: String,
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
}

impl Provider for KeychainProvider {
    fn shallow_status(&self) -> Status {
        keychain_status()
    }

    fn resolve(&self) -> Result<Secret, AppError> {
        #[cfg(all(feature = "test-keychain", debug_assertions))]
        if let Some(store) = TestStore::from_environment() {
            return store.read(&self.service, &self.account)?.ok_or_else(|| {
                AppError::Credential(format!(
                    "keychain credential '{}' is missing for service '{}' and account '{}'; set it with 'agentenv credential set {}'",
                    self.credential_name, self.service, self.account, self.credential_name
                ))
            });
        }
        let entry = keyring::Entry::new(&self.service, &self.account)
            .map_err(|error| self.read_error(error))?;
        let value = entry.get_secret().map_err(|error| self.read_error(error))?;
        CapturedSecret::new(value).into_secret().map_err(|error| {
            AppError::Credential(format!(
                "keychain credential '{}' for service '{}' and account '{}' has an invalid value: {error}; set it again with 'agentenv credential set {}'",
                self.credential_name, self.service, self.account, self.credential_name
            ))
        })
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
