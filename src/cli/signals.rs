//! Signal-to-cancellation forwarding for commands that own a long-running
//! operation with cleanup obligations: sudo execution and credential filling.

use agentenv::error::AppError;

/// Forwards SIGINT, SIGTERM, and SIGHUP (Ctrl-C elsewhere) into `cancel` as
/// the signal number. The command then cancels its operation and runs its
/// cleanup instead of dying on the default action.
#[cfg(unix)]
pub(super) fn install_signal_forwarder(
    runtime: &tokio::runtime::Runtime,
    cancel: agentenv::cancel::Canceller,
    error: impl Fn() -> AppError,
) -> Result<(), AppError> {
    runtime.block_on(async move {
        let mut interrupt =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())
                .map_err(|_| error())?;
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                .map_err(|_| error())?;
        let mut hangup = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::hangup())
            .map_err(|_| error())?;
        tokio::spawn(async move {
            let signal = tokio::select! {
                _ = interrupt.recv() => libc::SIGINT,
                _ = terminate.recv() => libc::SIGTERM,
                _ = hangup.recv() => libc::SIGHUP,
            };
            cancel.cancel(signal);
        });
        Ok(())
    })
}

/// Install both handlers before starting any operation. Signal numbers belong
/// to the remote POSIX protocol, not Windows console-event enum values.
#[cfg(windows)]
pub(super) fn install_signal_forwarder(
    runtime: &tokio::runtime::Runtime,
    cancel: agentenv::cancel::Canceller,
    error: impl Fn() -> AppError,
) -> Result<(), AppError> {
    runtime.block_on(async move {
        let mut interrupt = tokio::signal::windows::ctrl_c().map_err(|_| error())?;
        let mut stop = tokio::signal::windows::ctrl_break().map_err(|_| error())?;
        tokio::spawn(async move {
            // Both listeners live for the rest of the process. Once none
            // remains, tokio's console handler declines the event and the
            // default handler terminates the process in the middle of
            // cancellation cleanup; a repeated Ctrl+C must stay a no-op.
            let mut forwarded = false;
            loop {
                let signal = tokio::select! {
                    received = interrupt.recv() => received.map(|()| 2),
                    received = stop.recv() => received.map(|()| 15),
                };
                let Some(signal) = signal else {
                    return;
                };
                if !forwarded {
                    forwarded = true;
                    cancel.cancel(signal);
                }
            }
        });
        Ok(())
    })
}
