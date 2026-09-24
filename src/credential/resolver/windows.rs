//! The existing resolver protocol on a private Windows named pipe. The child
//! receives only routing metadata in its environment, never a credential.

use std::os::windows::process::CommandExt;
use std::process::{Child, Command, Stdio};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use windows_sys::Win32::System::Threading::CREATE_NO_WINDOW;

use super::*;
use crate::windows::{self, Job};

const PIPE_ENV: &str = "AGENTENV_RESOLVER_PIPE";
const PARENT_ENV: &str = "AGENTENV_RESOLVER_PARENT";

struct Resolver {
    child: Child,
    job: Option<Job>,
}
impl Drop for Resolver {
    fn drop(&mut self) {
        // Close the job before waiting, including when provider descendants
        // retain handles or the credential store is waiting for a dialog.
        drop(self.job.take());
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

pub(super) async fn resolve(
    executable: &Path,
    definition: &CredentialDef,
    stage: ResolutionStage,
    limit: usize,
) -> Result<Secret, ResolveError> {
    let request = encode_request(definition, stage, limit)?;
    let (name, mut pipe) = windows::private_pipe("resolver").map_err(|_| ResolveError::Provider)?;
    let job = Job::new().map_err(|_| ResolveError::Provider)?;
    let mut command = Command::new(executable);
    command
        .arg("--agentenv-resolver")
        .env(PIPE_ENV, &name)
        .env(PARENT_ENV, std::process::id().to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW);
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
    let mut resolver = Resolver {
        child: command.spawn().map_err(|_| ResolveError::Provider)?,
        job: None,
    };
    job.assign(&resolver.child)
        .map_err(|_| ResolveError::Provider)?;
    resolver.job = Some(job);
    // A pipe server sees no EOF from a client that never arrives, so a
    // resolver that exits before connecting is watched explicitly; otherwise
    // an immediate startup failure would surface as a full-deadline timeout.
    {
        let connect = pipe.connect();
        tokio::pin!(connect);
        loop {
            tokio::select! {
                biased;
                connected = &mut connect => {
                    connected.map_err(|_| ResolveError::Provider)?;
                    break;
                }
                _ = tokio::time::sleep(Duration::from_millis(20)) => {
                    if resolver
                        .child
                        .try_wait()
                        .map_err(|_| ResolveError::Provider)?
                        .is_some()
                    {
                        return Err(ResolveError::Provider);
                    }
                }
            }
        }
    }
    if windows::client_pid(&pipe).map_err(|_| ResolveError::Provider)? != resolver.child.id() {
        return Err(ResolveError::Provider);
    }
    let result = exchange(&mut pipe, &request, stage, limit).await;
    // Keep the server alive until its response is consumed. CloseHandle on
    // an unread Windows pipe is not an EOF/flush protocol.
    let _ = pipe.write_u8(0x06).await;
    result
}

pub(super) fn serve() -> Result<(), AppError> {
    let pipe = std::env::var(PIPE_ENV).map_err(|_| failure())?;
    let parent: u32 = std::env::var(PARENT_ENV)
        .map_err(|_| failure())?
        .parse()
        .map_err(|_| failure())?;
    if windows::parent_pid(std::process::id()).map_err(|_| failure())? != parent {
        return Err(failure());
    }
    // This internal entry runs before any runtime or provider thread exists.
    std::env::remove_var(PIPE_ENV);
    std::env::remove_var(PARENT_ENV);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|_| failure())?;
    runtime.block_on(async {
        let mut pipe = windows::connect(&pipe, parent).map_err(|_| failure())?;
        tokio::time::timeout(Duration::from_secs(300), async {
            let mut magic = [0; 5];
            pipe.read_exact(&mut magic).await.map_err(|_| failure())?;
            let size = pipe.read_u32().await.map_err(|_| failure())? as usize;
            if &magic != MAGIC || size == 0 || size > MAX_REQUEST {
                return Err(failure());
            }
            let mut metadata = vec![0; size];
            pipe.read_exact(&mut metadata)
                .await
                .map_err(|_| failure())?;
            // Potentially blocking provider work runs in this disposable child;
            // cancellation is enforced by the parent's job and pipe lifetime.
            let result = resolve_request(&metadata);
            pipe.write_all(MAGIC).await.map_err(|_| failure())?;
            let result = match result {
                Ok(secret) => {
                    pipe.write_u8(STATUS_OK).await.map_err(|_| failure())?;
                    pipe.write_u16(secret.as_str().len() as u16)
                        .await
                        .map_err(|_| failure())?;
                    pipe.write_all(secret.as_str().as_bytes())
                        .await
                        .map_err(|_| failure())?;
                    Ok(())
                }
                Err(status) => {
                    pipe.write_u8(status).await.map_err(|_| failure())?;
                    Err(failure())
                }
            };
            if pipe.read_u8().await.map_err(|_| failure())? != 0x06 {
                return Err(failure());
            }
            result
        })
        .await
        .map_err(|_| failure())?
    })
}
