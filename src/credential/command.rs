use std::io::Read;
use std::process::{Command, Stdio};

use crate::credential::shallow::command_status;
use crate::credential::{
    CapturedSecret, ConfidentialError, Provider, Secret, SecretDomainError, Status,
    FILL_VALUE_LIMIT,
};
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

    fn resolve_confidential(&self, line_oriented: bool) -> Result<Secret, ConfidentialError> {
        if self.program().is_empty() {
            return Err(ConfidentialError::Execution);
        }
        let mut child = Command::new(self.program())
            .args(&self.argv[1..])
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .stdout(Stdio::piped())
            .spawn()
            .map_err(|_| ConfidentialError::Execution)?;
        // Fixed storage avoids leaving old allocations containing value
        // fragments when a growing Vec reallocates. The capture bound is two
        // bytes above the largest value any stage accepts, so a maximal value
        // followed by CRLF still classifies through the stage's own limit as
        // a value rejection rather than an execution failure. One further
        // byte detects overflow without ever accepting a truncated value.
        let limit = FILL_VALUE_LIMIT + 2;
        let mut bytes = zeroize::Zeroizing::new(vec![0_u8; limit + 1]);
        let mut length = 0;
        let mut stdout = child.stdout.take().ok_or(ConfidentialError::Execution)?;
        let mut overflow = false;
        loop {
            match stdout.read(&mut bytes[length..=limit]) {
                Ok(0) => break,
                Ok(read) if length + read <= limit => length += read,
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Ok(_) => {
                    overflow = true;
                    break;
                }
                Err(_) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(ConfidentialError::Execution);
                }
            }
        }
        if overflow {
            let _ = child.kill();
            let _ = child.wait();
            return Err(ConfidentialError::Value);
        }
        if !child
            .wait()
            .map_err(|_| ConfidentialError::Execution)?
            .success()
        {
            return Err(ConfidentialError::Execution);
        }
        let captured = CapturedSecret::new(bytes[..length].to_vec());
        let captured = if line_oriented {
            captured.strip_one_trailing_newline()
        } else {
            captured
        };
        // Empty output, NUL, or invalid UTF-8 means the provider produced no
        // usable value: a provider failure, as in ordinary resolution.
        captured
            .into_secret()
            .map_err(|_| ConfidentialError::Execution)
    }

    fn store(&self, _value: Secret) -> Result<(), AppError> {
        Err(AppError::Usage(format!(
            "command credentials are managed externally; update argv[0] '{}' and run 'agentenv credential check {}'",
            self.program(),
            self.credential_name
        )))
    }
}
