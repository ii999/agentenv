//! Remote helper lifecycle built on the bounded sudo transport.

use tokio::io::{AsyncRead, AsyncWrite};

use crate::error::AppError;

#[cfg(unix)]
use {
    std::os::fd::OwnedFd,
    std::os::unix::net::UnixStream as StdUnixStream,
    std::process::Stdio,
    std::time::Duration,
    tokio::io::{AsyncReadExt, AsyncWriteExt},
    tokio::net::UnixStream,
    tokio::sync::{mpsc, oneshot},
};

#[cfg(unix)]
use crate::credential::Secret;

#[cfg(unix)]
use super::{
    cancellation_channel, check_local, companion_path, execute_local,
    protocol::{
        read_frame, write_frame, Cancel, ExecutionResult, Failure, FailureReason, Frame, Hello,
        MessageKind, Mode, PasswordRequest, Ready, Start, Stream, MAX_STREAM_CHUNK_SIZE, VERSION,
    },
    transport::{self, Side, StreamEvent, StreamReceiver, TransportError, TransportSender},
    ExecutionRequest, LocalOptions, ProcessIo, SUDO_PASSWORD_LIMIT,
};

#[cfg(unix)]
const INITIAL_HELLO_TIMEOUT: Duration = Duration::from_secs(30);
/// How long output may stay open after the process exited and Cancel arrived.
#[cfg(unix)]
const OUTPUT_CANCEL_GRACE: Duration = Duration::from_secs(5);

/// Serves one check or execute request. The caller supplies the protocol-only
/// stdin/stdout channel; target streams remain on their own bounded frames.
#[cfg(unix)]
pub async fn serve<R, W>(mut reader: R, mut writer: W) -> Result<(), AppError>
where
    R: AsyncRead + Unpin + Send + 'static,
    W: AsyncWrite + Unpin + Send + 'static,
{
    let hello_frame = tokio::time::timeout(INITIAL_HELLO_TIMEOUT, read_frame(&mut reader))
        .await
        .map_err(|_| remote_error("hello-timeout", "the remote hello timed out"))?
        .map_err(|_| remote_error("protocol", "the remote hello was invalid"))?
        .ok_or_else(|| remote_error("protocol", "the remote hello was missing"))?;
    if hello_frame.kind() != MessageKind::Hello {
        return Err(remote_error(
            "protocol",
            "the first remote message was not hello",
        ));
    }
    let request_id = hello_frame.request_id();
    let hello: Hello = hello_frame
        .decode_metadata()
        .map_err(|_| remote_error("protocol", "the remote hello metadata was invalid"))?;
    let options = match options_from_hello(&hello) {
        Ok(options) => options,
        Err(error) => {
            send_direct_failure(&mut writer, request_id, FailureReason::Preflight).await;
            return Err(error);
        }
    };
    if let Err(error) = check_local(&options).await {
        send_direct_failure(&mut writer, request_id, FailureReason::Preflight).await;
        return Err(error);
    }

    let cwd = std::env::current_dir()
        .ok()
        .and_then(|path| path.into_os_string().into_string().ok())
        .ok_or_else(|| remote_error("preflight", "the remote working directory is unavailable"))?;
    let ready = Ready {
        build: env!("CARGO_PKG_VERSION").to_owned(),
        protocol: VERSION,
        platform: std::env::consts::OS.to_owned(),
        auth_user: hello.auth_user.clone(),
        uid: unsafe { libc::getuid() },
        cwd,
        password_limit: SUDO_PASSWORD_LIMIT,
        features: vec![
            "one-shot-auth".to_owned(),
            "binary-streams".to_owned(),
            "credits".to_owned(),
            "cancel".to_owned(),
        ],
    };
    let ready = Frame::metadata(MessageKind::Ready, request_id, &ready)
        .map_err(|_| remote_error("protocol", "the remote ready frame could not be built"))?;
    write_frame(&mut writer, &ready)
        .await
        .map_err(|_| remote_error("transport", "the remote ready frame could not be sent"))?;
    writer
        .flush()
        .await
        .map_err(|_| remote_error("transport", "the remote ready frame could not be sent"))?;

    if hello.mode == Mode::Check {
        writer
            .shutdown()
            .await
            .map_err(|_| remote_error("transport", "the remote check could not close cleanly"))?;
        return Ok(());
    }

    serve_execution(reader, writer, request_id, hello, options).await
}

#[cfg(not(unix))]
pub async fn serve<R, W>(_reader: R, _writer: W) -> Result<(), AppError>
where
    R: AsyncRead + Unpin + Send + 'static,
    W: AsyncWrite + Unpin + Send + 'static,
{
    Err(remote_error(
        "unsupported-platform",
        "the remote sudo helper is unavailable on this platform",
    ))
}

