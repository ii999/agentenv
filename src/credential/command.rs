use std::process::{Command, Stdio};

use crate::credential::shallow::command_status;
use crate::credential::{CapturedSecret, Provider, Secret, SecretDomainError, Status};
use crate::error::AppError;

pub(crate) struct CommandProvider {
    credential_name: String,
    argv: Vec<String>,
}

impl CommandProvider {
    pub(crate) fn new(credential_name: String, argv: Vec<String>) -> Self {
        Self {
            credential_name,
            argv,
        }
    }

    fn program(&self) -> &str {
        self.argv.first().map(String::as_str).unwrap_or_default()
    }
}

impl Provider for CommandProvider {
    fn shallow_status(&self) -> Status {
        let env = |name: &str| std::env::var(name).ok();
        command_status(self.program(), &env)
    }

    fn resolve(&self) -> Result<Secret, AppError> {
        let program = self.program();
        if program.is_empty() {
            return Err(AppError::Credential(format!(
                "command credential '{}' has no argv[0]; edit its credential definition and run 'agentenv credential check {}'",
                self.credential_name, self.credential_name
            )));
        }
        let output = Command::new(program)
            .args(&self.argv[1..])
            .stdin(Stdio::inherit())
            .stderr(Stdio::inherit())
            .stdout(Stdio::piped())
            .output()
            .map_err(|error| {
                AppError::Credential(format!(
                    "command credential '{}' could not start argv[0] '{}': {error}; verify the command and run 'agentenv credential check {}'",
                    self.credential_name, program, self.credential_name
                ))
            })?;
        if !output.status.success() {
            return Err(AppError::Credential(format!(
                "command credential '{}' exited unsuccessfully (argv[0] '{}'); fix the command and run 'agentenv credential check {}'",
                self.credential_name, program, self.credential_name
            )));
        }
        CapturedSecret::new(output.stdout)
            .strip_one_trailing_newline()
            .into_secret()
            .map_err(|error| match error {
                SecretDomainError::Empty => AppError::Credential(format!(
                    "command credential '{}' produced no output or only whitespace; fix argv[0] '{}' and run 'agentenv credential check {}'",
                    self.credential_name, program, self.credential_name
                )),
                _ => AppError::Credential(format!(
                    "command credential '{}' returned an invalid value: {error}; fix argv[0] '{}' and run 'agentenv credential check {}'",
                    self.credential_name, program, self.credential_name
                )),
            })
    }

    fn store(&self, _value: Secret) -> Result<(), AppError> {
        Err(AppError::Usage(format!(
            "command credentials are managed externally; update argv[0] '{}' and run 'agentenv credential check {}'",
            self.program(),
            self.credential_name
        )))
    }
}
