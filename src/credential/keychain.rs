use crate::credential::shallow::keychain_status;
use crate::credential::{ConfidentialError, Provider, Secret, Status};
use crate::error::AppError;

#[cfg(all(feature = "test-support", debug_assertions))]
use crate::credential::test_store::TestStore;
#[cfg(not(target_os = "macos"))]
use crate::credential::CapturedSecret;

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

    #[cfg(any(
        not(target_os = "macos"),
        all(feature = "test-support", debug_assertions)
    ))]
    fn read_error(&self, error: impl std::fmt::Display) -> AppError {
        AppError::Credential(format!(
            "keychain credential '{}' for service '{}' and account '{}' could not be read: {error}; check the platform keychain or use an env/command credential",
            self.credential_name, self.service, self.account
        ))
    }

    #[cfg(not(target_os = "macos"))]
    fn write_error(&self, error: impl std::fmt::Display) -> AppError {
        AppError::Credential(format!(
            "keychain credential '{}' for service '{}' and account '{}' could not be stored: {error}; check the platform keychain and retry 'agentenv credential set {}'",
            self.credential_name, self.service, self.account, self.credential_name
        ))
    }

    fn missing(&self) -> AppError {
        AppError::Credential(format!(
            "keychain credential '{}' is missing for service '{}' and account '{}'; set it with 'agentenv credential set {}'",
            self.credential_name, self.service, self.account, self.credential_name
        ))
    }

    fn read(&self) -> Result<Secret, AppError> {
        #[cfg(all(feature = "test-support", debug_assertions))]
        if let Some(store) = TestStore::from_environment() {
            return store
                .read(&self.service, &self.account)
                .map_err(|error| self.read_error(error))?
                .ok_or_else(|| self.missing());
        }
        #[cfg(target_os = "macos")]
        {
            crate::macos::client::read(&self.service, &self.account).map_err(|error| {
                if error.reason == crate::credential::KeychainReason::Missing {
                    self.missing()
                } else {
                    AppError::Keychain(error)
                }
            })
        }
        #[cfg(not(target_os = "macos"))]
        {
            let entry = keyring::Entry::new(&self.service, &self.account)
                .map_err(|error| self.read_error(error))?;
            // The password API decodes Windows' UTF-16 blob before the value
            // enters the UTF-8-only secret container.
            let value = entry.get_password().map_err(|error| match error {
                keyring::Error::NoEntry => self.missing(),
                other => self.read_error(other),
            })?;
            CapturedSecret::new(value.into_bytes()).into_secret().map_err(|error| {
                AppError::Credential(format!(
                    "keychain credential '{}' for service '{}' and account '{}' has an invalid value: {error}; set it again with 'agentenv credential set {}'",
                    self.credential_name, self.service, self.account, self.credential_name
                ))
            })
        }
    }
}

impl Provider for KeychainProvider {
    fn shallow_status(&self) -> Status {
        keychain_status()
    }

    fn resolve(&self) -> Result<Secret, AppError> {
        self.read()
    }

    fn resolve_confidential(&self, _line_oriented: bool) -> Result<Secret, ConfidentialError> {
        self.read().map_err(|error| match error {
            AppError::Keychain(error) => ConfidentialError::Keychain(error),
            _ => ConfidentialError::Execution,
        })
    }

    fn store(&self, value: Secret) -> Result<(), AppError> {
        #[cfg(all(feature = "test-support", debug_assertions))]
        if let Some(store) = TestStore::from_environment() {
            return store.write(&self.service, &self.account, &value);
        }
        #[cfg(target_os = "macos")]
        {
            crate::macos::client::store(&self.service, &self.account, &value)
                .map_err(AppError::Keychain)
        }
        #[cfg(not(target_os = "macos"))]
        {
            let entry = keyring::Entry::new(&self.service, &self.account)
                .map_err(|error| self.write_error(error))?;
            entry
                .set_password(value.as_str())
                .map_err(|error| self.write_error(error))
        }
    }
}
