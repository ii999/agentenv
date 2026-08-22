//! Test-feature-only file-backed keychain adapter for out-of-process tests.

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::config::env_value;
use crate::credential::{CapturedSecret, Secret};
use crate::error::AppError;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Record {
    service: String,
    account: String,
    value: Vec<u8>,
}

pub(crate) struct TestStore {
    path: PathBuf,
}

impl TestStore {
    pub(crate) fn from_environment() -> Option<Self> {
        Self::from_env_value(std::env::var_os("AGENT_CONTEXT_TEST_KEYCHAIN"))
    }

    fn from_env_value(value: Option<OsString>) -> Option<Self> {
        let environment = |_: &str| value.clone();
        env_value(&environment, "AGENT_CONTEXT_TEST_KEYCHAIN")
            .map(PathBuf::from)
            .map(|path| Self { path })
    }

    pub(crate) fn read(&self, service: &str, account: &str) -> Result<Option<Secret>, AppError> {
        let records = self.load()?;
        let Some(record) = records
            .into_iter()
            .find(|record| record.service == service && record.account == account)
        else {
            return Ok(None);
        };
        CapturedSecret::new(record.value)
            .into_secret()
            .map(Some)
            .map_err(|error| {
                AppError::Credential(format!(
                    "test keychain item for service '{}' and account '{}' is invalid: {error}; set it again with 'agent-context credential set'",
                    service, account
                ))
            })
    }

    pub(crate) fn write(
        &self,
        service: &str,
        account: &str,
        value: &Secret,
    ) -> Result<(), AppError> {
        let mut records = self.load()?;
        if let Some(record) = records
            .iter_mut()
            .find(|record| record.service == service && record.account == account)
        {
            record.value = value.as_bytes().to_vec();
        } else {
            records.push(Record {
                service: service.to_owned(),
                account: account.to_owned(),
                value: value.as_bytes().to_vec(),
            });
        }
        let contents = serde_json::to_vec(&records).map_err(|error| {
            AppError::Credential(format!(
                "cannot encode test keychain store '{}': {error}; choose a writable test-store path",
                self.path.display()
            ))
        })?;
        fs::write(&self.path, contents).map_err(|error| write_error(&self.path, error))
    }

    fn load(&self) -> Result<Vec<Record>, AppError> {
        match fs::read(&self.path) {
            Ok(contents) => serde_json::from_slice(&contents).map_err(|error| {
                AppError::Credential(format!(
                    "cannot read test keychain store '{}': {error}; use a fresh test-store path",
                    self.path.display()
                ))
            }),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
            Err(error) => Err(AppError::Credential(format!(
                "cannot read test keychain store '{}': {error}; verify the test-store path",
                self.path.display()
            ))),
        }
    }
}

fn write_error(path: &Path, error: std::io::Error) -> AppError {
    AppError::Credential(format!(
        "cannot write test keychain store '{}': {error}; verify the test-store path",
        path.display()
    ))
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;

    use super::TestStore;

    #[test]
    fn empty_test_store_path_is_unset() {
        assert!(TestStore::from_env_value(None).is_none());
        assert!(TestStore::from_env_value(Some(OsString::new())).is_none());
    }
}
