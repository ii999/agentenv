//! A single local SSH login challenge, independent of the sudo challenge.

const IDENTITY: &str = concat!("agentenv-ssh-askpass 1 ", env!("CARGO_PKG_VERSION"), "\n");

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
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{UnixListener, UnixStream};
    use tokio::process::Command;
    use zeroize::Zeroizing;

    use crate::credential::{CapturedSecret, Secret};
    use crate::error::AppError;

    const SOCKET_ENV: &str = "AGENTENV_SSH_SOCKET";
    const SESSION_ENV: &str = "AGENTENV_SSH_SESSION";
    const MAGIC: &[u8; 5] = b"AGEL\x01";
    const FIELD_LIMIT: usize = 4096;

    fn failure() -> AppError {
        AppError::SudoExecution("ssh-askpass-rejected: the SSH login challenge was rejected".into())
    }

    fn credential_failure() -> AppError {
        AppError::Credential(
            "SSH authentication credential unavailable; check its provider separately".into(),
        )
    }

    pub struct Session {
        directory: tempfile::TempDir,
        socket: PathBuf,
        marker: String,
        helper: PathBuf,
        listener: UnixListener,
        timing: tokio::sync::watch::Sender<
            Option<(tokio::time::Instant, Option<tokio::time::Instant>)>,
        >,
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
            let mut child = Command::new(helper)
                .arg("--identity")
                .env_clear()
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .kill_on_drop(true)
                .spawn()
                .map_err(|_| failure())?;
            let mut stdout = child
                .stdout
                .take()
                .ok_or_else(failure)?
                .take(IDENTITY.len() as u64 + 1);
            let operation = tokio::time::timeout(timeout, async {
                let mut bytes = Vec::new();
                stdout
                    .read_to_end(&mut bytes)
                    .await
                    .map_err(|_| failure())?;
                if bytes != IDENTITY.as_bytes() {
                    return Err(failure());
                }
                let status = child.wait().await.map_err(|_| failure())?;
                if !status.success() {
                    return Err(failure());
                }
                Ok(())
            })
            .await;
            if !matches!(operation, Ok(Ok(()))) {
                let _ = child.start_kill();
                let _ = child.wait().await;
                return Err(failure());
            }
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
            let mut random = [0_u8; 24];
            fs::File::open("/dev/urandom")
                .and_then(|mut file| file.read_exact(&mut random))
                .map_err(|_| failure())?;
            let marker = random.iter().map(|byte| format!("{byte:02x}")).collect();
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
        pub fn auth_timing(
            &self,
        ) -> tokio::sync::watch::Receiver<
            Option<(tokio::time::Instant, Option<tokio::time::Instant>)>,
        > {
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
                let received = field(&mut stream).await?;
                let prompt = field(&mut stream).await?;
                let parent = stream.read_u32().await.map_err(|_| failure())?;
                if received.as_slice() != marker.as_bytes()
                    || prompt.as_slice() != expected_prompt.as_bytes()
                    || parent != ssh_pid
                {
                    return Err(failure());
                }
                Ok(())
            })
            .await
            .map_err(|_| failure())??;
            // No second askpass process can connect, even while resolution runs.
            fs::remove_file(&socket).map_err(|_| failure())?;
            drop(listener);
            let started = tokio::time::Instant::now();
            timing.send_replace(Some((started, None)));
            let _active = ActiveAuthentication { timing, started };
            let result = tokio::time::timeout(auth_timeout, async {
                let secret = resolve().await.map_err(|_| credential_failure())?;
                secret
                    .validate_authentication(255)
                    .map_err(|_| credential_failure())?;
                stream.write_all(MAGIC).await.map_err(|_| failure())?;
                stream
                    .write_u16(secret.as_str().len() as u16)
                    .await
                    .map_err(|_| failure())?;
                stream
                    .write_all(secret.as_str().as_bytes())
                    .await
                    .map_err(|_| failure())?;
                stream.shutdown().await.map_err(|_| failure())?;
                Ok(())
            })
            .await
            .map_err(|_| credential_failure())?;
            drop(directory);
            result
        }
    }

    struct ActiveAuthentication {
        timing: tokio::sync::watch::Sender<
            Option<(tokio::time::Instant, Option<tokio::time::Instant>)>,
        >,
        started: tokio::time::Instant,
    }

    impl Drop for ActiveAuthentication {
        fn drop(&mut self) {
            self.timing
                .send_replace(Some((self.started, Some(tokio::time::Instant::now()))));
        }
    }

    async fn field(stream: &mut UnixStream) -> Result<Zeroizing<Vec<u8>>, AppError> {
        let mut magic = [0; 5];
        stream.read_exact(&mut magic).await.map_err(|_| failure())?;
        let len = stream.read_u16().await.map_err(|_| failure())? as usize;
        if &magic != MAGIC || len == 0 || len > FIELD_LIMIT {
            return Err(failure());
        }
        let mut bytes = Zeroizing::new(vec![0; len]);
        stream.read_exact(&mut bytes).await.map_err(|_| failure())?;
        Ok(bytes)
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
        for value in [session.as_bytes(), prompt.as_bytes()] {
            if value.is_empty() || value.len() > FIELD_LIMIT {
                return Err(());
            }
            stream.write_all(MAGIC).map_err(|_| ())?;
            stream
                .write_all(&(value.len() as u16).to_be_bytes())
                .map_err(|_| ())?;
            stream.write_all(value).map_err(|_| ())?;
        }
        stream
            .write_all(&(unsafe { libc::getppid() } as u32).to_be_bytes())
            .map_err(|_| ())?;
        let mut magic = [0; 5];
        let mut length = [0; 2];
        stream.read_exact(&mut magic).map_err(|_| ())?;
        stream.read_exact(&mut length).map_err(|_| ())?;
        let length = u16::from_be_bytes(length) as usize;
        if &magic != MAGIC || !(1..=255).contains(&length) {
            return Err(());
        }
        let mut bytes = Zeroizing::new(vec![0; length]);
        stream.read_exact(&mut bytes).map_err(|_| ())?;
        let mut trailing = [0; 1];
        if stream.read(&mut trailing).map_err(|_| ())? != 0 {
            return Err(());
        }
        let secret = CapturedSecret::new(bytes.to_vec())
            .into_secret()
            .map_err(|_| ())?;
        secret.validate_authentication(255).map_err(|_| ())?;
        let mut output = std::io::stdout().lock();
        output
            .write_all(secret.as_str().as_bytes())
            .map_err(|_| ())?;
        output.write_all(b"\n").map_err(|_| ())?;
        output.flush().map_err(|_| ())
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
        return if std::io::stdout().write_all(IDENTITY.as_bytes()).is_ok() {
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
    #[cfg(not(any(unix, windows)))]
    9
}
