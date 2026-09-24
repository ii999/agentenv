//! The delivery gate: the coordinator's handle on the resolved value.
//!
//! This module is private so that no backend, even one inside `crate::fill`,
//! can reach the value except through [`Delivery::begin_mutation`].

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;
use std::time::Duration;

use super::{Deadline, FillError, Reason};
use crate::credential::Secret;

/// The delivery phase's view of the operation: the shared deadline, the
/// resolved value, and the point of no return. The value is handed out only
/// by [`Delivery::begin_mutation`], so a backend cannot send it without
/// marking the mutation point; until then, expiry or cancellation means
/// nothing was changed.
pub struct Delivery {
    deadline: Deadline,
    secret: OnceLock<Secret>,
    mutation_started: AtomicBool,
}

impl Delivery {
    pub(super) fn new(deadline: Deadline) -> Self {
        Self {
            deadline,
            secret: OnceLock::new(),
            mutation_started: AtomicBool::new(false),
        }
    }

    /// Stores the resolved value once; a second call is an error.
    pub(super) fn provide(&self, secret: Secret) -> Result<(), Secret> {
        self.secret.set(secret)
    }

    pub fn deadline(&self) -> &Deadline {
        &self.deadline
    }

    pub fn remaining(&self) -> Duration {
        self.deadline.remaining()
    }

    /// Marks the start of mutation and returns the value to send. Fails with
    /// [`Reason::Timeout`] and no mutation when the deadline has already
    /// passed; after `Ok`, every later failure is reported as uncertain.
    pub fn begin_mutation(&self) -> Result<&Secret, FillError> {
        if self.deadline.expired() {
            return Err(FillError::new(
                Reason::Timeout,
                "the deadline expired before insertion; nothing was changed",
            ));
        }
        let secret = self.secret.get().ok_or_else(|| {
            FillError::new(
                Reason::DeliveryFailed,
                "delivery started before the credential was resolved",
            )
        })?;
        self.mutation_started.store(true, Ordering::SeqCst);
        Ok(secret)
    }

    pub fn mutation_started(&self) -> bool {
        self.mutation_started.load(Ordering::SeqCst)
    }
}
