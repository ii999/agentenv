//! One foreground SSH session, with independent login and sudo authentication.

use super::{protocol::Ready, Cancellation, ExecutionOutcome, ExecutionRequest};
use crate::config::{Config, SudoTarget};
use crate::error::AppError;
use std::time::Duration;

pub struct RemoteOutcome {
    pub ready: Option<Ready>,
    pub execution: Option<ExecutionOutcome>,
    pub close_failed: bool,
    pub output_interrupted: bool,
    /// A valid terminal frame was followed by more data or an invalid close.
    pub protocol_after_result: bool,
}

pub async fn execute(
    config: &Config,
    target: &SudoTarget,
    request: Option<ExecutionRequest>,
    setup_timeout: Duration,
    auth_timeout: Duration,
    cancellation: Cancellation,
) -> Result<RemoteOutcome, AppError> {
    #[cfg(any(unix, windows))]
    return native::execute(
        config,
        target,
        request,
        setup_timeout,
        auth_timeout,
        cancellation,
    )
    .await;
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (
            config,
            target,
            request,
            setup_timeout,
            auth_timeout,
            cancellation,
        );
        Err(AppError::SudoExecution(
            "unsupported-platform: this build has no verified Windows SSH credential channel"
                .into(),
        ))
    }
}

#[cfg(any(unix, windows))]
mod native {
    use super::super::{
        protocol::{
            self, Cancel, ExecutionResult, Failure, FailureReason, Frame, Hello,
            MessageKind as Kind, Mode, PasswordRequest, Start, Stream, MAX_STREAM_CHUNK_SIZE,
        },
        ssh::{self, PreparedAuth},
        ssh_askpass,
        transport::{self, Side, StreamEvent, StreamReceiver, TransportSender},
    };
    use super::*;
    use crate::config::CredentialDef;
    use crate::credential::resolver::{self, ResolutionStage};
    use crate::credential::Secret;
    use std::future::Future;
    #[cfg(unix)]
    use std::io::Read;
    use std::path::PathBuf;
    use std::pin::Pin;
    use std::sync::atomic::{AtomicU8, Ordering};
    use std::sync::Arc;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::process::{Child, ChildStdin, ChildStdout};
    use tokio::sync::watch;
    use tokio::time::{timeout, Instant};

    type Operation = Pin<Box<dyn Future<Output = Result<(), AppError>> + Send>>;
    type AuthTiming = watch::Receiver<Option<(Instant, Option<Instant>)>>;

    fn owned(reason: &str) -> AppError {
        AppError::SudoExecution(format!("{reason}: SSH sudo execution could not continue"))
    }
    fn unknown() -> AppError {
        AppError::SudoCompletionUnconfirmed(
            "remote-result-missing: the command may have run; do not retry automatically".into(),
        )
    }
    fn credential_error() -> AppError {
        AppError::Credential(
            "authentication credential unavailable; check its provider separately".into(),
        )
    }
    fn cancelled(signal: i32) -> RemoteOutcome {
        RemoteOutcome {
            ready: None,
            execution: Some(ExecutionOutcome {
                exit_code: None,
                signal: Some(signal),
                password_delivered: false,
            }),
            close_failed: false,
            output_interrupted: false,
            protocol_after_result: false,
        }
    }
    fn definition(config: &Config, name: &str) -> Result<CredentialDef, AppError> {
        config.credential(name).cloned().ok_or_else(|| {
            AppError::NotFound("configured authentication credential is missing".into())
        })
    }
    fn remaining(start: Instant, budget: Duration, timing: Option<&AuthTiming>) -> Duration {
        let elapsed = start.elapsed();
        let authentication = timing
            .and_then(|clock| *clock.borrow())
            .map(|(begin, end)| {
                end.unwrap_or_else(Instant::now)
                    .saturating_duration_since(begin)
            })
            .unwrap_or_default();
        budget.saturating_sub(elapsed.saturating_sub(authentication))
    }

