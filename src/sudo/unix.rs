use std::fs::{self, File};
use std::future::Future;
use std::io::Read;
use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::Path;
use std::process::{ExitStatus, Stdio};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{UnixListener, UnixStream};
use tokio::process::{Child, Command};
use zeroize::Zeroizing;

use super::{
    Cancellation, ExecutionOutcome, ExecutionRequest, LocalOptions, ProcessIo, PROTOCOL_VERSION,
    SUDO_PASSWORD_LIMIT,
};
use crate::credential::Secret;
use crate::error::AppError;

const MAGIC: &[u8; 5] = b"AGES\x01";
const MAX_FIELD: usize = 4096;
const CLEANUP_TIMEOUT: Duration = Duration::from_secs(5);
const ROUTE_SOCKET: &str = "AGENTENV_SUDO_SOCKET";
const ROUTE_SESSION: &str = "AGENTENV_SUDO_SESSION";

pub(super) async fn check(options: &LocalOptions) -> Result<(), AppError> {
    validate_account(&options.auth_user)?;
    validate_executable(&options.sudo_path, "sudo-unavailable")?;
    validate_executable(&options.helper_path, "helper-missing")?;
    check_helper_identity(&options.helper_path, options.setup_timeout, None).await?;
    let runtime = Runtime::create()?;
    runtime.validate()?;
    Ok(())
}

pub(super) async fn execute<F, Fut>(
    request: ExecutionRequest,
    options: LocalOptions,
    io: ProcessIo,
    mut cancellation: Cancellation,
    resolve_password: F,
) -> Result<ExecutionOutcome, AppError>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<Secret, AppError>>,
{
    validate_account(&options.auth_user)?;
    validate_executable(&options.sudo_path, "sudo-unavailable")?;
    validate_executable(&options.helper_path, "helper-missing")?;
    if let Some(cwd) = request.cwd() {
        let metadata = fs::metadata(cwd)
            .map_err(|_| owned("invalid-cwd", "working directory is unavailable"))?;
        if !metadata.is_dir() {
            return Err(owned("invalid-cwd", "working directory is not a directory"));
        }
    }
    if let Some(signal) = pending_signal(&cancellation) {
        return Ok(cancelled(signal));
    }
    if let Some(signal) = check_helper_identity(
        &options.helper_path,
        options.setup_timeout,
        Some(&mut cancellation),
    )
    .await?
    {
        return Ok(cancelled(signal));
    }

    let runtime = Runtime::create()?;
    let session = random_session()?;
    let prompt_template = format!("agentenv sudo [{session}] password for %p:");
    let expected_prompt = format!(
        "agentenv sudo [{session}] password for {}:",
        options.auth_user
    );

    let mut command = Command::new(&options.sudo_path);
    command
        .arg("-A")
        .arg("-k")
        .arg("-u")
        .arg(&options.run_as)
        .arg("-p")
        .arg(&prompt_template)
        .arg("--")
        .arg(request.executable())
        .args(request.arguments())
        .stdin(io.stdin)
        .stdout(io.stdout)
        .stderr(io.stderr)
        .env_clear();
    apply_curated_environment(&mut command);
    command
        .env("SUDO_ASKPASS", &options.helper_path)
        .env(ROUTE_SOCKET, &runtime.socket)
        .env(ROUTE_SESSION, &session);
    if let Some(cwd) = request.cwd() {
        command.current_dir(cwd);
    }
    command.as_std_mut().process_group(0);
    if let Some(signal) = pending_signal(&cancellation) {
        return Ok(cancelled(signal));
    }
    let mut child = command
        .spawn()
        .map_err(|_| owned("spawn-failed", "could not start sudo"))?;
    if let Some(signal) = pending_signal(&cancellation) {
        let status = terminate(&mut child, signal).await?;
        return Ok(observed(status, false));
    }

    let Runtime {
        _directory,
        socket,
        listener,
    } = runtime;
    let mut broker = Box::pin(serve_once(
        listener,
        &socket,
        &session,
        &expected_prompt,
        options.setup_timeout,
        options.auth_timeout,
        resolve_password,
    ));
    let mut password_delivered = false;
    let mut cancellation_open = true;
    loop {
        tokio::select! {
            biased;
            changed = cancellation.changed(), if cancellation_open => {
                if changed.is_err() {
                    cancellation_open = false;
                } else if let Some(signal) = pending_signal(&cancellation) {
                    let _ = fs::remove_file(&socket);
                    drop(broker);
                    let status = terminate(&mut child, signal).await?;
                    return Ok(observed(status, password_delivered));
                }
            }
            broker_result = &mut broker, if !password_delivered => {
                let _ = fs::remove_file(&socket);
                match broker_result {
                    Ok(()) => password_delivered = true,
                    Err(error) => {
                        terminate(&mut child, libc::SIGTERM).await?;
                        return Err(error);
                    }
                }
            }
            status = child.wait() => {
                let status = status.map_err(|_| uncertain("could not observe sudo completion"))?;
                return Ok(observed(status, password_delivered));
            }
        }
    }
}

