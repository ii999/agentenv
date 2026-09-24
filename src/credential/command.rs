use std::io::Read;
use std::process::{Command, Stdio};

use super::ResolutionIo;
use crate::credential::shallow::command_status;
use crate::credential::{CapturedSecret, Provider, Secret, SecretDomainError, Status};
use crate::error::AppError;

pub(crate) struct CommandProvider {
    credential_name: String,
    argv: Vec<String>,
    io: ResolutionIo,
}

impl CommandProvider {
    pub(crate) fn new(credential_name: String, argv: Vec<String>, io: ResolutionIo) -> Self {
        Self {
            credential_name,
            argv,
            io,
        }
    }

    fn program(&self) -> &str {
        self.argv.first().map(String::as_str).unwrap_or_default()
    }

    fn resolve_confidential(&self, max_bytes: usize) -> Result<Secret, AppError> {
        let failure = || {
            AppError::Credential(
                "authentication provider failed; check the configured provider separately"
                    .to_owned(),
            )
        };
        let mut child = Command::new(self.program())
            .args(&self.argv[1..])
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .stdout(Stdio::piped())
            .spawn()
            .map_err(|_| failure())?;
        // Fixed storage avoids leaving old allocations containing password
        // fragments when a growing Vec reallocates. One extra byte detects
        // overflow without ever accepting a truncated authentication value.
        let limit = max_bytes.min(255);
        let mut bytes = zeroize::Zeroizing::new([0_u8; 256]);
        let mut length = 0;
        let mut stdout = child.stdout.take().ok_or_else(failure)?;
        loop {
            match stdout.read(&mut bytes[length..=limit]) {
                Ok(0) => break,
                Ok(read) if length + read <= limit => length += read,
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                _ => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(failure());
                }
            }
        }
        if !child.wait().map_err(|_| failure())?.success() {
            return Err(failure());
        }
        CapturedSecret::new(bytes[..length].to_vec())
            .into_secret()
            .map_err(|_| failure())
    }
}

impl Provider for CommandProvider {
    fn shallow_status(&self) -> Status {
        let env = |name: &str| std::env::var(name).ok();
        command_status(self.program(), &env)
    }

    fn resolve(&self) -> Result<Secret, AppError> {
        if let ResolutionIo::Confidential { max_bytes } = self.io {
            return self.resolve_confidential(max_bytes);
        }
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
