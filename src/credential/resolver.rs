//! Cancellable credential resolution over a private inherited session socket.
//!
//! The resolver never returns secret bytes on process stdout. Its process group
//! also owns command-provider descendants, so cancellation revokes their channel
//! and kills/reaps the resolver without waiting for an OS credential API.
//!
//! Each caller names a resolution stage. The stage fixes the credential usage
//! it may consume, the byte limit, and the value validation applied on both
//! sides of the channel: authentication stages accept single-line values of
//! at most 255 bytes; the fill stage accepts single-line values of at most
//! [`FILL_VALUE_LIMIT`](crate::credential::FILL_VALUE_LIMIT) bytes.

use std::fmt;
use std::path::Path;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::{KeychainFailure, Secret, SecretDomainError, FILL_VALUE_LIMIT};

#[cfg(windows)]
mod windows;
use crate::config::{CredentialDef, CredentialUsage};
use crate::error::AppError;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ResolutionStage {
    Sudo,
    SshPassword,
    Fill,
}

impl ResolutionStage {
    fn usage(self) -> CredentialUsage {
        match self {
            Self::Sudo => CredentialUsage::Sudo,
            Self::SshPassword => CredentialUsage::SshPassword,
            Self::Fill => CredentialUsage::Environment,
        }
    }

    fn max_limit(self) -> usize {
        match self {
            Self::Sudo | Self::SshPassword => 255,
            Self::Fill => FILL_VALUE_LIMIT,
        }
    }

    fn validate(self, secret: &Secret, limit: usize) -> Result<(), SecretDomainError> {
        match self {
            Self::Sudo | Self::SshPassword => secret.validate_authentication(limit),
            Self::Fill => secret.validate_fill(limit),
        }
    }
}

/// Why a resolution produced no value. Variants carry no candidate bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResolveError {
    /// The definition does not permit the stage's usage, or the request is
    /// outside the stage's limits.
    NotPermitted,
    /// The provider failed, produced nothing, or the channel broke.
    Provider,
    /// The provider produced a value the stage cannot accept.
    Value,
    /// The deadline expired before a value arrived.
    Timeout,
    /// The macOS helper reported a classified failure without candidate bytes.
    Keychain(KeychainFailure),
}

impl fmt::Display for ResolveError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Self::Keychain(error) = self {
            return error.fmt(formatter);
        }
        formatter.write_str(match self {
            Self::NotPermitted => "the credential does not permit this use",
            Self::Provider => "the credential provider failed",
            Self::Value => "the credential value is not supported for this use",
            Self::Timeout => "the credential lookup deadline expired",
            Self::Keychain(_) => unreachable!("Keychain diagnostics are rendered above"),
        })
    }
}

impl From<ResolveError> for AppError {
    fn from(error: ResolveError) -> Self {
        match error {
            ResolveError::Keychain(error) => Self::Keychain(error),
            _ => failure(),
        }
    }
}

fn failure() -> AppError {
    AppError::Credential(
        "authentication credential unavailable; check its provider and permitted usage separately"
            .to_owned(),
    )
}

/// Resolves one authorized value. Dropping this future cancels the owned
/// resolver; callers retain ownership of response deadlines.
pub async fn resolve(
    executable: &Path,
    definition: &CredentialDef,
    stage: ResolutionStage,
    limit: usize,
    deadline: Duration,
) -> Result<Secret, AppError> {
    resolve_detailed(executable, definition, stage, limit, deadline)
        .await
        .map_err(AppError::from)
}

/// [`resolve`] with a typed error, for callers that report value rejection
/// separately from provider failure.
pub async fn resolve_detailed(
    executable: &Path,
    definition: &CredentialDef,
    stage: ResolutionStage,
    limit: usize,
    deadline: Duration,
) -> Result<Secret, ResolveError> {
    if !definition.permits(stage.usage())
        || !(1..=stage.max_limit()).contains(&limit)
        || deadline.is_zero()
    {
        return Err(ResolveError::NotPermitted);
    }
    #[cfg(unix)]
    {
        tokio::time::timeout(
            deadline,
            unix::resolve(executable, definition, stage, limit),
        )
        .await
        .map_err(|_| ResolveError::Timeout)?
    }
    #[cfg(windows)]
    {
        tokio::time::timeout(
            deadline,
            windows::resolve(executable, definition, stage, limit),
        )
        .await
        .map_err(|_| ResolveError::Timeout)?
    }
}

/// Called before public CLI parsing. Only the fixed internal invocation is
/// recognized; it requires a connected same-owner Unix socket on descriptor 3.
pub fn internal_entry() -> Option<i32> {
    let mut args = std::env::args_os().skip(1);
    if args.next().as_deref() != Some(std::ffi::OsStr::new("--agentenv-resolver")) {
        return None;
    }
    if args.next().is_some() {
        return Some(9);
    }
    #[cfg(unix)]
    return Some(if unix::serve().is_ok() { 0 } else { 9 });
    #[cfg(windows)]
    return Some(if windows::serve().is_ok() { 0 } else { 9 });
}