fn pending_signal(cancellation: &Cancellation) -> Option<i32> {
    let signal = *cancellation.borrow();
    (signal > 0).then_some(signal)
}

fn observed(status: ExitStatus, password_delivered: bool) -> ExecutionOutcome {
    ExecutionOutcome {
        exit_code: status.code(),
        signal: status.signal(),
        password_delivered,
    }
}

fn cancelled(signal: i32) -> ExecutionOutcome {
    ExecutionOutcome {
        exit_code: None,
        signal: Some(signal),
        password_delivered: false,
    }
}

/// Forwards `signal` to the sudo process group and returns sudo's observed
/// status. If sudo does not exit, it is killed and reaped, but the privileged
/// target may survive, so the outcome is reported as unconfirmed.
async fn terminate(child: &mut Child, signal: i32) -> Result<ExitStatus, AppError> {
    let pid = child
        .id()
        .ok_or_else(|| uncertain("sudo process identifier is unavailable"))? as i32;
    let sent = unsafe { libc::kill(-pid, signal) };
    if sent != 0 {
        return match tokio::time::timeout(CLEANUP_TIMEOUT, child.wait()).await {
            Ok(Ok(status)) => Ok(status),
            _ => Err(uncertain("could not signal or reap the sudo process group")),
        };
    }
    match tokio::time::timeout(CLEANUP_TIMEOUT, child.wait()).await {
        Ok(Ok(status)) => Ok(status),
        Ok(Err(_)) => Err(uncertain("could not reap sudo during cleanup")),
        Err(_) => {
            let _ = unsafe { libc::kill(-pid, libc::SIGKILL) };
            let _ = tokio::time::timeout(CLEANUP_TIMEOUT, child.wait()).await;
            Err(uncertain(
                "sudo did not exit after the forwarded signal; target termination is unconfirmed",
            ))
        }
    }
}

