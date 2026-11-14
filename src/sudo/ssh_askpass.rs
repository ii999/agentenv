//! A single local SSH login challenge, independent of the sudo challenge.

use super::askpass_channel::SSH_LOGIN;
use crate::error::AppError;

/// Version of the askpass companion's `--identity` contract.
const ASKPASS_PROTOCOL_VERSION: u16 = 1;

/// The identity line, without its newline, that the askpass companion of
/// release `version` reports for `--identity`.
pub fn identity_for(version: impl std::fmt::Display) -> String {
    format!("agentenv-ssh-askpass {ASKPASS_PROTOCOL_VERSION} {version}")
}

/// The exact `--identity` output of this build's askpass companion.
fn identity_line() -> String {
    format!("{}\n", identity_for(env!("CARGO_PKG_VERSION")))
}

/// Start and, once it ends, end of the credential response, which the SSH
/// coordinator excludes from its setup budget.
type Timing = Option<(tokio::time::Instant, Option<tokio::time::Instant>)>;

fn failure() -> AppError {
    super::SudoError::new(
        super::Reason::SshAskpassRejected,
        "the SSH login challenge was rejected",
    )
    .into()
}

fn credential_failure() -> AppError {
    AppError::Credential(
        "SSH authentication credential unavailable; check its provider separately".into(),
    )
}

/// Records the credential response's start, and its end when dropped.
struct ActiveAuthentication<'a> {
    timing: &'a tokio::sync::watch::Sender<Timing>,
    started: tokio::time::Instant,
}

impl<'a> ActiveAuthentication<'a> {
    fn start(timing: &'a tokio::sync::watch::Sender<Timing>) -> Self {
        let started = tokio::time::Instant::now();
        timing.send_replace(Some((started, None)));
        Self { timing, started }
    }
}

impl Drop for ActiveAuthentication<'_> {
    fn drop(&mut self) {
        self.timing
            .send_replace(Some((self.started, Some(tokio::time::Instant::now()))));
    }
}

/// Runs the companion as `command`, already set up with `--identity` and a
/// piped stdout, and requires this build's exact identity line and a
/// successful exit within `timeout`.
async fn verify_companion(
    mut command: tokio::process::Command,
    timeout: std::time::Duration,
) -> Result<(), AppError> {
    use tokio::io::AsyncReadExt;
    let expected = identity_line();
    let mut child = command.spawn().map_err(|_| failure())?;
    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(failure)?
        .take(expected.len() as u64 + 1);
    let result = tokio::time::timeout(timeout, async {
        let mut bytes = Vec::new();
        stdout
            .read_to_end(&mut bytes)
            .await
            .map_err(|_| failure())?;
        if bytes != expected.as_bytes() || !child.wait().await.map_err(|_| failure())?.success() {
            return Err(failure());
        }
        Ok(())
    })
    .await;
    if !matches!(result, Ok(Ok(()))) {
        let _ = child.start_kill();
        let _ = child.wait().await;
        return Err(failure());
    }
    Ok(())
}

/// Reads the companion's request and requires this session's marker, the
/// expected prompt, and the SSH process as the companion's parent.
async fn read_request(
    channel: &mut (impl tokio::io::AsyncRead + Unpin),
    marker: &str,
    expected_prompt: &str,
    ssh_pid: u32,
) -> Result<(), AppError> {
    use tokio::io::AsyncReadExt;
    let received = SSH_LOGIN.read_field(channel).await.map_err(|_| failure())?;
    let prompt = SSH_LOGIN.read_field(channel).await.map_err(|_| failure())?;
    let parent = channel.read_u32().await.map_err(|_| failure())?;
    if received.as_slice() != marker.as_bytes()
        || prompt.as_slice() != expected_prompt.as_bytes()
        || parent != ssh_pid
    {
        return Err(failure());
    }
    Ok(())
}

/// Resolves the login password and writes it as the channel's one reply.
async fn deliver<F, Fut>(
    channel: &mut (impl tokio::io::AsyncWrite + Unpin),
    resolve: F,
) -> Result<(), AppError>
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = Result<crate::credential::Secret, AppError>>,
{
    let secret = resolve().await.map_err(|error| match error {
        AppError::Keychain(_) => error,
        _ => credential_failure(),
    })?;
    secret
        .validate_authentication(super::SUDO_PASSWORD_LIMIT)
        .map_err(|_| credential_failure())?;
    SSH_LOGIN
        .write_secret(channel, &secret)
        .await
        .map_err(|_| failure())
}