    pub(super) async fn execute(
        config: &Config,
        target: &SudoTarget,
        request: Option<ExecutionRequest>,
        setup_timeout: Duration,
        auth_timeout: Duration,
        mut cancellation: Cancellation,
    ) -> Result<RemoteOutcome, AppError> {
        let started = Instant::now();
        if *cancellation.borrow() > 0 {
            return Ok(cancelled(*cancellation.borrow()));
        }
        let mut bytes = [0; 8];
        #[cfg(unix)]
        std::fs::File::open("/dev/urandom")
            .and_then(|mut file| file.read_exact(&mut bytes))
            .map_err(|_| owned("session-unavailable"))?;
        #[cfg(windows)]
        crate::windows::random(&mut bytes).map_err(|_| owned("session-unavailable"))?;
        let request_id = u64::from_be_bytes(bytes).max(1);
        let start_frame = request
            .as_ref()
            .map(|request| {
                Frame::metadata(
                    Kind::Start,
                    request_id,
                    &Start {
                        executable: request
                            .executable()
                            .to_str()
                            .expect("validated UTF-8 request")
                            .into(),
                        arguments: request.arguments().to_vec(),
                        cwd: request
                            .cwd()
                            .map(|path| path.to_str().expect("validated UTF-8 cwd").into()),
                        run_as: target.run_as.clone(),
                        auth_user: target.auth_user.clone(),
                    },
                )
            })
            .transpose()
            .map_err(|_| {
                AppError::Usage("sudo command metadata exceeds the remote protocol limit".into())
            })?;
        let prepared = tokio::select! {
            biased;
            _ = cancellation_requested(&mut cancellation) => return Ok(cancelled(*cancellation.borrow())),
            result = ssh::prepare(target, setup_timeout) => result?,
        };
        if *cancellation.borrow() > 0 {
            return Ok(cancelled(*cancellation.borrow()));
        }
        let executable = std::env::current_exe().map_err(|_| owned("resolver-unavailable"))?;
        let sudo_credential = definition(config, &target.credential.name)?;
        let mut command = prepared.command();
        let mut timing = None;
        let login = match &prepared.auth {
            PreparedAuth::PublicKey => None,
            PreparedAuth::Password {
                credential,
                expected_prompt,
            } => {
                let path = executable
                    .parent()
                    .ok_or_else(|| owned("helper-missing"))?
                    .join(if cfg!(windows) {
                        "agentenv-ssh-askpass.exe"
                    } else {
                        "agentenv-ssh-askpass"
                    });
                let budget = remaining(started, setup_timeout, None);
                if budget.is_zero() {
                    return Err(owned("setup-timeout"));
                }
                let session = tokio::select! {
                    biased;
                    _ = cancellation_requested(&mut cancellation) => return Ok(cancelled(*cancellation.borrow())),
                    result = ssh_askpass::Session::create(&path, budget) => result?,
                };
                timing = Some(session.auth_timing());
                session.configure(&mut command);
                Some((
                    session,
                    definition(config, &credential.name)?,
                    expected_prompt.clone(),
                ))
            }
        };
        if *cancellation.borrow() > 0 {
            return Ok(cancelled(*cancellation.borrow()));
        }
        if remaining(started, setup_timeout, None).is_zero() {
            return Err(owned("setup-timeout"));
        }
        // A terminal or harness signal to agentenv's process group must reach
        // only agentenv, which forwards it as Cancel; ssh would otherwise exit
        // and turn an observable result into completion unknown.
        #[cfg(unix)]
        command.process_group(0);
        #[cfg(windows)]
        command.creation_flags(windows_sys::Win32::System::Threading::CREATE_NO_WINDOW);
        let mut child = command.spawn().map_err(|_| owned("ssh-spawn-failed"))?;
        let ssh_pid = child.id().ok_or_else(|| owned("ssh-spawn-failed"))?;
        let reader = child
            .stdout
            .take()
            .ok_or_else(|| owned("ssh-stream-unavailable"))?;
        let writer = child
            .stdin
            .take()
            .ok_or_else(|| owned("ssh-stream-unavailable"))?;
        let mut stderr = child
            .stderr
            .take()
            .ok_or_else(|| owned("ssh-stream-unavailable"))?;
        // Never relay provider, askpass, helper, or transport diagnostics as
        // target stderr. Continuously discard this independent SSH stream.
        let diagnostic = tokio::spawn(async move {
            let mut bytes = [0; 4096];
            while let Ok(length) = stderr.read(&mut bytes).await {
                if length == 0 {
                    break;
                }
            }
        });
        let diagnostic = AbortTask(diagnostic);
        let broker: Option<Operation> = login.map(|(session, credential, prompt)| {
            let executable = executable.clone();
            Box::pin(async move {
                session
                    .serve(
                        ssh_pid,
                        &prompt,
                        setup_timeout,
                        auth_timeout,
                        || async move {
                            resolver::resolve(
                                &executable,
                                &credential,
                                ResolutionStage::SshPassword,
                                255,
                                auth_timeout,
                            )
                            .await
                        },
                    )
                    .await
            }) as Operation
        });
        let result = session(
            reader,
            writer,
            target,
            request_id,
            start_frame,
            started,
            setup_timeout,
            auth_timeout,
            executable,
            sudo_credential,
            broker,
            timing,
            &mut cancellation,
        )
        .await;
        let result = match result {
            Ok(outcome) if outcome.ready.is_none() => {
                stop_ssh(&mut child).await;
                Ok(outcome)
            }
            Ok(mut outcome) => {
                let close = timeout(Duration::from_secs(5), child.wait()).await;
                if !matches!(close, Ok(Ok(status)) if status.success()) {
                    stop_ssh(&mut child).await;
                    if outcome.execution.is_some() {
                        outcome.close_failed = true;
                    } else {
                        return Err(owned("ssh-close-failed"));
                    }
                }
                Ok(outcome)
            }
            Err(error) => {
                stop_ssh(&mut child).await;
                Err(error)
            }
        };
        drop(diagnostic);
        result
    }

    struct AbortTask<T>(tokio::task::JoinHandle<T>);
    impl<T> Drop for AbortTask<T> {
        fn drop(&mut self) {
            self.0.abort();
        }
    }