use crate::config::Provider;
const MAGIC: &[u8; 5] = b"AGER\x03";
const MAX_REQUEST: usize = 65536;
const STATUS_OK: u8 = 0;
const STATUS_PROVIDER: u8 = 1;
const STATUS_VALUE: u8 = 2;
const STATUS_KEYCHAIN: u8 = 3;
const DIAGNOSTIC_LIMIT: usize = 1024;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    provider: ProviderRequest,
    stage: ResolutionStage,
    limit: usize,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", deny_unknown_fields)]
enum ProviderRequest {
    Keychain { service: String, account: String },
    Command { argv: Vec<String> },
}

fn encode_request(
    definition: &CredentialDef,
    stage: ResolutionStage,
    limit: usize,
) -> Result<Vec<u8>, ResolveError> {
    let provider = match &definition.provider {
        Provider::Keychain { service, account } => ProviderRequest::Keychain {
            service: service.clone(),
            account: account.clone(),
        },
        Provider::Command { argv } => ProviderRequest::Command { argv: argv.clone() },
        Provider::Env { .. } => return Err(ResolveError::NotPermitted),
    };
    let request = serde_json::to_vec(&Request {
        provider,
        stage,
        limit,
    })
    .map_err(|_| ResolveError::Provider)?;
    if request.len() > MAX_REQUEST {
        return Err(ResolveError::NotPermitted);
    }
    Ok(request)
}

async fn exchange<S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin>(
    channel: &mut S,
    request: &[u8],
    stage: ResolutionStage,
    limit: usize,
) -> Result<Secret, ResolveError> {
    use crate::credential::CapturedSecret;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use zeroize::Zeroizing;
    channel
        .write_all(MAGIC)
        .await
        .map_err(|_| ResolveError::Provider)?;
    channel
        .write_u32(request.len() as u32)
        .await
        .map_err(|_| ResolveError::Provider)?;
    channel
        .write_all(request)
        .await
        .map_err(|_| ResolveError::Provider)?;
    let mut response = [0u8; 5];
    channel
        .read_exact(&mut response)
        .await
        .map_err(|_| ResolveError::Provider)?;
    if &response != MAGIC {
        return Err(ResolveError::Provider);
    }
    match channel
        .read_u8()
        .await
        .map_err(|_| ResolveError::Provider)?
    {
        STATUS_OK => {}
        STATUS_VALUE => return Err(ResolveError::Value),
        STATUS_KEYCHAIN => {
            let length = channel
                .read_u16()
                .await
                .map_err(|_| ResolveError::Provider)? as usize;
            if length == 0 || length > DIAGNOSTIC_LIMIT {
                return Err(ResolveError::Provider);
            }
            let mut diagnostic = vec![0; length];
            channel
                .read_exact(&mut diagnostic)
                .await
                .map_err(|_| ResolveError::Provider)?;
            let failure =
                serde_json::from_slice(&diagnostic).map_err(|_| ResolveError::Provider)?;
            return Err(ResolveError::Keychain(failure));
        }
        _ => return Err(ResolveError::Provider),
    }
    let size = channel
        .read_u16()
        .await
        .map_err(|_| ResolveError::Provider)? as usize;
    if size == 0 || size > limit {
        return Err(ResolveError::Provider);
    }
    let mut bytes = Zeroizing::new(vec![0; size]);
    channel
        .read_exact(&mut bytes)
        .await
        .map_err(|_| ResolveError::Provider)?;
    // The resolver validated the bytes; a value that no longer parses is
    // a resolver failure, not a rejected value.
    let secret = CapturedSecret::new(std::mem::take(&mut *bytes))
        .into_secret()
        .map_err(|_| ResolveError::Provider)?;
    stage
        .validate(&secret, limit)
        .map_err(|_| ResolveError::Value)?;
    Ok(secret)
}

enum ProviderFailure {
    Provider,
    Value,
    Keychain(KeychainFailure),
}

fn failure_payload(error: &ProviderFailure) -> Result<Vec<u8>, AppError> {
    match error {
        ProviderFailure::Provider => Ok(vec![STATUS_PROVIDER]),
        ProviderFailure::Value => Ok(vec![STATUS_VALUE]),
        ProviderFailure::Keychain(error) => {
            let encoded = serde_json::to_vec(error).map_err(|_| failure())?;
            if encoded.len() > DIAGNOSTIC_LIMIT {
                return Err(failure());
            }
            let mut payload = vec![STATUS_KEYCHAIN];
            payload.extend_from_slice(&(encoded.len() as u16).to_be_bytes());
            payload.extend_from_slice(&encoded);
            Ok(payload)
        }
    }
}

