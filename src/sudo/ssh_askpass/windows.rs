//! Windows OpenSSH askpass broker. Only the installed companion process,
//! launched directly by this invocation's SSH process, can claim the one
//! challenge. Password bytes never enter argv, environment or disk.

use std::future::Future;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::windows::named_pipe::NamedPipeServer;
use tokio::process::Command;
use tokio::sync::watch;
use tokio::time::{timeout, Instant};
use windows_sys::Win32::System::Threading::CREATE_NO_WINDOW;
use zeroize::Zeroizing;

use super::IDENTITY;
use crate::credential::{CapturedSecret, Secret};
use crate::error::AppError;
use crate::windows;

const PIPE_ENV: &str = "AGENTENV_SSH_PIPE";
const PARENT_ENV: &str = "AGENTENV_SSH_PARENT";
const SESSION_ENV: &str = "AGENTENV_SSH_SESSION";
const MAGIC: &[u8; 5] = b"AGEL\x01";
const FIELD_LIMIT: usize = 4096;
type Timing = Option<(Instant, Option<Instant>)>;

fn failure() -> AppError {
    AppError::SudoExecution("ssh-askpass-rejected: the SSH login challenge was rejected".into())
}
fn credential_failure() -> AppError {
    AppError::Credential(
        "SSH authentication credential unavailable; check its provider separately".into(),
    )
}

pub struct Session {
    name: String,
    pipe: NamedPipeServer,
    marker: String,
    helper: PathBuf,
    timing: watch::Sender<Timing>,
}