    async fn stop_ssh(child: &mut Child) {
        let _ = child.start_kill();
        let _ = timeout(Duration::from_secs(5), child.wait()).await;
    }

    #[allow(clippy::too_many_arguments)]
    async fn session(
        mut reader: ChildStdout,
        mut writer: ChildStdin,
        target: &SudoTarget,
        request_id: u64,
        start_frame: Option<Frame>,
        started: Instant,
        setup_timeout: Duration,
        auth_timeout: Duration,
        executable: PathBuf,
        credential: CredentialDef,
        mut broker: Option<Operation>,
        mut timing: Option<AuthTiming>,
        cancellation: &mut Cancellation,
    ) -> Result<RemoteOutcome, AppError> {
        let hello = Frame::metadata(
            Kind::Hello,
            request_id,
            &Hello {
                mode: if start_frame.is_some() {
                    Mode::Execute
                } else {
                    Mode::Check
                },
                sudo_path: target
                    .sudo_path
                    .to_str()
                    .ok_or_else(|| owned("invalid-target"))?
                    .into(),
                auth_user: target.auth_user.clone(),
                setup_timeout_secs: setup_timeout.as_secs(),
                auth_timeout_secs: auth_timeout.as_secs(),
            },
        )
        .map_err(|_| owned("invalid-target"))?;
        let mut handshake = Box::pin(async {
            protocol::write_frame(&mut writer, &hello)
                .await
                .map_err(|_| owned("ssh-connect-failed"))?;
            writer
                .flush()
                .await
                .map_err(|_| owned("ssh-connect-failed"))?;
            let frame = protocol::read_frame(&mut reader)
                .await
                .map_err(|_| owned("helper-handshake-invalid"))?
                .ok_or_else(|| owned("helper-handshake-missing"))?;
            if frame.kind() != Kind::Ready || frame.request_id() != request_id {
                return Err(owned("helper-handshake-invalid"));
            }
            let ready: Ready = frame
                .decode_metadata()
                .map_err(|_| owned("helper-handshake-invalid"))?;
            validate_ready(&ready, target)?;
            Ok(ready)
        });
        let mut cancellation_open = true;
        let mut timing_open = timing.is_some();
        let ready = loop {
            if *cancellation.borrow() > 0 {
                return Ok(cancelled(*cancellation.borrow()));
            }
            let budget = remaining(started, setup_timeout, timing.as_ref());
            if budget.is_zero() {
                return Err(owned("setup-timeout"));
            }
            tokio::select! {
                biased;
                changed = cancellation.changed(), if cancellation_open => {
                    if changed.is_err() { cancellation_open = false; }
                }
                result = optional(&mut broker), if broker.is_some() => {
                    result?;
                    broker = None;
                }
                result = &mut handshake => break result?,
                changed = timing_changed(&mut timing), if timing_open => {
                    if changed.is_err() { timing_open = false; }
                }
                _ = tokio::time::sleep(budget) => {}
            }
        };
        drop(handshake);
        drop(broker);
        if *cancellation.borrow() > 0 {
            return Ok(cancelled(*cancellation.borrow()));
        }
        let Some(start_frame) = start_frame else {
            let budget = remaining(started, setup_timeout, timing.as_ref());
            let end = tokio::select! {
                biased;
                _ = cancellation_requested(cancellation) => return Ok(cancelled(*cancellation.borrow())),
                result = timeout(budget, protocol::read_frame(&mut reader)) => {
                    result.map_err(|_| owned("setup-timeout"))?
                        .map_err(|_| owned("helper-check-invalid"))?
                }
            };
            if end.is_some() {
                return Err(owned("helper-check-invalid"));
            }
            return Ok(RemoteOutcome {
                ready: Some(ready),
                execution: None,
                close_failed: false,
                output_interrupted: false,
                protocol_after_result: false,
            });
        };
        // From this point even a partial write can have reached the helper.
        // Missing completion is uncertain, never a reason to replay the command.
        tokio::select! {
            biased;
            _ = cancellation_requested(cancellation) => return Err(unknown()),
            result = async {
                protocol::write_frame(&mut writer, &start_frame).await.map_err(|_| unknown())?;
                writer.flush().await.map_err(|_| unknown())
            } => result?,
        }
        let resolve: Resolve = Box::new(move |limit| {
            Box::pin(async move {
                resolver::resolve(
                    &executable,
                    &credential,
                    ResolutionStage::Sudo,
                    limit,
                    auth_timeout,
                )
                .await
            })
        });
        run_session(
            reader,
            writer,
            request_id,
            ready,
            SessionIo {
                stdin: tokio::io::stdin(),
                stdout: tokio::io::stdout(),
                stderr: tokio::io::stderr(),
            },
            resolve,
            auth_timeout,
            cancellation,
        )
        .await
    }

    async fn timing_changed(
        timing: &mut Option<AuthTiming>,
    ) -> Result<(), watch::error::RecvError> {
        match timing {
            Some(timing) => timing.changed().await,
            None => std::future::pending().await,
        }
    }

