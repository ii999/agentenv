//! Signal-to-cancellation forwarding for commands that own a long-running
//! operation with cleanup obligations: sudo execution and credential filling.

use agentenv::error::AppError;

/// Forwards SIGINT, SIGTERM, and SIGHUP (Ctrl-C elsewhere) into `cancel` as
/// the signal number. The command then cancels its operation and runs its
/// cleanup instead of dying on the default action.
#[cfg(unix)]
pub(super) fn install_signal_forwarder(
    runtime: &tokio::runtime::Runtime,
    cancel: tokio::sync::watch::Sender<i32>,
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
            let _ = cancel.send(signal);
        });
        Ok(())
    })
}

#[cfg(not(unix))]
pub(super) fn install_signal_forwarder(
    runtime: &tokio::runtime::Runtime,
    cancel: tokio::sync::watch::Sender<i32>,
    _error: impl Fn() -> AppError,
) -> Result<(), AppError> {
    runtime.spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            let _ = cancel.send(2);
        }
    });
    Ok(())
}
