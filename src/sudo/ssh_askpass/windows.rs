//! Windows OpenSSH askpass broker. Only the installed companion process,
//! launched directly by this invocation's SSH process, can claim the one
//! challenge. Password bytes never enter argv, environment or disk.

use std::future::Future;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::windows::named_pipe::NamedPipeServer;
use tokio::process::Command;
use tokio::sync::watch;
use tokio::time::timeout;
use windows_sys::Win32::System::Threading::CREATE_NO_WINDOW;
use zeroize::Zeroizing;

use super::{
    credential_failure, deliver, encode_request, failure, print_secret, read_request,
    verify_companion, ActiveAuthentication, Timing, SSH_LOGIN,
};
use crate::credential::Secret;
use crate::error::AppError;
use crate::windows;

const PIPE_ENV: &str = "AGENTENV_SSH_PIPE";
const PARENT_ENV: &str = "AGENTENV_SSH_PARENT";
const SESSION_ENV: &str = "AGENTENV_SSH_SESSION";

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
        verify_companion(command, budget).await?;
        let (name, pipe) = windows::private_pipe("ssh").map_err(|_| failure())?;
        Ok(Self {
            name,
            pipe,
            marker: crate::random::nonce().map_err(|_| failure())?,
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
            read_request(&mut self.pipe, &self.marker, expected_prompt, ssh_pid).await?;
            Ok::<_, AppError>(peer)
        })
        .await
        .map_err(|_| failure())??;
        // This single connected instance is never disconnected/recreated for
        // another client, even while the credential provider is blocked.
        let _active = ActiveAuthentication::start(&self.timing);
        let pipe = &mut self.pipe;
        let result = timeout(auth_timeout, async {
            deliver(pipe, resolve).await?;
            if pipe.read_u8().await.map_err(|_| failure())? != ACKNOWLEDGED {
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

/// The companion's acknowledgement that it received the whole reply. A pipe
/// server cannot observe the client's read, so the broker waits for it.
const ACKNOWLEDGED: u8 = 0x06;

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
    let bytes = runtime.block_on(async {
        timeout(Duration::from_secs(300), async {
            let mut pipe = windows::connect(&name, server).map_err(|_| ())?;
            pipe.write_all(&encode_request(&marker, prompt, parent)?)
                .await
                .map_err(|_| ())?;
            let mut header = vec![0; SSH_LOGIN.reply_header_len()];
            pipe.read_exact(&mut header).await.map_err(|_| ())?;
            let length = SSH_LOGIN
                .secret_len(&header, crate::sudo::SUDO_PASSWORD_LIMIT)
                .map_err(|_| ())?;
            let mut bytes = Zeroizing::new(vec![0; length]);
            pipe.read_exact(&mut bytes).await.map_err(|_| ())?;
            pipe.write_u8(ACKNOWLEDGED).await.map_err(|_| ())?;
            let mut trailing = [0; 1];
            if pipe.read(&mut trailing).await.map_err(|_| ())? != 0 {
                return Err(());
            }
            Ok(bytes)
        })
        .await
        .map_err(|_| ())?
    })?;
    print_secret(bytes)
}