#[cfg(unix)]
async fn serve_execution<R, W>(
    reader: R,
    writer: W,
    request_id: u64,
    hello: Hello,
    options: LocalOptions,
) -> Result<(), AppError>
where
    R: AsyncRead + Unpin + Send + 'static,
    W: AsyncWrite + Unpin + Send + 'static,
{
    let mut session = transport::spawn(reader, writer, request_id, Side::Helper);
    let mut failure = session.failure();
    let start_frame = tokio::time::timeout(Duration::from_secs(hello.setup_timeout_secs), async {
        tokio::select! {
            frame = session.control.recv() => Ok(frame),
            changed = failure.changed() => {
                if changed.is_err() || failure.borrow().is_some() {
                    Err(remote_error("protocol", "a message arrived before start"))
                } else {
                    Err(remote_error("transport", "the remote transport failed before start"))
                }
            }
        }
    })
    .await
    .map_err(|_| remote_error("start-timeout", "the remote start request timed out"))?;
    let start_frame = match start_frame {
        Ok(Some(frame)) => frame,
        Ok(None) => {
            if failure.borrow().is_some() {
                return fail_session(
                    &session.sender,
                    request_id,
                    FailureReason::Protocol,
                    remote_error("protocol", "a message arrived before start"),
                )
                .await;
            }
            return Err(remote_error(
                "transport",
                "the remote session closed before start",
            ));
        }
        Err(error) => {
            return fail_session(&session.sender, request_id, FailureReason::Protocol, error).await;
        }
    };
    if start_frame.kind() != MessageKind::Start {
        return fail_session(
            &session.sender,
            request_id,
            FailureReason::Protocol,
            remote_error("protocol", "the remote start request was out of order"),
        )
        .await;
    }
    let start: Start = start_frame
        .decode_metadata()
        .map_err(|_| remote_error("protocol", "the remote start metadata was invalid"))?;
    if start.auth_user != hello.auth_user {
        return fail_session(
            &session.sender,
            request_id,
            FailureReason::Protocol,
            remote_error("protocol", "the remote authentication account changed"),
        )
        .await;
    }
    let request =
        match ExecutionRequest::new(start.executable, start.arguments, start.cwd.map(Into::into)) {
            Ok(request) => request,
            Err(error) => {
                return fail_session(&session.sender, request_id, FailureReason::Preflight, error)
                    .await;
            }
        };
    let options = LocalOptions {
        run_as: start.run_as,
        ..options
    };
    if let Err(error) = options.validate() {
        return fail_session(&session.sender, request_id, FailureReason::Preflight, error).await;
    }
    session.activate_streams().map_err(transport_error)?;

    let stdin_events = session
        .take_stream(Stream::Stdin)
        .map_err(transport_error)?;
    let (stdin_io, stdin_stdio) = socket_pair()?;
    let (stdout_io, stdout_stdio) = socket_pair()?;
    let (stderr_io, stderr_stdio) = socket_pair()?;
    let stdin_sender = session.sender.clone();
    let (stdin_stop, stdin_stopping) = tokio::sync::watch::channel(false);
    let stdin_task = tokio::spawn(pump_stdin(
        stdin_io,
        stdin_events,
        stdin_sender,
        stdin_stopping,
    ));
    let stdout_sender = session.sender.clone();
    let mut stdout_task = tokio::spawn(pump_output(stdout_io, Stream::Stdout, stdout_sender));
    let stderr_sender = session.sender.clone();
    let mut stderr_task = tokio::spawn(pump_output(stderr_io, Stream::Stderr, stderr_sender));

    let (cancel_tx, cancel_rx) = cancellation_channel();
    let (auth_tx, mut auth_rx) = mpsc::channel::<AuthRequest>(1);
    let password_sender = session.sender.clone();
    let auth_user = hello.auth_user;
    let resolver = move || async move {
        let (reply, received) = oneshot::channel();
        let (ready, ready_received) = oneshot::channel();
        auth_tx
            .send(AuthRequest { reply, ready })
            .await
            .map_err(|_| credential_error())?;
        ready_received.await.map_err(|_| credential_error())?;
        let request = Frame::metadata(
            MessageKind::PasswordRequest,
            request_id,
            &PasswordRequest { auth_user },
        )
        .map_err(|_| credential_error())?;
        password_sender
            .send_control(request)
            .await
            .map_err(|_| credential_error())?;
        received.await.map_err(|_| credential_error())?
    };
    let io = ProcessIo {
        stdin: stdin_stdio,
        stdout: stdout_stdio,
        stderr: stderr_stdio,
    };
    let mut engine = Box::pin(execute_local(request, options, io, cancel_rx, resolver));
    let mut auth_pending: Option<oneshot::Sender<Result<Secret, AppError>>> = None;
    let mut auth_used = false;

    // Once sudo has started, a stopped session cannot prove what ran, so
    // failures report completion as unknown rather than a protocol error.
    let engine_result = loop {
        tokio::select! {
            request = auth_rx.recv(), if !auth_used => {
                auth_used = true;
                // A closed channel means the engine will never ask for a
                // password (cancelled or finished); keep waiting for its result.
                if let Some(request) = request {
                    auth_pending = Some(request.reply);
                    let _ = request.ready.send(());
                }
            }
            control = session.control.recv() => {
                let Some(control) = control else {
                    cancel_and_wait(&cancel_tx, &mut engine).await;
                    return Err(remote_error("transport", "the remote control channel closed"));
                };
                match control.kind() {
                    MessageKind::PasswordResponse => {
                        let Some(reply) = auth_pending.take() else {
                            cancel_and_wait(&cancel_tx, &mut engine).await;
                            return fail_session(&session.sender, request_id, FailureReason::CompletionUnknown, remote_error("protocol", "a password response arrived out of order")).await;
                        };
                        let secret = match control.consume_secret() {
                            Ok(secret) => secret,
                            Err(_) => {
                                let _ = reply.send(Err(credential_error()));
                                cancel_and_wait(&cancel_tx, &mut engine).await;
                                return fail_session(&session.sender, request_id, FailureReason::CompletionUnknown, remote_error("protocol", "the password response was invalid")).await;
                            }
                        };
                        let _ = reply.send(Ok(secret));
                    }
                    MessageKind::PasswordUnavailable => {
                        let Some(reply) = auth_pending.take() else {
                            cancel_and_wait(&cancel_tx, &mut engine).await;
                            return fail_session(&session.sender, request_id, FailureReason::CompletionUnknown, remote_error("protocol", "password unavailability arrived out of order")).await;
                        };
                        let _ = reply.send(Err(credential_error()));
                    }
                    MessageKind::Cancel => {
                        let cancel: Cancel = match control.decode_metadata() {
                            Ok(cancel) => cancel,
                            Err(_) => {
                                cancel_and_wait(&cancel_tx, &mut engine).await;
                                return fail_session(&session.sender, request_id, FailureReason::CompletionUnknown, remote_error("protocol", "the remote cancel request was invalid")).await;
                            }
                        };
                        if !matches!(cancel.signal, libc::SIGHUP | libc::SIGINT | libc::SIGTERM) {
                            cancel_and_wait(&cancel_tx, &mut engine).await;
                            return fail_session(&session.sender, request_id, FailureReason::CompletionUnknown, remote_error("protocol", "the remote cancel signal was invalid")).await;
                        }
                        let _ = cancel_tx.send(cancel.signal);
                    }
                    _ => {
                        cancel_and_wait(&cancel_tx, &mut engine).await;
                        return fail_session(&session.sender, request_id, FailureReason::CompletionUnknown, remote_error("protocol", "a remote control message arrived out of order")).await;
                    }
                }
            }
            changed = failure.changed() => {
                if changed.is_err() || failure.borrow().is_some() {
                    cancel_and_wait(&cancel_tx, &mut engine).await;
                    return Err(remote_error("transport", "the remote transport failed"));
                }
            }
            result = &mut engine => break result,
        }
    };
    drop(auth_pending);
    let _ = stdin_stop.send(true);

    let mut stdout_done = false;
    let mut stderr_done = false;
    // The process has exited, so Cancel can only bound how long output held
    // open by a surviving descendant may delay the terminal frame.
    let mut output_deadline: Option<tokio::time::Instant> = None;
    while !stdout_done || !stderr_done {
        tokio::select! {
            result = &mut stdout_task, if !stdout_done => {
                if result
                    .map_err(|_| transport_error(TransportError::Closed))
                    .and_then(|result| result)
                    .is_err()
                {
                    stderr_task.abort();
                    return fail_session(
                        &session.sender,
                        request_id,
                        FailureReason::CompletionUnknown,
                        AppError::SudoCompletionUnconfirmed(
                            "remote-output: target stdout could not be completed".to_owned(),
                        ),
                    )
                    .await;
                }
                stdout_done = true;
            }
            result = &mut stderr_task, if !stderr_done => {
                if result
                    .map_err(|_| transport_error(TransportError::Closed))
                    .and_then(|result| result)
                    .is_err()
                {
                    stdout_task.abort();
                    return fail_session(
                        &session.sender,
                        request_id,
                        FailureReason::CompletionUnknown,
                        AppError::SudoCompletionUnconfirmed(
                            "remote-output: target stderr could not be completed".to_owned(),
                        ),
                    )
                    .await;
                }
                stderr_done = true;
            }
            control = session.control.recv() => {
                let Some(control) = control else {
                    stdout_task.abort();
                    stderr_task.abort();
                    return Err(remote_error("transport", "the remote control channel closed"));
                };
                match control.kind() {
                    // A reply to a request that crossed the client's Cancel
                    // is obsolete once the process has exited.
                    MessageKind::PasswordUnavailable => continue,
                    MessageKind::PasswordResponse => {
                        drop(control.consume_secret());
                        continue;
                    }
                    MessageKind::Cancel if matches!(control.decode_metadata::<Cancel>(), Ok(cancel) if matches!(cancel.signal, libc::SIGHUP | libc::SIGINT | libc::SIGTERM)) => {}
                    _ => {
                        // The target already ran; only its output is incomplete.
                        stdout_task.abort();
                        stderr_task.abort();
                        return fail_session(&session.sender, request_id, FailureReason::CompletionUnknown, remote_error("protocol", "an invalid control message arrived after process completion")).await;
                    }
                }
                output_deadline.get_or_insert_with(|| tokio::time::Instant::now() + OUTPUT_CANCEL_GRACE);
            }
            changed = failure.changed() => {
                if changed.is_err() || failure.borrow().is_some() {
                    stdout_task.abort();
                    stderr_task.abort();
                    return Err(remote_error("transport", "the remote transport failed"));
                }
            }
            _ = sleep_until_deadline(output_deadline), if output_deadline.is_some() => {
                stdout_task.abort();
                stderr_task.abort();
                return fail_session(
                    &session.sender,
                    request_id,
                    FailureReason::CompletionUnknown,
                    AppError::SudoCompletionUnconfirmed(
                        "remote-output: target output did not finish after cancellation".to_owned(),
                    ),
                )
                .await;
            }
        }
    }

    // Stop stdin credit updates first: no frame may follow the terminal.
    stdin_task.abort();
    let _ = stdin_task.await;
    match engine_result {
        Ok(outcome) => {
            let result = Frame::metadata(
                MessageKind::Result,
                request_id,
                &ExecutionResult {
                    exit_code: outcome.exit_code,
                    signal: outcome.signal,
                    password_delivered: outcome.password_delivered,
                },
            )
            .map_err(|_| remote_error("protocol", "the remote result could not be built"))?;
            session
                .sender
                .send_terminal(result)
                .await
                .map_err(transport_error)
        }
        Err(error) => {
            let reason = match error {
                AppError::Credential(_) => FailureReason::Credential,
                AppError::SudoCompletionUnconfirmed(_) => FailureReason::CompletionUnknown,
                _ => FailureReason::Preflight,
            };
            fail_session(&session.sender, request_id, reason, error).await
        }
    }
}