/// The companion's request: the session marker and the prompt, then the
/// companion's parent process id.
fn encode_request(marker: &str, prompt: &str, parent: u32) -> Result<Vec<u8>, ()> {
    let mut request = SSH_LOGIN.encode_field(marker.as_bytes()).map_err(|_| ())?;
    request.extend(SSH_LOGIN.encode_field(prompt.as_bytes()).map_err(|_| ())?);
    request.extend_from_slice(&parent.to_be_bytes());
    Ok(request)
}

/// Prints the received login password for ssh, as its askpass contract
/// requires.
fn print_secret(bytes: zeroize::Zeroizing<Vec<u8>>) -> Result<(), ()> {
    use std::io::Write;
    let secret = crate::credential::CapturedSecret::new(bytes.to_vec())
        .into_secret()
        .map_err(|_| ())?;
    secret
        .validate_authentication(super::SUDO_PASSWORD_LIMIT)
        .map_err(|_| ())?;
    let mut output = std::io::stdout().lock();
    output
        .write_all(secret.as_str().as_bytes())
        .map_err(|_| ())?;
    output.write_all(b"\n").map_err(|_| ())?;
    output.flush().map_err(|_| ())
}

#[cfg(unix)]
pub use unix::Session;
#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::Session;

#[cfg(unix)]
mod unix {
    use super::*;
    use std::fs;
    use std::future::Future;
    use std::io::{Read, Write};
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    use std::path::{Path, PathBuf};
    use std::process::Stdio;
    use std::time::Duration;
    use tokio::io::AsyncWriteExt;
    use tokio::net::UnixListener;
    use tokio::process::Command;
    use zeroize::Zeroizing;

    use crate::credential::Secret;

    const SOCKET_ENV: &str = "AGENTENV_SSH_SOCKET";
    const SESSION_ENV: &str = "AGENTENV_SSH_SESSION";

    pub struct Session {
        directory: tempfile::TempDir,
        socket: PathBuf,
        marker: String,
        helper: PathBuf,
        listener: UnixListener,
        timing: tokio::sync::watch::Sender<Timing>,
    }