async fn serve_once<F, Fut>(
    listener: UnixListener,
    socket: &Path,
    session: &str,
    expected_prompt: &str,
    setup_timeout: Duration,
    auth_timeout: Duration,
    resolve_password: F,
) -> Result<(), AppError>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<Secret, AppError>>,
{
    let (mut stream, _) = listener
        .accept()
        .await
        .map_err(|_| owned("broker-failed", "could not accept the sudo helper"))?;
    let mut stream = tokio::time::timeout(setup_timeout, async {
        validate_peer(&stream)?;
        let received_session = read_field(&mut stream).await?;
        let prompt = read_field(&mut stream).await?;
        if received_session.as_slice() != session.as_bytes()
            || prompt.as_slice() != expected_prompt.as_bytes()
        {
            let _ = write_failure(&mut stream).await;
            return Err(owned(
                "authentication-rejected",
                "sudo requested an unexpected account or prompt",
            ));
        }
        Ok(stream)
    })
    .await
    .map_err(|_| owned("helper-timeout", "the sudo helper request timed out"))??;
    fs::remove_file(socket)
        .map_err(|_| owned("broker-failed", "could not disarm the sudo helper socket"))?;

    tokio::time::timeout(auth_timeout, async {
        let secret = match resolve_password().await {
            Ok(secret) => secret,
            Err(_) => {
                let _ = write_failure(&mut stream).await;
                return Err(credential_failure());
            }
        };
        if secret.validate_authentication(SUDO_PASSWORD_LIMIT).is_err() {
            let _ = write_failure(&mut stream).await;
            return Err(credential_failure());
        }
        stream
            .write_all(MAGIC)
            .await
            .map_err(|_| owned("broker-failed", "could not reply to the sudo helper"))?;
        stream
            .write_u8(0)
            .await
            .map_err(|_| owned("broker-failed", "could not reply to the sudo helper"))?;
        stream
            .write_u16(secret.as_str().len() as u16)
            .await
            .map_err(|_| owned("broker-failed", "could not reply to the sudo helper"))?;
        stream
            .write_all(secret.as_str().as_bytes())
            .await
            .map_err(|_| owned("broker-failed", "could not reply to the sudo helper"))?;
        stream
            .shutdown()
            .await
            .map_err(|_| owned("broker-failed", "could not close the sudo helper response"))?;
        Ok(())
    })
    .await
    .map_err(|_| credential_failure())?
}

async fn read_field(stream: &mut UnixStream) -> Result<Zeroizing<Vec<u8>>, AppError> {
    let mut magic = [0; 5];
    stream
        .read_exact(&mut magic)
        .await
        .map_err(|_| owned("broker-invalid", "the sudo helper request was incomplete"))?;
    if &magic != MAGIC {
        return Err(owned(
            "broker-invalid",
            "the sudo helper request was invalid",
        ));
    }
    let length = stream
        .read_u16()
        .await
        .map_err(|_| owned("broker-invalid", "the sudo helper request was incomplete"))?
        as usize;
    if length == 0 || length > MAX_FIELD {
        return Err(owned(
            "broker-invalid",
            "the sudo helper request was invalid",
        ));
    }
    let mut field = Zeroizing::new(vec![0; length]);
    stream
        .read_exact(&mut field)
        .await
        .map_err(|_| owned("broker-invalid", "the sudo helper request was incomplete"))?;
    Ok(field)
}

async fn write_failure(stream: &mut UnixStream) -> std::io::Result<()> {
    stream.write_all(MAGIC).await?;
    stream.write_u8(1).await?;
    stream.write_u16(0).await?;
    stream.shutdown().await
}

fn validate_peer(stream: &UnixStream) -> Result<(), AppError> {
    let fd = std::os::fd::AsRawFd::as_raw_fd(stream);
    #[cfg(target_os = "linux")]
    unsafe {
        let mut credentials = std::mem::MaybeUninit::<libc::ucred>::uninit();
        let mut length = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
        if libc::getsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            credentials.as_mut_ptr().cast(),
            &mut length,
        ) != 0
            || credentials.assume_init().uid != libc::getuid()
        {
            return Err(owned(
                "peer-rejected",
                "the sudo helper peer is not the invoking user",
            ));
        }
    }
    #[cfg(not(target_os = "linux"))]
    unsafe {
        let (mut uid, mut gid) = (0, 0);
        if libc::getpeereid(fd, &mut uid, &mut gid) != 0 || uid != libc::getuid() {
            return Err(owned(
                "peer-rejected",
                "the sudo helper peer is not the invoking user",
            ));
        }
    }
    Ok(())
}