    async fn cancellation_requested(cancellation: &mut Cancellation) {
        loop {
            if *cancellation.borrow() > 0 {
                return;
            }
            if cancellation.changed().await.is_err() {
                std::future::pending::<()>().await;
            }
        }
    }
    fn validate_ready(ready: &Ready, target: &SudoTarget) -> Result<(), AppError> {
        if ready.build != env!("CARGO_PKG_VERSION")
            || ready.protocol != protocol::VERSION
            || !matches!(ready.platform.as_str(), "linux" | "macos")
            || ready.auth_user != target.auth_user
            || !(1..=255).contains(&ready.password_limit)
            || !ready.cwd.starts_with('/')
            || ready.cwd.contains('\0')
            || ["one-shot-auth", "binary-streams", "credits", "cancel"]
                .iter()
                .any(|required| !ready.features.iter().any(|feature| feature == required))
        {
            return Err(owned("helper-identity-mismatch"));
        }
        Ok(())
    }

    /// Local endpoints for the target's standard streams.
    pub(super) struct SessionIo<I, O, E> {
        pub stdin: I,
        pub stdout: O,
        pub stderr: E,
    }

    type Resolve = Box<
        dyn FnOnce(usize) -> Pin<Box<dyn Future<Output = Result<Secret, AppError>> + Send>> + Send,
    >;
    type AuthOperation = Pin<Box<dyn Future<Output = Result<(), AuthFailure>> + Send>>;

    enum AuthFailure {
        /// No password bytes left this process.
        Unresolved(AppError),
        /// The response may have been partly or fully written.
        Delivery,
    }

    /// Progress of the one sudo password response.
    const RESPONSE_IDLE: u8 = 0;
    const RESPONSE_WRITING: u8 = 1;
    const RESPONSE_FLUSHED: u8 = 2;

    /// Bound on waiting for the helper's stream to close after a terminal
    /// frame, or for a terminal frame after the local sending side failed.
    const CLOSE_GRACE: Duration = Duration::from_secs(5);