    impl Session {
        /// Checks the installed companion before opening a login capability.
        pub async fn create(helper: &Path, timeout: Duration) -> Result<Self, AppError> {
            let metadata = fs::symlink_metadata(helper).map_err(|_| failure())?;
            let uid = unsafe { libc::getuid() };
            if !helper.is_absolute()
                || !metadata.is_file()
                || metadata.file_type().is_symlink()
                || (metadata.uid() != uid && metadata.uid() != 0)
                || metadata.mode() & 0o022 != 0
                || metadata.mode() & 0o111 == 0
            {
                return Err(failure());
            }
            let mut command = Command::new(helper);
            command
                .arg("--identity")
                .env_clear()
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .kill_on_drop(true);
            verify_companion(command, timeout).await?;
            let directory = tempfile::Builder::new()
                .prefix("agentenv-ssh-")
                .tempdir()
                .map_err(|_| failure())?;
            fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o700))
                .map_err(|_| failure())?;
            let metadata = fs::symlink_metadata(directory.path()).map_err(|_| failure())?;
            if !metadata.is_dir() || metadata.uid() != uid || metadata.mode() & 0o077 != 0 {
                return Err(failure());
            }
            let socket = directory.path().join("login");
            let listener = UnixListener::bind(&socket).map_err(|_| failure())?;
            fs::set_permissions(&socket, fs::Permissions::from_mode(0o600))
                .map_err(|_| failure())?;
            let marker = crate::random::nonce().map_err(|_| failure())?;
            Ok(Self {
                directory,
                socket,
                marker,
                helper: helper.to_owned(),
                listener,
                timing: tokio::sync::watch::channel(None).0,
            })
        }

        /// Reports exact start/end instants so the coordinator can exclude only
        /// validated credential-response time from its setup budget.
        pub fn auth_timing(&self) -> tokio::sync::watch::Receiver<Timing> {
            self.timing.subscribe()
        }

        pub fn configure(&self, command: &mut Command) {
            command
                .env("SSH_ASKPASS", &self.helper)
                .env("SSH_ASKPASS_REQUIRE", "force")
                .env("DISPLAY", "agentenv:0")
                .env_remove("SSH_ASKPASS_PROMPT")
                .env(SOCKET_ENV, &self.socket)
                .env(SESSION_ENV, &self.marker);
        }

        /// Dropping this future revokes both the broker and a pending resolver.
        /// The owner must terminate/reap its SSH child on an error.
        pub async fn serve<F, Fut>(
            self,
            ssh_pid: u32,
            expected_prompt: &str,
            setup_timeout: Duration,
            auth_timeout: Duration,
            resolve: F,
        ) -> Result<(), AppError>
        where
            F: FnOnce() -> Fut,
            Fut: Future<Output = Result<Secret, AppError>>,
        {
            let Self {
                directory,
                socket,
                marker,
                helper: _,
                listener,
                timing,
            } = self;
            let (mut stream, _) = listener.accept().await.map_err(|_| failure())?;
            tokio::time::timeout(setup_timeout, async {
                if stream.peer_cred().map_err(|_| failure())?.uid() != unsafe { libc::getuid() } {
                    return Err(failure());
                }
                read_request(&mut stream, &marker, expected_prompt, ssh_pid).await
            })
            .await
            .map_err(|_| failure())??;
            // No second askpass process can connect, even while resolution runs.
            fs::remove_file(&socket).map_err(|_| failure())?;
            drop(listener);
            let _active = ActiveAuthentication::start(&timing);
            let result = tokio::time::timeout(auth_timeout, async {
                deliver(&mut stream, resolve).await?;
                stream.shutdown().await.map_err(|_| failure())
            })
            .await
            .map_err(|_| credential_failure())?;
            drop(directory);
            result
        }
    }

    pub(super) fn reply(prompt: &str) -> Result<(), ()> {
        if std::env::var_os("SSH_ASKPASS_PROMPT").is_some() {
            return Err(());
        }
        let socket = std::env::var_os(SOCKET_ENV).ok_or(())?;
        let session = std::env::var(SESSION_ENV).map_err(|_| ())?;
        let mut stream = std::os::unix::net::UnixStream::connect(socket).map_err(|_| ())?;
        stream
            .set_read_timeout(Some(Duration::from_secs(300)))
            .map_err(|_| ())?;
        stream
            .set_write_timeout(Some(Duration::from_secs(300)))
            .map_err(|_| ())?;
        let parent = unsafe { libc::getppid() } as u32;
        stream
            .write_all(&encode_request(&session, prompt, parent)?)
            .map_err(|_| ())?;
        let mut header = vec![0; SSH_LOGIN.reply_header_len()];
        stream.read_exact(&mut header).map_err(|_| ())?;
        let length = SSH_LOGIN
            .secret_len(&header, crate::sudo::SUDO_PASSWORD_LIMIT)
            .map_err(|_| ())?;
        let mut bytes = Zeroizing::new(vec![0; length]);
        stream.read_exact(&mut bytes).map_err(|_| ())?;
        let mut trailing = [0; 1];
        if stream.read(&mut trailing).map_err(|_| ())? != 0 {
            return Err(());
        }
        print_secret(bytes)
    }
}

/// Internal companion entry. It never accepts a credential name or returns a
/// secret without the invocation's private broker channel.
pub fn helper_main() -> i32 {
    let mut arguments = std::env::args_os().skip(1);
    let Some(first) = arguments.next() else {
        return 1;
    };
    if arguments.next().is_some() {
        return 1;
    }
    if first == std::ffi::OsStr::new("--identity") {
        use std::io::Write;
        return if std::io::stdout()
            .write_all(identity_line().as_bytes())
            .is_ok()
        {
            0
        } else {
            1
        };
    }
    let Some(first) = first.to_str() else {
        return 1;
    };
    #[cfg(unix)]
    return if unix::reply(first).is_ok() { 0 } else { 1 };
    #[cfg(windows)]
    return if windows::reply(first).is_ok() { 0 } else { 1 };
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::credential::{KeychainFailure, KeychainOperation, KeychainReason};
    use tokio::io::AsyncReadExt;

    #[tokio::test]
    async fn login_failure_preserves_safe_diagnostics_and_delivers_no_bytes() {
        let expected = KeychainFailure::new(KeychainReason::Cancelled, KeychainOperation::Unlock);
        let (mut writer, mut reader) = tokio::io::duplex(128);
        let error = deliver(&mut writer, || async { Err(AppError::Keychain(expected)) })
            .await
            .unwrap_err();
        assert!(matches!(error, AppError::Keychain(actual) if actual == expected));
        drop(writer);
        let mut received = Vec::new();
        reader.read_to_end(&mut received).await.unwrap();
        assert!(received.is_empty());

        let mut sink = tokio::io::sink();
        let error = deliver(&mut sink, || async {
            Err(AppError::Credential(
                "synthetic-sensitive-provider-detail".into(),
            ))
        })
        .await
        .unwrap_err();
        assert!(!error
            .to_string()
            .contains("synthetic-sensitive-provider-detail"));
    }
}
