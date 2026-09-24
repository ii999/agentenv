//! Cancellable credential resolution over a private inherited session socket.
//!
//! The resolver never returns secret bytes on process stdout. Its process group
//! also owns command-provider descendants, so cancellation revokes their channel
//! and kills/reaps the resolver without waiting for an OS credential API.

use std::path::Path;
use std::time::Duration;

use super::Secret;
use crate::config::{CredentialDef, CredentialUsage};
use crate::error::AppError;

#[derive(Clone, Copy)]
pub enum AuthenticationStage {
    Sudo,
    SshPassword,
}

impl AuthenticationStage {
    fn usage(self) -> CredentialUsage {
        match self {
            Self::Sudo => CredentialUsage::Sudo,
            Self::SshPassword => CredentialUsage::SshPassword,
        }
    }
}

fn failure() -> AppError {
    AppError::Credential(
        "authentication credential unavailable; check its provider and permitted usage separately"
            .to_owned(),
    )
}

/// Resolves one authorized authentication value. Dropping this future cancels
/// the owned resolver; callers retain ownership of response deadlines.
pub async fn resolve(
    executable: &Path,
    definition: &CredentialDef,
    stage: AuthenticationStage,
    limit: usize,
    deadline: Duration,
) -> Result<Secret, AppError> {
    if !definition.permits(stage.usage()) || !(1..=255).contains(&limit) || deadline.is_zero() {
        return Err(failure());
    }
    #[cfg(unix)]
    {
        tokio::time::timeout(deadline, unix::resolve(executable, definition, limit))
            .await
            .map_err(|_| failure())?
    }
    #[cfg(not(unix))]
    {
        let _ = (executable, definition, deadline);
        Err(AppError::Credential(
            "confidential resolver is unavailable on this platform".to_owned(),
        ))
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
    #[cfg(not(unix))]
    Some(9)
}

#[cfg(unix)]
mod unix {
    use std::io::{Read, Write};
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::net::UnixStream;
    use std::os::unix::process::CommandExt;
    use std::process::{Child, Command, Stdio};

    use serde::{Deserialize, Serialize};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use zeroize::Zeroizing;

    use super::*;
    use crate::config::Provider;
    use crate::credential::{provider_for_with_io, CapturedSecret, ResolutionIo};

    const MAGIC: &[u8; 5] = b"AGER\x01";
    const MAX_REQUEST: usize = 65536;

    #[derive(Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Request {
        provider: ProviderRequest,
        limit: usize,
    }

    #[derive(Serialize, Deserialize)]
    #[serde(tag = "kind", deny_unknown_fields)]
    enum ProviderRequest {
        Keychain { service: String, account: String },
        Command { argv: Vec<String> },
    }

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
        limit: usize,
    ) -> Result<Secret, AppError> {
        let provider = match &definition.provider {
            Provider::Keychain { service, account } => ProviderRequest::Keychain {
                service: service.clone(),
                account: account.clone(),
            },
            Provider::Command { argv } => ProviderRequest::Command { argv: argv.clone() },
            Provider::Env { .. } => return Err(failure()),
        };
        let request = serde_json::to_vec(&Request { provider, limit }).map_err(|_| failure())?;
        if request.len() > MAX_REQUEST {
            return Err(failure());
        }
        let (parent, child) = UnixStream::pair().map_err(|_| failure())?;
        let child_fd = child.as_raw_fd();
        let mut command = Command::new(executable);
        command
            .arg("--agentenv-resolver")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .process_group(0);
        // Preserve provider environment compatibility while disabling owned
        // diagnostic/debug hooks. No password is added to this environment.
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
        let _resolver = Resolver(command.spawn().map_err(|_| failure())?);
        drop(child);
        parent.set_nonblocking(true).map_err(|_| failure())?;
        let mut channel = tokio::net::UnixStream::from_std(parent).map_err(|_| failure())?;
        channel.write_all(MAGIC).await.map_err(|_| failure())?;
        channel
            .write_u32(request.len() as u32)
            .await
            .map_err(|_| failure())?;
        channel.write_all(&request).await.map_err(|_| failure())?;
        let mut response = [0u8; 5];
        channel
            .read_exact(&mut response)
            .await
            .map_err(|_| failure())?;
        if &response != MAGIC {
            return Err(failure());
        }
        let size = channel.read_u16().await.map_err(|_| failure())? as usize;
        if size == 0 || size > limit {
            return Err(failure());
        }
        let mut bytes = Zeroizing::new(vec![0; size]);
        channel
            .read_exact(&mut bytes)
            .await
            .map_err(|_| failure())?;
        let secret = CapturedSecret::new(std::mem::take(&mut *bytes))
            .into_secret()
            .map_err(|_| failure())?;
        secret
            .validate_authentication(limit)
            .map_err(|_| failure())?;
        Ok(secret)
    }

    fn private_channel() -> Result<UnixStream, AppError> {
        let mut metadata = std::mem::MaybeUninit::<libc::stat>::uninit();
        // Validate the inherited descriptor before taking ownership. Never
        // accept a regular file, stdout pipe, terminal, or network socket.
        unsafe {
            if libc::fstat(3, metadata.as_mut_ptr()) != 0
                || metadata.assume_init().st_mode & libc::S_IFMT != libc::S_IFSOCK
            {
                return Err(failure());
            }
        }
        let channel = unsafe { UnixStream::from_raw_fd(3) };
        unsafe {
            let mut address = std::mem::MaybeUninit::<libc::sockaddr_storage>::uninit();
            let mut length = std::mem::size_of::<libc::sockaddr_storage>() as libc::socklen_t;
            if libc::getpeername(3, address.as_mut_ptr().cast(), &mut length) != 0
                || address.assume_init().ss_family as i32 != libc::AF_UNIX
            {
                return Err(failure());
            }
        }
        #[cfg(target_os = "linux")]
        unsafe {
            let mut credentials = std::mem::MaybeUninit::<libc::ucred>::uninit();
            let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
            if libc::getsockopt(
                3,
                libc::SOL_SOCKET,
                libc::SO_PEERCRED,
                credentials.as_mut_ptr().cast(),
                &mut len,
            ) != 0
                || credentials.assume_init().uid != libc::getuid()
            {
                return Err(failure());
            }
        }
        #[cfg(not(target_os = "linux"))]
        unsafe {
            let (mut uid, mut gid) = (0, 0);
            if libc::getpeereid(3, &mut uid, &mut gid) != 0 || uid != libc::getuid() {
                return Err(failure());
            }
        }
        // Providers must not inherit the resolver capability.
        if unsafe { libc::fcntl(3, libc::F_SETFD, libc::FD_CLOEXEC) } == -1 {
            return Err(failure());
        }
        Ok(channel)
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
        let request: Request = serde_json::from_slice(&metadata).map_err(|_| failure())?;
        if !(1..=255).contains(&request.limit) {
            return Err(failure());
        }
        let provider = match request.provider {
            ProviderRequest::Keychain { service, account } => {
                Provider::Keychain { service, account }
            }
            ProviderRequest::Command { argv } if !argv.is_empty() => Provider::Command { argv },
            _ => return Err(failure()),
        };
        let definition = CredentialDef {
            name: "authentication".to_owned(),
            description: String::new(),
            provider,
            inject_as: None,
            usages: vec![CredentialUsage::Sudo],
        };
        let secret = provider_for_with_io(
            &definition,
            ResolutionIo::Confidential {
                max_bytes: request.limit,
            },
        )
        .resolve()
        .map_err(|_| failure())?;
        secret
            .validate_authentication(request.limit)
            .map_err(|_| failure())?;
        channel.write_all(MAGIC).map_err(|_| failure())?;
        channel
            .write_all(&(secret.as_str().len() as u16).to_be_bytes())
            .map_err(|_| failure())?;
        channel
            .write_all(secret.as_str().as_bytes())
            .map_err(|_| failure())?;
        Ok(())
    }
}