    /// Runs the session after `Start` may have reached the helper. Until a
    /// valid terminal frame arrives, every local or wire failure means the
    /// command may have run, so it is reported as completion unknown.
    #[allow(clippy::too_many_arguments)]
    async fn run_session<R, W, I, O, E>(
        reader: R,
        writer: W,
        request_id: u64,
        ready: Ready,
        io: SessionIo<I, O, E>,
        resolve: Resolve,
        auth_timeout: Duration,
        cancellation: &mut Cancellation,
    ) -> Result<RemoteOutcome, AppError>
    where
        R: tokio::io::AsyncRead + Unpin + Send + 'static,
        W: tokio::io::AsyncWrite + Unpin + Send + 'static,
        I: tokio::io::AsyncRead + Unpin + Send + 'static,
        O: tokio::io::AsyncWrite + Unpin,
        E: tokio::io::AsyncWrite + Unpin,
    {
        let mut transport = transport::spawn(reader, writer, request_id, Side::Client);
        let sender = transport.sender.clone();
        let stdout = transport
            .take_stream(Stream::Stdout)
            .map_err(|_| unknown())?;
        let stderr = transport
            .take_stream(Stream::Stderr)
            .map_err(|_| unknown())?;
        let SessionIo {
            stdin: local_stdin,
            stdout: local_stdout,
            stderr: local_stderr,
        } = io;
        let mut output = Box::pin(async {
            tokio::try_join!(
                pump_output(stdout, local_stdout, Stream::Stdout, sender.clone()),
                pump_output(stderr, local_stderr, Stream::Stderr, sender.clone())
            )?;
            Ok::<(), AppError>(())
        });
        let mut input = AbortTask(tokio::spawn(pump_input(local_stdin, sender.clone())));
        let mut resolve = Some(resolve);
        let response = Arc::new(AtomicU8::new(RESPONSE_IDLE));
        let mut input_done = false;
        let mut output_done = false;
        let mut output_interrupted = false;
        let mut protocol_after_result = false;
        let mut result = None;
        let mut authentication: Option<AuthOperation> = None;
        let mut transport_failure = transport.failure();
        let mut reader_outcome = transport.reader_outcome();
        let mut reader_open = true;
        let mut failure_open = true;
        let mut failure_deadline = None;
        let mut control_open = true;
        let mut cancellation_open = true;
        let mut cancel_deadline = None;
        loop {
            if result.is_some() && (output_done || cancel_deadline.is_some()) {
                output_interrupted |= !output_done;
                break;
            }
            tokio::select! {
                biased;
                changed = cancellation.changed(), if cancellation_open && cancel_deadline.is_none() => {
                    if changed.is_err() { cancellation_open = false; }
                    else if *cancellation.borrow() > 0 {
                        if result.is_some() {
                            // The status is already known; stop waiting on
                            // a blocked local output consumer.
                            output_interrupted = true;
                            break;
                        }
                        authentication = None;
                        input.0.abort();
                        let cancel = Frame::metadata(Kind::Cancel, request_id, &Cancel { signal: *cancellation.borrow() })
                            .map_err(|_| unknown())?;
                        // A terminal frame may already be queued or arriving
                        // even when Cancel cannot be written; keep reading it.
                        // Enqueue without a flush acknowledgement: abandoning a
                        // slow acknowledgement would make the writer skip it.
                        let sent = matches!(
                            timeout(Duration::from_secs(1), sender.send_control(cancel)).await,
                            Ok(Ok(()))
                        );
                        cancel_deadline = Some(Instant::now() + if sent { Duration::from_secs(10) } else { CLOSE_GRACE });
                    }
                }
                frame = transport.control.recv(), if control_open => {
                    let Some(frame) = frame else {
                        control_open = false;
                        if result.is_none() { return Err(unknown()); }
                        continue;
                    };
                    match frame.kind() {
                        Kind::PasswordRequest if resolve.is_some() && result.is_none() && cancel_deadline.is_none() => {
                            let request: PasswordRequest = frame.decode_metadata().map_err(|_| unknown())?;
                            if request.auth_user != ready.auth_user { return Err(unknown()); }
                            let resolve = resolve.take().expect("checked above");
                            let (sender, progress, limit) = (sender.clone(), response.clone(), ready.password_limit);
                            authentication = Some(Box::pin(async move {
                                let deadline = Instant::now() + auth_timeout;
                                let secret = tokio::time::timeout_at(deadline, resolve(limit)).await
                                    .map_err(|_| AuthFailure::Unresolved(credential_error()))?
                                    .map_err(AuthFailure::Unresolved)?;
                                let response = Frame::password(request_id, &secret)
                                    .map_err(|_| AuthFailure::Unresolved(credential_error()))?;
                                drop(secret);
                                progress.store(RESPONSE_WRITING, Ordering::Release);
                                tokio::time::timeout_at(deadline, sender.send_control_flushed(response)).await
                                    .map_err(|_| AuthFailure::Delivery)?
                                    .map_err(|_| AuthFailure::Delivery)?;
                                progress.store(RESPONSE_FLUSHED, Ordering::Release);
                                Ok(())
                            }));
                        }
                        Kind::PasswordRequest if result.is_none() && cancel_deadline.is_some() => {
                            // Crossed our Cancel; decline and keep waiting for
                            // the helper's terminal frame.
                            if let Ok(frame) = Frame::empty(Kind::PasswordUnavailable, request_id) {
                                let _ = timeout(Duration::from_secs(1), sender.send_control_flushed(frame)).await;
                            }
                        }
                        Kind::Result if result.is_none() => {
                            let observed: ExecutionResult = frame.decode_metadata().map_err(|_| unknown())?;
                            if observed.password_delivered
                                && response.load(Ordering::Acquire) == RESPONSE_WRITING
                            {
                                // The flush acknowledgement can trail a fast
                                // Result. Only an already-written response is
                                // settled; resolution never continues here.
                                if let Some(pending) = authentication.take() {
                                    if !matches!(timeout(CLOSE_GRACE, pending).await, Ok(Ok(()))) {
                                        return Err(unknown());
                                    }
                                }
                            }
                            let valid = match (observed.exit_code, observed.signal) {
                                (Some(code), None) => (0..=255).contains(&code),
                                (None, Some(signal)) => (1..=127).contains(&signal),
                                _ => false,
                            };
                            // Delivery is claimable only after this client
                            // flushed its one response, not when it was asked.
                            if !valid || (observed.password_delivered && response.load(Ordering::Acquire) != RESPONSE_FLUSHED) {
                                return Err(unknown());
                            }
                            result = Some(ExecutionOutcome { exit_code: observed.exit_code, signal: observed.signal,
                                password_delivered: observed.password_delivered });
                            authentication = None;
                            input.0.abort();
                        }
                        Kind::Failure if result.is_none() => {
                            let failure: Failure = frame.decode_metadata().map_err(|_| unknown())?;
                            return Err(match failure.reason {
                                FailureReason::Credential => credential_error(),
                                FailureReason::CompletionUnknown => unknown(),
                                FailureReason::Preflight => owned("remote-preflight-failed"),
                                FailureReason::Protocol => owned("remote-protocol-failed"),
                            });
                        }
                        _ if result.is_some() => protocol_after_result = true,
                        _ => return Err(unknown()),
                    }
                }
                resolved = optional(&mut authentication), if authentication.is_some() => {
                    authentication = None;
                    match resolved {
                        Ok(()) => {}
                        Err(AuthFailure::Unresolved(error)) => {
                            if let Ok(frame) = Frame::empty(Kind::PasswordUnavailable, request_id) {
                                let _ = timeout(Duration::from_secs(1), sender.send_control_flushed(frame)).await;
                            }
                            return Err(error);
                        }
                        Err(AuthFailure::Delivery) => return Err(unknown()),
                    }
                }
                completed = &mut output, if !output_done => {
                    output_done = true;
                    if completed.is_err() {
                        if result.is_none() { return Err(unknown()); }
                        output_interrupted = true;
                    }
                }
                completed = &mut input.0, if !input_done => {
                    input_done = true;
                    if matches!(completed, Ok(Err(InputError::Read))) && result.is_none() {
                        return Err(unknown());
                    }
                }
                changed = reader_outcome.changed(), if reader_open => {
                    let outcome = *reader_outcome.borrow();
                    if changed.is_err() || outcome.is_some() { reader_open = false; }
                    if matches!(outcome, Some(Err(_)) | None) && !reader_open {
                        if result.is_none() { return Err(unknown()); }
                        protocol_after_result = true;
                    }
                }
                changed = transport_failure.changed(), if failure_open => {
                    if changed.is_err() { failure_open = false; }
                    // A failed local write does not invalidate frames the
                    // helper already sent; let the reader deliver them.
                    if result.is_none() && failure_deadline.is_none() {
                        failure_deadline = Some(Instant::now() + CLOSE_GRACE);
                    }
                }
                _ = deadline_expired(failure_deadline), if failure_deadline.is_some() && result.is_none() => return Err(unknown()),
                _ = deadline_expired(cancel_deadline), if cancel_deadline.is_some() => return Err(unknown()),
            }
        }
        if reader_open {
            // Keep reading until SSH closes so a duplicate terminal or any
            // later frame is observed instead of silently discarded.
            let closed = timeout(CLOSE_GRACE, async {
                loop {
                    if let Some(outcome) = *reader_outcome.borrow() {
                        return Some(outcome);
                    }
                    if reader_outcome.changed().await.is_err() {
                        return *reader_outcome.borrow();
                    }
                }
            })
            .await;
            if matches!(closed, Ok(Some(Err(_)) | None)) {
                protocol_after_result = true;
            }
        }
        Ok(RemoteOutcome {
            ready: Some(ready),
            execution: result,
            close_failed: false,
            output_interrupted,
            protocol_after_result,
        })
    }

