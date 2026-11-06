//! Cancellation requested by a forwarded signal.
//!
//! A [`Canceller`] records the signal number that asked an operation to stop,
//! and every clone of the paired [`Cancellation`] observes it. Signal numbers
//! are positive and use POSIX numbering on every platform, because the remote
//! sudo protocol forwards them.

use tokio::sync::watch;

/// Requests cancellation with a signal number.
#[derive(Debug)]
pub struct Canceller(watch::Sender<i32>);

/// Observes whether cancellation was requested, and with which signal.
#[derive(Debug, Clone)]
pub struct Cancellation(watch::Receiver<i32>);

/// A connected canceller and cancellation with no request yet.
pub fn channel() -> (Canceller, Cancellation) {
    let (sender, receiver) = watch::channel(0);
    (Canceller(sender), Cancellation(receiver))
}

impl Canceller {
    /// Requests cancellation with `signal`, whether or not anything still
    /// observes it.
    pub fn cancel(&self, signal: i32) {
        self.0.send_replace(signal);
    }
}

impl Cancellation {
    /// A cancellation that is never requested.
    pub fn never() -> Self {
        channel().1
    }

    /// The requesting signal, once cancellation has been requested.
    pub fn pending(&self) -> Option<i32> {
        let signal = *self.0.borrow();
        (signal > 0).then_some(signal)
    }

    /// Waits until cancellation is requested and returns its signal. When the
    /// canceller is dropped without a request, this never completes. The
    /// future is cancel-safe.
    pub async fn requested(&mut self) -> i32 {
        loop {
            if let Some(signal) = self.pending() {
                return signal;
            }
            if self.0.changed().await.is_err() {
                std::future::pending::<()>().await;
            }
        }
    }
}