#[cfg(unix)]
struct AuthRequest {
    reply: oneshot::Sender<Result<Secret, AppError>>,
    ready: oneshot::Sender<()>,
}

#[cfg(unix)]
async fn pump_stdin(
    mut stream: UnixStream,
    mut events: StreamReceiver,
    sender: TransportSender,
    mut stopping: tokio::sync::watch::Receiver<bool>,
) -> Result<(), AppError> {
    loop {
        let event = tokio::select! {
            changed = stopping.changed() => {
                if changed.is_err() || *stopping.borrow() {
                    return drain_stdin(events, sender).await;
                }
                continue;
            }
            event = events.recv() => event,
        };
        let Some(event) = event else {
            return Err(remote_error("transport", "the remote stdin channel closed"));
        };
        match event {
            StreamEvent::Chunk(bytes) => {
                let write = tokio::select! {
                    changed = stopping.changed() => {
                        if changed.is_err() || *stopping.borrow() {
                            None
                        } else {
                            continue;
                        }
                    }
                    result = stream.write_all(&bytes) => Some(result),
                };
                sender
                    .acknowledge(Stream::Stdin, bytes.len() as u32)
                    .await
                    .map_err(transport_error)?;
                if !matches!(write, Some(Ok(()))) {
                    return drain_stdin(events, sender).await;
                }
            }
            StreamEvent::Eof => {
                stream
                    .shutdown()
                    .await
                    .map_err(|_| remote_error("stream", "target stdin could not be closed"))?;
                return Ok(());
            }
        }
    }
}