    async fn optional<T>(operation: &mut Option<Pin<Box<dyn Future<Output = T> + Send>>>) -> T {
        match operation {
            Some(operation) => operation.await,
            None => std::future::pending().await,
        }
    }

    async fn deadline_expired(deadline: Option<Instant>) {
        match deadline {
            Some(deadline) => tokio::time::sleep_until(deadline).await,
            None => std::future::pending().await,
        }
    }
    enum InputError {
        Read,
        Transport,
    }
    async fn pump_input<I: tokio::io::AsyncRead + Unpin>(
        mut input: I,
        sender: TransportSender,
    ) -> Result<(), InputError> {
        let mut bytes = vec![0; MAX_STREAM_CHUNK_SIZE];
        loop {
            let length = input.read(&mut bytes).await.map_err(|_| InputError::Read)?;
            if length == 0 {
                return sender
                    .send_eof(Stream::Stdin)
                    .await
                    .map_err(|_| InputError::Transport);
            }
            sender
                .send_stream(Stream::Stdin, bytes[..length].to_vec())
                .await
                .map_err(|_| InputError::Transport)?;
        }
    }
    async fn pump_output<W: tokio::io::AsyncWrite + Unpin>(
        mut source: StreamReceiver,
        mut output: W,
        stream: Stream,
        sender: TransportSender,
    ) -> Result<(), AppError> {
        while let Some(event) = source.recv().await {
            match event {
                StreamEvent::Chunk(bytes) => {
                    output
                        .write_all(&bytes)
                        .await
                        .map_err(|_| owned("output-write-failed"))?;
                    // The helper may close its transport after flushing Result
                    // while this consumer still drains queued output. The
                    // coordinator owns transport failure and completion; an
                    // obsolete credit reply must not discard buffered bytes.
                    match sender.acknowledge(stream, bytes.len() as u32).await {
                        Ok(()) | Err(transport::TransportError::Closed) => {}
                        Err(_) => return Err(owned("stream-credit-invalid")),
                    }
                }
                StreamEvent::Eof => {
                    output
                        .flush()
                        .await
                        .map_err(|_| owned("output-write-failed"))?;
                    return Ok(());
                }
            }
        }
        Err(unknown())
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[tokio::test]
        async fn buffered_output_finishes_after_credit_channel_closes() {
            let (client, mut helper) = tokio::io::duplex(4096);
            let (read, write) = tokio::io::split(client);
            let mut transport = transport::spawn(read, write, 7, Side::Client);
            for frame in [
                Frame::stream(Kind::StdoutChunk, 7, vec![0, 255, 10, 42]).unwrap(),
                Frame::empty(Kind::StdoutEof, 7).unwrap(),
                Frame::empty(Kind::StderrEof, 7).unwrap(),
            ] {
                protocol::write_frame(&mut helper, &frame).await.unwrap();
            }
            let mut failure = transport.failure();
            drop(helper);
            while failure.borrow().is_none() {
                failure.changed().await.unwrap();
            }
            let receiver = transport.take_stream(Stream::Stdout).unwrap();
            let sender = transport.sender.clone();
            drop(transport);
            tokio::task::yield_now().await;
            let mut output = Vec::new();
            pump_output(receiver, &mut output, Stream::Stdout, sender)
                .await
                .expect("already received output survives the obsolete credit reply");
            assert_eq!(output, [0, 255, 10, 42]);
        }

        struct Broken;
        impl tokio::io::AsyncRead for Broken {
            fn poll_read(
                self: Pin<&mut Self>,
                _: &mut std::task::Context<'_>,
                _: &mut tokio::io::ReadBuf<'_>,
            ) -> std::task::Poll<std::io::Result<()>> {
                std::task::Poll::Ready(Err(std::io::ErrorKind::BrokenPipe.into()))
            }
        }
        impl tokio::io::AsyncWrite for Broken {
            fn poll_write(
                self: Pin<&mut Self>,
                _: &mut std::task::Context<'_>,
                _: &[u8],
            ) -> std::task::Poll<std::io::Result<usize>> {
                std::task::Poll::Ready(Err(std::io::ErrorKind::BrokenPipe.into()))
            }
            fn poll_flush(
                self: Pin<&mut Self>,
                _: &mut std::task::Context<'_>,
            ) -> std::task::Poll<std::io::Result<()>> {
                std::task::Poll::Ready(Ok(()))
            }
            fn poll_shutdown(
                self: Pin<&mut Self>,
                _: &mut std::task::Context<'_>,
            ) -> std::task::Poll<std::io::Result<()>> {
                std::task::Poll::Ready(Ok(()))
            }
        }

        fn ready() -> Ready {
            Ready {
                build: env!("CARGO_PKG_VERSION").into(),
                protocol: protocol::VERSION,
                platform: "linux".into(),
                auth_user: "deploy".into(),
                uid: 1000,
                cwd: "/home/deploy".into(),
                password_limit: 255,
                features: ["one-shot-auth", "binary-streams", "credits", "cancel"]
                    .map(String::from)
                    .to_vec(),
            }
        }
        fn eofs() -> [Frame; 2] {
            [
                Frame::empty(Kind::StdoutEof, 7).unwrap(),
                Frame::empty(Kind::StderrEof, 7).unwrap(),
            ]
        }
        fn result(exit_code: Option<i32>, signal: Option<i32>, delivered: bool) -> Frame {
            Frame::metadata(
                Kind::Result,
                7,
                &ExecutionResult {
                    exit_code,
                    signal,
                    password_delivered: delivered,
                },
            )
            .unwrap()
        }
        fn password_request() -> Frame {
            Frame::metadata(
                Kind::PasswordRequest,
                7,
                &PasswordRequest {
                    auth_user: "deploy".into(),
                },
            )
            .unwrap()
        }
        fn unrequested() -> Resolve {
            Box::new(|_| Box::pin(async { panic!("no password was requested") }))
        }
        fn pending_provider() -> Resolve {
            Box::new(|_| Box::pin(std::future::pending()))
        }

        /// Runs the post-Start session against scripted helper frames. With
        /// `close`, the helper ends its stream cleanly after the script.
        #[derive(Clone, Copy, PartialEq)]
        enum HelperEnd {
            Open,
            Close,
            Disconnect,
        }

        /// Runs the post-Start session against scripted helper frames and
        /// then ends the helper side as requested.
        async fn scripted<I, O>(
            frames: Vec<Frame>,
            end: HelperEnd,
            stdin: I,
            stdout: O,
            resolve: Resolve,
        ) -> Result<RemoteOutcome, AppError>
        where
            I: tokio::io::AsyncRead + Unpin + Send + 'static,
            O: tokio::io::AsyncWrite + Unpin,
        {
            scripted_with_signal(frames, end, stdin, stdout, resolve, 0).await
        }

        async fn scripted_with_signal<I, O>(
            frames: Vec<Frame>,
            end: HelperEnd,
            stdin: I,
            stdout: O,
            resolve: Resolve,
            signal: i32,
        ) -> Result<RemoteOutcome, AppError>
        where
            I: tokio::io::AsyncRead + Unpin + Send + 'static,
            O: tokio::io::AsyncWrite + Unpin,
        {
            let (client, mut helper) = tokio::io::duplex(1 << 20);
            let (client_read, client_write) = tokio::io::split(client);
            for frame in frames {
                protocol::write_frame(&mut helper, &frame).await.unwrap();
            }
            let (mut helper_read, mut helper_write) = tokio::io::split(helper);
            let drain = AbortTask(tokio::spawn(async move {
                let mut sink = Vec::new();
                if end != HelperEnd::Disconnect {
                    let _ = helper_read.read_to_end(&mut sink).await;
                }
            }));
            match end {
                HelperEnd::Open => {}
                HelperEnd::Close => helper_write.shutdown().await.unwrap(),
                HelperEnd::Disconnect => {
                    // Drop both halves: buffered frames stay readable, while
                    // every client write fails.
                    drop(drain);
                    drop(helper_write);
                    tokio::task::yield_now().await;
                    return run_scripted(client_read, client_write, stdin, stdout, resolve, signal)
                        .await;
                }
            }
            let outcome =
                run_scripted(client_read, client_write, stdin, stdout, resolve, signal).await;
            drop((helper_write, drain));
            outcome
        }

        async fn run_scripted<R, W, I, O>(
            reader: R,
            writer: W,
            stdin: I,
            stdout: O,
            resolve: Resolve,
            signal: i32,
        ) -> Result<RemoteOutcome, AppError>
        where
            R: tokio::io::AsyncRead + Unpin + Send + 'static,
            W: tokio::io::AsyncWrite + Unpin + Send + 'static,
            I: tokio::io::AsyncRead + Unpin + Send + 'static,
            O: tokio::io::AsyncWrite + Unpin,
        {
            let (cancel, mut cancellation) = crate::sudo::cancellation_channel();
            if signal > 0 {
                cancel.send(signal).unwrap();
            }
            run_session(
                reader,
                writer,
                7,
                ready(),
                SessionIo {
                    stdin,
                    stdout,
                    stderr: tokio::io::sink(),
                },
                resolve,
                Duration::from_secs(60),
                &mut cancellation,
            )
            .await
        }
        fn completion_unknown(outcome: Result<RemoteOutcome, AppError>) -> bool {
            matches!(outcome, Err(AppError::SudoCompletionUnconfirmed(_)))
        }

        #[tokio::test]
        async fn one_terminal_and_clean_close_is_accepted() {
            let mut output = Vec::new();
            let mut frames = vec![Frame::stream(Kind::StdoutChunk, 7, b"done".to_vec()).unwrap()];
            frames.extend(eofs());
            frames.push(result(Some(3), None, false));
            let outcome = scripted(
                frames,
                HelperEnd::Close,
                tokio::io::empty(),
                &mut output,
                unrequested(),
            )
            .await
            .expect("valid session");
            assert_eq!(outcome.execution.and_then(|e| e.exit_code), Some(3));
            assert!(!outcome.protocol_after_result);
            assert_eq!(output, b"done");
        }

        #[tokio::test]
        async fn buffered_duplicate_result_keeps_status_and_reports_protocol_error() {
            let mut frames = Vec::from(eofs());
            frames.push(result(Some(0), None, false));
            frames.push(result(Some(0), None, false));
            let outcome = scripted(
                frames,
                HelperEnd::Close,
                tokio::io::empty(),
                tokio::io::sink(),
                unrequested(),
            )
            .await
            .expect("the first valid result is evidence");
            assert_eq!(outcome.execution.and_then(|e| e.exit_code), Some(0));
            assert!(outcome.protocol_after_result);
        }

        #[tokio::test]
        async fn queued_result_survives_a_signal_whose_cancel_cannot_be_written() {
            let mut frames = Vec::from(eofs());
            frames.push(result(Some(3), None, false));
            let outcome = scripted_with_signal(
                frames,
                HelperEnd::Disconnect,
                tokio::io::empty(),
                tokio::io::sink(),
                unrequested(),
                2, // protocol SIGINT, on either client platform
            )
            .await
            .expect("the already-sent result is evidence");
            assert_eq!(outcome.execution.and_then(|e| e.exit_code), Some(3));
        }

        #[tokio::test]
        async fn delivery_claim_before_the_response_is_flushed_is_rejected() {
            let mut frames = vec![password_request()];
            frames.extend(eofs());
            frames.push(result(Some(0), None, true));
            // A pending provider must be abandoned, not awaited.
            let outcome = tokio::time::timeout(
                Duration::from_secs(2),
                scripted(
                    frames,
                    HelperEnd::Open,
                    tokio::io::empty(),
                    tokio::io::sink(),
                    pending_provider(),
                ),
            )
            .await
            .expect("an unwritten response is not awaited");
            assert!(completion_unknown(outcome));
        }

        #[tokio::test]
        async fn post_start_failures_before_a_result_are_completion_unknown() {
            let chunk = Frame::stream(Kind::StdoutChunk, 7, b"x".to_vec()).unwrap();
            assert!(completion_unknown(
                scripted(
                    vec![chunk],
                    HelperEnd::Open,
                    tokio::io::empty(),
                    Broken,
                    unrequested()
                )
                .await
            ));
            assert!(completion_unknown(
                scripted(
                    Vec::new(),
                    HelperEnd::Open,
                    Broken,
                    tokio::io::sink(),
                    unrequested()
                )
                .await
            ));
            let mut malformed = Vec::from(eofs());
            malformed.push(result(Some(1), Some(9), false));
            assert!(completion_unknown(
                scripted(
                    malformed,
                    HelperEnd::Open,
                    tokio::io::empty(),
                    tokio::io::sink(),
                    unrequested()
                )
                .await
            ));
            assert!(completion_unknown(
                scripted(
                    vec![password_request(), password_request()],
                    HelperEnd::Open,
                    tokio::io::empty(),
                    tokio::io::sink(),
                    pending_provider(),
                )
                .await
            ));
            assert!(completion_unknown(
                scripted(
                    Vec::new(),
                    HelperEnd::Close,
                    tokio::io::empty(),
                    tokio::io::sink(),
                    unrequested()
                )
                .await
            ));
        }
    }
}
