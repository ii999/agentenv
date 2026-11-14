//! Secret-safe calls to the verified, persistent native credential helper.

use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use crate::credential::{KeychainFailure, KeychainOperation, KeychainReason, Secret};

use super::installation;
use super::native::{Diagnostic, UnlockReport};
use super::protocol::{self, Reply, ReplyFrame, Request};

struct Helper(Child);

impl Drop for Helper {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn exchange(request: Request, secret: Option<&Secret>) -> Result<ReplyFrame, KeychainFailure> {
    let operation = match request {
        Request::Read { .. } => KeychainOperation::Read,
        Request::Store { .. } => KeychainOperation::Store,
        Request::Status => KeychainOperation::Status,
        Request::Unlock => KeychainOperation::Unlock,
        #[cfg(all(feature = "test-support", debug_assertions))]
        Request::DeleteFixture { .. } => KeychainOperation::Store,
    };
    let installed = installation::installed_helper().map_err(|_| {
        let exists = installation::state_directory()
            .map(|directory| installation::has_installation(&directory))
            .unwrap_or(false);
        KeychainFailure::new(
            if exists {
                KeychainReason::HelperInvalid
            } else {
                KeychainReason::HelperMissing
            },
            KeychainOperation::Connect,
        )
    })?;
    let failed = || KeychainFailure::new(KeychainReason::Transport, operation);
    let (mut parent, child) = UnixStream::pair().map_err(|_| failed())?;
    parent
        .set_read_timeout(Some(Duration::from_secs(300)))
        .map_err(|_| failed())?;
    parent
        .set_write_timeout(Some(Duration::from_secs(300)))
        .map_err(|_| failed())?;
    let descriptor = child.as_raw_fd();
    let mut command = Command::new(&installed.path);
    command
        .arg("--serve")
        .env_clear()
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    for name in [
        "HOME",
        "USER",
        "LOGNAME",
        "TMPDIR",
        "LANG",
        "LC_ALL",
        "__CF_USER_TEXT_ENCODING",
    ] {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
    #[cfg(all(feature = "test-support", debug_assertions))]
    for name in [
        "AGENTENV_TEST_NATIVE_KEYCHAIN",
        "AGENTENV_TEST_KEYCHAIN_NO_UI",
        "AGENTENV_TEST_KEYCHAIN_DROP_STORE_REPLY",
    ] {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
    unsafe {
        command.pre_exec(move || {
            if descriptor != 3 && libc::dup2(descriptor, 3) == -1 {
                return Err(std::io::Error::last_os_error());
            }
            if libc::fcntl(3, libc::F_SETFD, 0) == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut helper = Helper(command.spawn().map_err(|_| failed())?);
    drop(child);
    let incomplete = || {
        KeychainFailure::new(
            if matches!(request, Request::Store { .. }) {
                KeychainReason::StoreUnconfirmed
            } else {
                KeychainReason::Transport
            },
            operation,
        )
    };
    protocol::write_request(&mut parent, &request, secret).map_err(|_| incomplete())?;
    let response = protocol::read_reply(&mut parent).map_err(|_| incomplete())?;
    let status = helper.0.wait().map_err(|_| incomplete())?;
    if !status.success() {
        return Err(incomplete());
    }
    if let Reply::Failed { error } = response.metadata {
        return Err(error);
    }
    Ok(response)
}

pub(crate) fn read(service: &str, account: &str) -> Result<Secret, KeychainFailure> {
    let response = exchange(
        Request::Read {
            service: service.into(),
            account: account.into(),
        },
        None,
    )?;
    match (response.metadata, response.secret) {
        (Reply::Read, Some(secret)) => Ok(secret),
        _ => Err(KeychainFailure::new(
            KeychainReason::Transport,
            KeychainOperation::Read,
        )),
    }
}

pub(crate) fn store(service: &str, account: &str, secret: &Secret) -> Result<(), KeychainFailure> {
    let response = exchange(
        Request::Store {
            service: service.into(),
            account: account.into(),
        },
        Some(secret),
    )?;
    match response.metadata {
        Reply::Stored => Ok(()),
        _ => Err(KeychainFailure::new(
            KeychainReason::StoreUnconfirmed,
            KeychainOperation::Store,
        )),
    }
}

pub fn status() -> Result<Diagnostic, KeychainFailure> {
    let response = exchange(Request::Status, None)?;
    match response.metadata {
        Reply::Inspected { state } => Ok(state),
        _ => Err(KeychainFailure::new(
            KeychainReason::Transport,
            KeychainOperation::Status,
        )),
    }
}

pub fn unlock() -> Result<UnlockReport, KeychainFailure> {
    let response = exchange(Request::Unlock, None)?;
    match response.metadata {
        Reply::Unlocked { report } => Ok(report),
        _ => Err(KeychainFailure::new(
            KeychainReason::Transport,
            KeychainOperation::Unlock,
        )),
    }
}

#[cfg(all(feature = "test-support", debug_assertions))]
pub fn delete_fixture(service: &str, account: &str) -> Result<(), KeychainFailure> {
    let response = exchange(
        Request::DeleteFixture {
            service: service.into(),
            account: account.into(),
        },
        None,
    )?;
    match response.metadata {
        Reply::FixtureDeleted => Ok(()),
        _ => Err(KeychainFailure::new(
            KeychainReason::Transport,
            KeychainOperation::Store,
        )),
    }
}
