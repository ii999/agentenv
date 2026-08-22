use std::ffi::OsString;

use crate::config::env_value;
use crate::credential::shallow::env_status;
use crate::credential::{CapturedSecret, Provider, Secret, Status};
use crate::error::AppError;

pub(crate) struct EnvProvider {
    credential_name: String,
    variable: String,
}

impl EnvProvider {
    pub(crate) fn new(credential_name: String, variable: String) -> Self {
        Self {
            credential_name,
            variable,
        }
    }
}

impl Provider for EnvProvider {
    fn shallow_status(&self) -> Status {
        let env = |name: &str| std::env::var(name).ok();
        env_status(&self.variable, &env)
    }

    fn resolve(&self) -> Result<Secret, AppError> {
        let environment = |name: &str| std::env::var_os(name);
        let value = env_value(&environment, &self.variable).ok_or_else(|| {
            AppError::Credential(format!(
                "env credential '{}' is unset; set {} and run 'agentenv credential check {}'",
                self.credential_name, self.variable, self.credential_name
            ))
        })?;
        captured_from_os(value, &self.credential_name, &self.variable)
    }

    fn store(&self, _value: Secret) -> Result<(), AppError> {
        Err(AppError::Usage(format!(
            "env credentials are managed externally through {}; set the environment variable and run 'agentenv credential check {}'",
            self.variable, self.credential_name
        )))
    }
}

fn captured_from_os(
    value: OsString,
    credential_name: &str,
    variable: &str,
) -> Result<Secret, AppError> {
    #[cfg(unix)]
    use std::os::unix::ffi::OsStringExt;

    #[cfg(unix)]
    let bytes = value.into_vec();
    #[cfg(not(unix))]
    let bytes = value.to_string_lossy().as_bytes().to_vec();

    CapturedSecret::new(bytes).into_secret().map_err(|error| {
        AppError::Credential(format!(
            "env credential '{}' in {} is invalid: {error}; set a valid value and run 'agentenv credential check {}'",
            credential_name, variable, credential_name
        ))
    })
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;

    use super::captured_from_os;

    #[test]
    fn environment_values_preserve_trailing_newlines() {
        let secret = captured_from_os(OsString::from("abc\n"), "example", "EXAMPLE_SECRET")
            .expect("a non-empty environment value is valid");
        assert_eq!(secret.as_str(), "abc\n");
        assert_eq!(secret.as_str().len(), 4);
    }

    #[test]
    fn newline_only_environment_value_is_valid() {
        let secret = captured_from_os(OsString::from("\n"), "example", "EXAMPLE_SECRET")
            .expect("a newline is not an empty credential value");
        assert_eq!(secret.as_str(), "\n");
    }
}