fn resolve_request(metadata: &[u8]) -> Result<Secret, ProviderFailure> {
    use crate::credential::{provider_for, ConfidentialError};
    let request: Request =
        serde_json::from_slice(metadata).map_err(|_| ProviderFailure::Provider)?;
    let stage = request.stage;
    if !(1..=stage.max_limit()).contains(&request.limit) {
        return Err(ProviderFailure::Provider);
    }
    let provider = match request.provider {
        ProviderRequest::Keychain { service, account } => Provider::Keychain { service, account },
        ProviderRequest::Command { argv } if !argv.is_empty() => Provider::Command { argv },
        _ => return Err(ProviderFailure::Provider),
    };
    let definition = CredentialDef {
        name: "resolution".to_owned(),
        description: String::new(),
        provider,
        inject_as: None,
        usages: vec![stage.usage()],
    };
    // The fill stage keeps the line-oriented value semantics environment
    // credentials have under ordinary resolution; authentication stages
    // keep raw bytes and reject line endings through their validation.
    let line_oriented = stage == ResolutionStage::Fill;
    let secret = match provider_for(&definition).resolve_confidential(line_oriented) {
        Ok(secret) => secret,
        Err(ConfidentialError::Execution) => return Err(ProviderFailure::Provider),
        Err(ConfidentialError::Value) => return Err(ProviderFailure::Value),
        Err(ConfidentialError::Keychain(error)) => return Err(ProviderFailure::Keychain(error)),
    };
    if stage.validate(&secret, request.limit).is_err() {
        return Err(ProviderFailure::Value);
    }
    Ok(secret)
}

#[cfg(unix)]
mod unix {
    use std::io::{Read, Write};
    use std::os::fd::AsRawFd;
    use std::os::unix::net::UnixStream;
    use std::os::unix::process::CommandExt;
    use std::process::{Child, Command, Stdio};

    use super::*;

    struct Resolver(Child);

    impl Drop for Resolver {
        fn drop(&mut self) {
            // The child was spawned in a new process group and is not reaped
            // anywhere else. Its pid cannot be reused before this wait.
            unsafe {
                libc::kill(-(self.0.id() as i32), libc::SIGKILL);
            }
            let _ = self.0.wait();
        }
    }

    pub(super) async fn resolve(
        executable: &Path,
        definition: &CredentialDef,
        stage: ResolutionStage,
        limit: usize,
    ) -> Result<Secret, ResolveError> {
        let request = encode_request(definition, stage, limit)?;
        let (parent, child) = UnixStream::pair().map_err(|_| ResolveError::Provider)?;
        let child_fd = child.as_raw_fd();
        let mut command = Command::new(executable);
        command
            .arg("--agentenv-resolver")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .process_group(0);
        // Preserve provider environment compatibility while disabling owned
        // diagnostic/debug hooks. No secret is added to this environment.
        for name in [
            "RUST_LOG",
            "RUST_BACKTRACE",
            "RUST_LIB_BACKTRACE",
            "DYLD_INSERT_LIBRARIES",
            "LD_PRELOAD",
            "LD_AUDIT",
        ] {
            command.env_remove(name);
        }
        unsafe {
            command.pre_exec(move || {
                if child_fd != 3 && libc::dup2(child_fd, 3) == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                if libc::fcntl(3, libc::F_SETFD, 0) == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let _resolver = Resolver(command.spawn().map_err(|_| ResolveError::Provider)?);
        drop(child);
        parent
            .set_nonblocking(true)
            .map_err(|_| ResolveError::Provider)?;
        let mut channel =
            tokio::net::UnixStream::from_std(parent).map_err(|_| ResolveError::Provider)?;
        exchange(&mut channel, &request, stage, limit).await
    }

    fn private_channel() -> Result<UnixStream, AppError> {
        crate::credential::channel::inherited_socket(3).map_err(|_| failure())
    }

    pub(super) fn serve() -> Result<(), AppError> {
        let mut channel = private_channel()?;
        channel
            .set_read_timeout(Some(Duration::from_secs(300)))
            .map_err(|_| failure())?;
        channel
            .set_write_timeout(Some(Duration::from_secs(300)))
            .map_err(|_| failure())?;
        let mut magic = [0; 5];
        channel.read_exact(&mut magic).map_err(|_| failure())?;
        if &magic != MAGIC {
            return Err(failure());
        }
        let mut length = [0; 4];
        channel.read_exact(&mut length).map_err(|_| failure())?;
        let length = u32::from_be_bytes(length) as usize;
        if length == 0 || length > MAX_REQUEST {
            return Err(failure());
        }
        let mut metadata = vec![0; length];
        channel.read_exact(&mut metadata).map_err(|_| failure())?;
        let secret = match resolve_request(&metadata) {
            Ok(secret) => secret,
            Err(error) => return reply_failure(&mut channel, &error),
        };
        channel.write_all(MAGIC).map_err(|_| failure())?;
        channel.write_all(&[STATUS_OK]).map_err(|_| failure())?;
        channel
            .write_all(&(secret.as_str().len() as u16).to_be_bytes())
            .map_err(|_| failure())?;
        channel
            .write_all(secret.as_str().as_bytes())
            .map_err(|_| failure())?;
        Ok(())
    }

    fn reply_failure(channel: &mut UnixStream, error: &ProviderFailure) -> Result<(), AppError> {
        channel.write_all(MAGIC).map_err(|_| failure())?;
        channel
            .write_all(&failure_payload(error)?)
            .map_err(|_| failure())?;
        Err(failure())
    }
}
