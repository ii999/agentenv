//! Password-safe sudo execution.
//!
//! The local executor keeps authentication on an invocation-scoped Unix
//! socket. Command stdin is connected directly to sudo and is never inspected
//! for authentication.

use std::future::Future;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use crate::error::AppError;

pub mod client;
pub mod deploy;
pub mod protocol;
pub mod remote;
pub mod ssh;
pub mod ssh_askpass;
pub mod transport;

#[cfg(unix)]
mod unix;

pub const PROTOCOL_VERSION: u16 = 1;
pub const SUDO_PASSWORD_LIMIT: usize = 255;
pub const DEFAULT_SETUP_TIMEOUT: Duration = Duration::from_secs(30);
pub const DEFAULT_AUTH_TIMEOUT: Duration = Duration::from_secs(60);

/// One validated, immutable command invocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutionRequest {
    executable: PathBuf,
    arguments: Vec<String>,
    cwd: Option<PathBuf>,
}

impl ExecutionRequest {
    pub fn new(
        executable: impl Into<PathBuf>,
        arguments: Vec<String>,
        cwd: Option<PathBuf>,
    ) -> Result<Self, AppError> {
        let executable = executable.into();
        validate_posix_absolute_utf8_path(&executable, "command executable")?;
        if let Some(cwd) = &cwd {
            validate_posix_absolute_utf8_path(cwd, "working directory")?;
        }
        if arguments.iter().any(|argument| argument.contains('\0')) {
            return Err(AppError::Usage(
                "command arguments cannot contain NUL bytes".to_owned(),
            ));
        }
        Ok(Self {
            executable,
            arguments,
            cwd,
        })
    }

    pub fn executable(&self) -> &Path {
        &self.executable
    }

    pub fn arguments(&self) -> &[String] {
        &self.arguments
    }

    pub fn cwd(&self) -> Option<&Path> {
        self.cwd.as_deref()
    }
}

fn validate_absolute_utf8_path(path: &Path, label: &str) -> Result<(), AppError> {
    if !path.is_absolute() {
        return Err(AppError::Usage(format!("{label} must be an absolute path")));
    }
    let Some(text) = path.to_str() else {
        return Err(AppError::Usage(format!("{label} must be valid UTF-8")));
    };
    if text.contains('\0') {
        return Err(AppError::Usage(format!("{label} cannot contain NUL bytes")));
    }
    Ok(())
}

fn validate_posix_absolute_utf8_path(path: &Path, label: &str) -> Result<(), AppError> {
    let Some(text) = path.to_str() else {
        return Err(AppError::Usage(format!("{label} must be valid UTF-8")));
    };
    if !text.starts_with('/') {
        return Err(AppError::Usage(format!(
            "{label} must be an absolute POSIX path"
        )));
    }
    if text.contains('\0') {
        return Err(AppError::Usage(format!("{label} cannot contain NUL bytes")));
    }
    Ok(())
}

/// Process streams supplied to the executor. They are passed through without
/// interpreting or buffering target bytes.
pub struct ProcessIo {
    pub stdin: Stdio,
    pub stdout: Stdio,
    pub stderr: Stdio,
}

impl ProcessIo {
    pub fn inherit() -> Self {
        Self {
            stdin: Stdio::inherit(),
            stdout: Stdio::inherit(),
            stderr: Stdio::inherit(),
        }
    }
}

/// A cancellation receiver. A changed positive Unix signal number requests
/// forwarding that signal to the owned sudo process group.
pub type Cancellation = tokio::sync::watch::Receiver<i32>;

pub fn cancellation_channel() -> (tokio::sync::watch::Sender<i32>, Cancellation) {
    tokio::sync::watch::channel(0)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExecutionOutcome {
    pub exit_code: Option<i32>,
    pub signal: Option<i32>,
    pub password_delivered: bool,
}

impl ExecutionOutcome {
    pub fn shell_status(self) -> i32 {
        self.exit_code
            .unwrap_or_else(|| 128_i32.saturating_add(self.signal.unwrap_or(1)))
    }
}

#[derive(Debug, Clone)]
pub struct LocalOptions {
    pub sudo_path: PathBuf,
    pub helper_path: PathBuf,
    pub auth_user: String,
    pub run_as: String,
    pub setup_timeout: Duration,
    pub auth_timeout: Duration,
}

impl LocalOptions {
    pub fn validate(&self) -> Result<(), AppError> {
        validate_absolute_utf8_path(&self.sudo_path, "sudo executable")?;
        validate_absolute_utf8_path(&self.helper_path, "sudo helper")?;
        if self.auth_user.is_empty()
            || self.run_as.is_empty()
            || self.auth_user.chars().any(char::is_control)
            || self.run_as.chars().any(char::is_control)
        {
            return Err(AppError::Usage(
                "sudo account names must be nonempty and contain no control characters".to_owned(),
            ));
        }
        if self.setup_timeout.is_zero()
            || self.auth_timeout.is_zero()
            || self.setup_timeout > Duration::from_secs(300)
            || self.auth_timeout > Duration::from_secs(300)
        {
            return Err(AppError::Usage(
                "sudo setup and authentication timeouts must be between 1 and 300 seconds"
                    .to_owned(),
            ));
        }
        Ok(())
    }
}

#[cfg(unix)]
pub async fn check_local(options: &LocalOptions) -> Result<(), AppError> {
    options.validate()?;
    unix::check(options).await
}

#[cfg(not(unix))]
pub async fn check_local(_options: &LocalOptions) -> Result<(), AppError> {
    Err(AppError::SudoExecution(
        "unsupported-platform: local sudo is unavailable on this platform".to_owned(),
    ))
}

/// Runs one local sudo command. The resolver is called at most once and only
/// after the helper proves the invocation prompt and peer account.
#[cfg(unix)]
pub async fn execute_local<F, Fut>(
    request: ExecutionRequest,
    options: LocalOptions,
    io: ProcessIo,
    cancellation: Cancellation,
    resolve_password: F,
) -> Result<ExecutionOutcome, AppError>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<crate::credential::Secret, AppError>>,
{
    options.validate()?;
    unix::execute(request, options, io, cancellation, resolve_password).await
}

#[cfg(not(unix))]
pub async fn execute_local<F, Fut>(
    _request: ExecutionRequest,
    _options: LocalOptions,
    _io: ProcessIo,
    _cancellation: Cancellation,
    _resolve_password: F,
) -> Result<ExecutionOutcome, AppError>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<crate::credential::Secret, AppError>>,
{
    Err(AppError::SudoExecution(
        "unsupported-platform: local sudo is unavailable on this platform".to_owned(),
    ))
}

pub fn companion_path(executable: &Path) -> Result<PathBuf, AppError> {
    let directory = executable.parent().ok_or_else(|| {
        AppError::SudoExecution(
            "helper-missing: agentenv executable has no parent directory".to_owned(),
        )
    })?;
    Ok(directory.join(format!(
        "{}{}",
        deploy::HELPER_FILE_NAME,
        std::env::consts::EXE_SUFFIX
    )))
}

#[cfg(test)]
mod tests {
    use super::ExecutionRequest;

    #[test]
    fn requests_use_destination_posix_path_grammar() {
        assert!(ExecutionRequest::new("/usr/bin/id", Vec::new(), None).is_ok());
        assert!(
            ExecutionRequest::new("C:\\Windows\\System32\\whoami.exe", Vec::new(), None).is_err()
        );
    }
}