async fn check_helper_identity(
    path: &Path,
    timeout: Duration,
    mut cancellation: Option<&mut Cancellation>,
) -> Result<Option<i32>, AppError> {
    let expected = format!(
        "agentenv-sudo-helper {PROTOCOL_VERSION} {}\n",
        env!("CARGO_PKG_VERSION")
    );
    let mut command = Command::new(path);
    command
        .arg("--identity")
        .env_clear()
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    apply_curated_environment(&mut command);
    if let Some(signal) = cancellation.as_deref().and_then(pending_signal) {
        return Ok(Some(signal));
    }
    let mut child = command
        .spawn()
        .map_err(|_| owned("helper-missing", "the sudo helper could not be started"))?;
    let stdout = child.stdout.take().ok_or_else(|| {
        owned(
            "helper-mismatch",
            "the sudo helper identity could not be read",
        )
    })?;
    let limit = expected.len() as u64 + 1;
    let mut reader = tokio::spawn(async move {
        let mut output = Vec::new();
        stdout
            .take(limit)
            .read_to_end(&mut output)
            .await
            .map_err(|_| ())?;
        Ok::<_, ()>(output)
    });
    let deadline = tokio::time::sleep(timeout);
    tokio::pin!(deadline);
    let mut status = None;
    let mut output = None;
    while status.is_none() || output.is_none() {
        tokio::select! {
            biased;
            signal = wait_for_optional_signal(&mut cancellation) => {
                let _ = child.start_kill();
                let _ = child.wait().await;
                reader.abort();
                let _ = reader.await;
                return Ok(Some(signal));
            }
            _ = &mut deadline => {
                let _ = child.start_kill();
                let _ = child.wait().await;
                reader.abort();
                let _ = reader.await;
                return Err(owned(
                    "helper-timeout",
                    "the sudo helper identity check timed out",
                ));
            }
            result = child.wait(), if status.is_none() => status = Some(result),
            result = &mut reader, if output.is_none() => output = Some(result),
        }
    }
    let status = status
        .and_then(Result::ok)
        .ok_or_else(|| owned("helper-mismatch", "the sudo helper identity check failed"))?;
    let output = output
        .and_then(Result::ok)
        .and_then(Result::ok)
        .ok_or_else(|| owned("helper-mismatch", "the sudo helper identity check failed"))?;
    if !status.success() || output != expected.as_bytes() {
        return Err(owned(
            "helper-mismatch",
            "the sudo helper identity or protocol version does not match agentenv",
        ));
    }
    Ok(None)
}

async fn wait_for_signal(cancellation: &mut Cancellation) -> i32 {
    loop {
        if let Some(signal) = pending_signal(cancellation) {
            return signal;
        }
        if cancellation.changed().await.is_err() {
            std::future::pending::<()>().await;
        }
    }
}

async fn wait_for_optional_signal(cancellation: &mut Option<&mut Cancellation>) -> i32 {
    match cancellation.as_deref_mut() {
        Some(cancellation) => wait_for_signal(cancellation).await,
        None => std::future::pending().await,
    }
}

fn validate_executable(path: &Path, reason: &str) -> Result<(), AppError> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|_| owned(reason, "the configured executable is unavailable"))?;
    let owner = metadata.uid();
    if metadata.file_type().is_symlink()
        || !metadata.file_type().is_file()
        || (owner != 0 && owner != unsafe { libc::getuid() })
        || metadata.permissions().mode() & 0o111 == 0
        || metadata.permissions().mode() & 0o022 != 0
    {
        return Err(owned(
            reason,
            "the configured executable is not a safe executable file",
        ));
    }
    Ok(())
}

