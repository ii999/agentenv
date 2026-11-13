//! On-demand native credential helper entry point.

use std::io;
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;
use std::time::Duration;

use crate::credential::channel::inherited_socket;
use crate::credential::{KeychainFailure, KeychainOperation, KeychainReason};

use super::native;
use super::protocol::{self, Reply, ReplyFrame, Request, RequestFrame};

pub fn main() -> i32 {
    let arguments: Vec<_> = std::env::args_os().skip(1).collect();
    if arguments == ["--identity"] {
        println!("{}", protocol::IDENTITY);
        return 0;
    }
    if arguments != ["--serve"] {
        eprintln!(
            "This helper is started by agentenv; run 'agentenv keychain status' for diagnostics."
        );
        return 1;
    }
    match serve() {
        Ok(()) => 0,
        Err(_) => {
            eprintln!("The credential helper channel could not complete.");
            4
        }
    }
}

fn serve() -> io::Result<()> {
    let mut channel = inherited_socket(3)?;
    channel.set_read_timeout(Some(Duration::from_secs(300)))?;
    channel.set_write_timeout(Some(Duration::from_secs(300)))?;
    let request = protocol::read_request(&mut channel)?;
    watch_parent(&channel)?;
    let frame = match operate(request) {
        Ok(frame) => frame,
        Err(error) => ReplyFrame {
            metadata: Reply::Failed { error },
            secret: None,
        },
    };
    #[cfg(all(feature = "test-support", debug_assertions))]
    if matches!(frame.metadata, Reply::Stored)
        && std::env::var_os("AGENTENV_TEST_KEYCHAIN_DROP_STORE_REPLY").is_some()
    {
        return Err(io::Error::new(
            io::ErrorKind::ConnectionReset,
            "synthetic lost store acknowledgement",
        ));
    }
    protocol::write_reply(&mut channel, &frame)
}

/// A cancelled parent closes its socket even while Security.framework waits
/// in a system dialog. Exit the helper instead of leaving an orphaned prompt.
fn watch_parent(channel: &UnixStream) -> io::Result<()> {
    let monitor = channel.try_clone()?;
    std::thread::Builder::new()
        .name("agentenv-keychain-parent".into())
        .spawn(move || {
            let mut descriptor = libc::pollfd {
                fd: monitor.as_raw_fd(),
                events: libc::POLLHUP,
                revents: 0,
            };
            loop {
                let status = unsafe { libc::poll(&mut descriptor, 1, -1) };
                if status < 0 && io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                if status < 0
                    || descriptor.revents & (libc::POLLHUP | libc::POLLERR | libc::POLLNVAL) != 0
                {
                    // This process owns one operation and no shared service state.
                    std::process::exit(4);
                }
            }
        })?;
    Ok(())
}

fn operate(request: RequestFrame) -> Result<ReplyFrame, KeychainFailure> {
    let mut keychain = native::open_keychain()?;
    let mut secret = None;
    let metadata = match request.metadata {
        Request::Read { service, account } => {
            secret = Some(native::read(&mut keychain, &service, &account)?);
            Reply::Read
        }
        Request::Store { service, account } => {
            let value = request.secret.ok_or_else(|| {
                KeychainFailure::new(KeychainReason::InvalidValue, KeychainOperation::Store)
            })?;
            native::store(&mut keychain, &service, &account, &value)?;
            Reply::Stored
        }
        Request::Status => Reply::Inspected {
            state: native::inspect(&keychain)?,
        },
        Request::Unlock => Reply::Unlocked {
            report: native::unlock(&mut keychain)?,
        },
        #[cfg(all(feature = "test-support", debug_assertions))]
        Request::DeleteFixture { service, account } => {
            native::delete_fixture(&mut keychain, &service, &account)?;
            Reply::FixtureDeleted
        }
    };
    Ok(ReplyFrame { metadata, secret })
}