#[cfg(unix)]
async fn drain_stdin(mut events: StreamReceiver, sender: TransportSender) -> Result<(), AppError> {
    while let Some(event) = events.recv().await {
        match event {
            StreamEvent::Chunk(bytes) => sender
                .acknowledge(Stream::Stdin, bytes.len() as u32)
                .await
                .map_err(transport_error)?,
            StreamEvent::Eof => return Ok(()),
        }
    }
    Ok(())
}

#[cfg(unix)]
async fn pump_output(
    mut stream: UnixStream,
    stream_kind: Stream,
    sender: TransportSender,
) -> Result<(), AppError> {
    let mut buffer = vec![0_u8; MAX_STREAM_CHUNK_SIZE];
    loop {
        let count = stream
            .read(&mut buffer)
            .await
            .map_err(|_| remote_error("stream", "target output could not be read"))?;
        if count == 0 {
            sender
                .send_eof(stream_kind)
                .await
                .map_err(transport_error)?;
            return Ok(());
        }
        sender
            .send_stream(stream_kind, buffer[..count].to_vec())
            .await
            .map_err(transport_error)?;
    }
}

#[cfg(unix)]
fn socket_pair() -> Result<(UnixStream, Stdio), AppError> {
    let (helper, target) = StdUnixStream::pair()
        .map_err(|_| remote_error("stream", "a target stream could not be created"))?;
    helper
        .set_nonblocking(true)
        .map_err(|_| remote_error("stream", "a target stream could not be configured"))?;
    let helper = UnixStream::from_std(helper)
        .map_err(|_| remote_error("stream", "a target stream could not be configured"))?;
    let target: OwnedFd = target.into();
    Ok((helper, Stdio::from(target)))
}