fn validate_account(expected: &str) -> Result<(), AppError> {
    unsafe {
        let uid = libc::getuid();
        if libc::geteuid() != uid {
            return Err(owned(
                "account-mismatch",
                "agentenv is already running with a different effective user",
            ));
        }
        let size = libc::sysconf(libc::_SC_GETPW_R_SIZE_MAX);
        let size = if size > 0 { size as usize } else { 16_384 };
        let mut buffer = vec![0_u8; size];
        let mut record = std::mem::MaybeUninit::<libc::passwd>::uninit();
        let mut result = std::ptr::null_mut();
        if libc::getpwuid_r(
            uid,
            record.as_mut_ptr(),
            buffer.as_mut_ptr().cast(),
            buffer.len(),
            &mut result,
        ) != 0
            || result.is_null()
        {
            return Err(owned(
                "account-unavailable",
                "the invoking account could not be determined",
            ));
        }
        let record = record.assume_init();
        let actual = std::ffi::CStr::from_ptr(record.pw_name)
            .to_str()
            .map_err(|_| {
                owned(
                    "account-unavailable",
                    "the invoking account name is not valid UTF-8",
                )
            })?;
        if actual != expected {
            return Err(owned(
                "account-mismatch",
                "the configured sudo account is not the invoking account",
            ));
        }
    }
    Ok(())
}

fn apply_curated_environment(command: &mut Command) {
    command.env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin");
    for name in ["HOME", "LANG", "LC_ALL", "LC_CTYPE", "TZ"] {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
}

struct Runtime {
    _directory: tempfile::TempDir,
    socket: std::path::PathBuf,
    listener: UnixListener,
}

impl Runtime {
    fn create() -> Result<Self, AppError> {
        let directory = tempfile::Builder::new()
            .prefix("agentenv-sudo-")
            .tempdir()
            .map_err(|_| {
                owned(
                    "broker-setup",
                    "could not create a private runtime directory",
                )
            })?;
        fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o700))
            .map_err(|_| owned("broker-setup", "could not protect the runtime directory"))?;
        let socket = directory.path().join("askpass.sock");
        let listener = UnixListener::bind(&socket)
            .map_err(|_| owned("broker-setup", "could not create the sudo helper socket"))?;
        fs::set_permissions(&socket, fs::Permissions::from_mode(0o600))
            .map_err(|_| owned("broker-setup", "could not protect the sudo helper socket"))?;
        let runtime = Self {
            _directory: directory,
            socket,
            listener,
        };
        runtime.validate()?;
        Ok(runtime)
    }

    fn validate(&self) -> Result<(), AppError> {
        let uid = unsafe { libc::getuid() };
        let directory = fs::symlink_metadata(self._directory.path())
            .map_err(|_| owned("broker-setup", "the runtime directory disappeared"))?;
        let socket = fs::symlink_metadata(&self.socket)
            .map_err(|_| owned("broker-setup", "the sudo helper socket disappeared"))?;
        if directory.file_type().is_symlink()
            || !directory.is_dir()
            || directory.uid() != uid
            || directory.mode() & 0o077 != 0
            || socket.file_type().is_symlink()
            || !socket.file_type().is_socket()
            || socket.uid() != uid
            || socket.mode() & 0o177 != 0
        {
            return Err(owned(
                "broker-setup",
                "the sudo helper socket is not owner-only",
            ));
        }
        Ok(())
    }
}

fn random_session() -> Result<String, AppError> {
    let mut bytes = [0_u8; 24];
    File::open("/dev/urandom")
        .and_then(|mut source| source.read_exact(&mut bytes))
        .map_err(|_| owned("broker-setup", "could not create a sudo session identifier"))?;
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        write!(&mut encoded, "{byte:02x}")
            .map_err(|_| owned("broker-setup", "could not create a sudo session identifier"))?;
    }
    Ok(encoded)
}

fn owned(reason: &str, detail: &str) -> AppError {
    AppError::SudoExecution(format!("{reason}: {detail}"))
}

fn uncertain(detail: &str) -> AppError {
    AppError::SudoCompletionUnconfirmed(detail.to_owned())
}

fn credential_failure() -> AppError {
    AppError::Credential(
        "authentication credential unavailable; check its provider and permitted usage separately"
            .to_owned(),
    )
}