impl Session {
    pub async fn create(helper: &Path, budget: Duration) -> Result<Self, AppError> {
        let metadata = std::fs::symlink_metadata(helper).map_err(|_| failure())?;
        if !helper.is_absolute() || !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err(failure());
        }
        let mut command = Command::new(helper);
        command
            .arg("--identity")
            .env_clear()
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .creation_flags(CREATE_NO_WINDOW)
            .kill_on_drop(true);
        if let Some(root) = std::env::var_os("SystemRoot") {
            command.env("SystemRoot", root);
        }
        let mut child = command.spawn().map_err(|_| failure())?;
        let mut stdout = child
            .stdout
            .take()
            .ok_or_else(failure)?
            .take(IDENTITY.len() as u64 + 1);
        let result = timeout(budget, async {
            let mut bytes = Vec::new();
            stdout
                .read_to_end(&mut bytes)
                .await
                .map_err(|_| failure())?;
            if bytes != IDENTITY.as_bytes() || !child.wait().await.map_err(|_| failure())?.success()
            {
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
        let (name, pipe) = windows::private_pipe("ssh").map_err(|_| failure())?;
        Ok(Self {
            name,
            pipe,
            marker: windows::nonce().map_err(|_| failure())?,
            helper: helper.to_owned(),
            timing: watch::channel(None).0,
        })
    }

    pub fn auth_timing(&self) -> watch::Receiver<Timing> {
        self.timing.subscribe()
    }

    pub fn configure(&self, command: &mut Command) {
        command
            .env("SSH_ASKPASS", &self.helper)
            .env("SSH_ASKPASS_REQUIRE", "force")
            .env("DISPLAY", "agentenv:0")
            .env_remove("SSH_ASKPASS_PROMPT")
            .env(PIPE_ENV, &self.name)
            .env(PARENT_ENV, std::process::id().to_string())
            .env(SESSION_ENV, &self.marker);
    }

    pub async fn serve<F, Fut>(
        mut self,
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
        let peer = timeout(setup_timeout, async {
            self.pipe.connect().await.map_err(|_| failure())?;
            let pid = windows::client_pid(&self.pipe).map_err(|_| failure())?;
            let peer =
                windows::verify_process(pid, ssh_pid, &self.helper).map_err(|_| failure())?;
            let marker = field(&mut self.pipe).await?;
            let prompt = field(&mut self.pipe).await?;
            let parent = self.pipe.read_u32().await.map_err(|_| failure())?;
            if marker.as_slice() != self.marker.as_bytes()
                || prompt.as_slice() != expected_prompt.as_bytes()
                || parent != ssh_pid
            {
                return Err(failure());
            }
            Ok(peer)
        })
        .await
        .map_err(|_| failure())??;
        // This single connected instance is never disconnected/recreated for
        // another client, even while the credential provider is blocked.
        let started = Instant::now();
        self.timing.send_replace(Some((started, None)));
        let _active = ActiveAuthentication {
            timing: self.timing.clone(),
            started,
        };
        let result = timeout(auth_timeout, async {
            let secret = resolve().await.map_err(|_| credential_failure())?;
            secret
                .validate_authentication(255)
                .map_err(|_| credential_failure())?;
            self.pipe.write_all(MAGIC).await.map_err(|_| failure())?;
            self.pipe
                .write_u16(secret.as_str().len() as u16)
                .await
                .map_err(|_| failure())?;
            self.pipe
                .write_all(secret.as_str().as_bytes())
                .await
                .map_err(|_| failure())?;
            if self.pipe.read_u8().await.map_err(|_| failure())? != 0x06 {
                return Err(failure());
            }
            Ok(())
        })
        .await
        .map_err(|_| credential_failure())?;
        drop(peer);
        result
    }
}

struct ActiveAuthentication {
    timing: watch::Sender<Timing>,
    started: Instant,
}
impl Drop for ActiveAuthentication {
    fn drop(&mut self) {
        self.timing
            .send_replace(Some((self.started, Some(Instant::now()))));
    }
}

async fn field(pipe: &mut NamedPipeServer) -> Result<Zeroizing<Vec<u8>>, AppError> {
    let mut magic = [0; 5];
    pipe.read_exact(&mut magic).await.map_err(|_| failure())?;
    let length = pipe.read_u16().await.map_err(|_| failure())? as usize;
    if &magic != MAGIC || !(1..=FIELD_LIMIT).contains(&length) {
        return Err(failure());
    }
    let mut bytes = Zeroizing::new(vec![0; length]);
    pipe.read_exact(&mut bytes).await.map_err(|_| failure())?;
    Ok(bytes)
}

pub(super) fn reply(prompt: &str) -> Result<(), ()> {
    if std::env::var_os("SSH_ASKPASS_PROMPT").is_some() {
        return Err(());
    }
    let name = std::env::var(PIPE_ENV).map_err(|_| ())?;
    let server = std::env::var(PARENT_ENV)
        .map_err(|_| ())?
        .parse()
        .map_err(|_| ())?;
    let marker = std::env::var(SESSION_ENV).map_err(|_| ())?;
    let parent = windows::parent_pid(std::process::id()).map_err(|_| ())?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|_| ())?;
    let secret = runtime.block_on(async {
        timeout(Duration::from_secs(300), async {
            let mut pipe = windows::connect(&name, server).map_err(|_| ())?;
            for value in [marker.as_bytes(), prompt.as_bytes()] {
                if value.is_empty() || value.len() > FIELD_LIMIT {
                    return Err(());
                }
                pipe.write_all(MAGIC).await.map_err(|_| ())?;
                pipe.write_u16(value.len() as u16).await.map_err(|_| ())?;
                pipe.write_all(value).await.map_err(|_| ())?;
            }
            pipe.write_u32(parent).await.map_err(|_| ())?;
            let mut magic = [0; 5];
            pipe.read_exact(&mut magic).await.map_err(|_| ())?;
            let length = pipe.read_u16().await.map_err(|_| ())? as usize;
            if &magic != MAGIC || !(1..=255).contains(&length) {
                return Err(());
            }
            let mut bytes = Zeroizing::new(vec![0; length]);
            pipe.read_exact(&mut bytes).await.map_err(|_| ())?;
            pipe.write_u8(0x06).await.map_err(|_| ())?;
            let mut trailing = [0; 1];
            if pipe.read(&mut trailing).await.map_err(|_| ())? != 0 {
                return Err(());
            }
            let secret = CapturedSecret::new(std::mem::take(&mut *bytes))
                .into_secret()
                .map_err(|_| ())?;
            secret.validate_authentication(255).map_err(|_| ())?;
            Ok(secret)
        })
        .await
        .map_err(|_| ())?
    })?;
    let mut output = std::io::stdout().lock();
    output
        .write_all(secret.as_str().as_bytes())
        .map_err(|_| ())?;
    output.write_all(b"\n").map_err(|_| ())?;
    output.flush().map_err(|_| ())
}