#[cfg(unix)]
fn options_from_hello(hello: &Hello) -> Result<LocalOptions, AppError> {
    if hello.setup_timeout_secs == 0
        || hello.setup_timeout_secs > 300
        || hello.auth_timeout_secs == 0
        || hello.auth_timeout_secs > 300
    {
        return Err(remote_error(
            "preflight",
            "remote timeout values must be between 1 and 300 seconds",
        ));
    }
    let executable = std::env::current_exe()
        .map_err(|_| remote_error("helper-missing", "the helper path is unavailable"))?;
    Ok(LocalOptions {
        sudo_path: hello.sudo_path.clone().into(),
        helper_path: companion_path(&executable)?,
        auth_user: hello.auth_user.clone(),
        run_as: "root".to_owned(),
        setup_timeout: Duration::from_secs(hello.setup_timeout_secs),
        auth_timeout: Duration::from_secs(hello.auth_timeout_secs),
    })
}

#[cfg(unix)]
async fn send_direct_failure<W: AsyncWrite + Unpin>(
    writer: &mut W,
    request_id: u64,
    reason: FailureReason,
) {
    let Ok(frame) = Frame::metadata(MessageKind::Failure, request_id, &Failure { reason }) else {
        return;
    };
    if write_frame(writer, &frame).await.is_ok() {
        let _ = writer.flush().await;
    }
}

#[cfg(unix)]
async fn fail_session(
    sender: &TransportSender,
    request_id: u64,
    reason: FailureReason,
    error: AppError,
) -> Result<(), AppError> {
    if let Ok(frame) = Frame::metadata(MessageKind::Failure, request_id, &Failure { reason }) {
        let _ = sender.send_terminal(frame).await;
    }
    Err(error)
}

#[cfg(unix)]
async fn sleep_until_deadline(deadline: Option<tokio::time::Instant>) {
    match deadline {
        Some(deadline) => tokio::time::sleep_until(deadline).await,
        None => std::future::pending().await,
    }
}

#[cfg(unix)]
async fn cancel_and_wait<F>(
    cancel: &tokio::sync::watch::Sender<i32>,
    engine: &mut std::pin::Pin<Box<F>>,
) where
    F: std::future::Future<Output = Result<super::ExecutionOutcome, AppError>>,
{
    let _ = cancel.send(libc::SIGTERM);
    let _ = tokio::time::timeout(Duration::from_secs(6), engine).await;
}

#[cfg(unix)]
fn credential_error() -> AppError {
    AppError::Credential("remote sudo credential was unavailable".to_owned())
}

#[cfg(unix)]
fn transport_error(_error: TransportError) -> AppError {
    remote_error("transport", "the remote transport failed")
}

fn remote_error(reason: &str, message: &str) -> AppError {
    AppError::SudoExecution(format!("{reason}: {message}"))
}
