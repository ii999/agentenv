//! A scripted destination for integration tests of the fill command.
//!
//! Compiled only with the `test-keychain` feature in debug builds, like the
//! test credential store. The behavior is selected by
//! `AGENTENV_FILL_TEST_BEHAVIOR`; a delivered value is written to the file
//! named by `AGENTENV_FILL_TEST_SINK` so a test can inspect what its own
//! fixture received without the product ever reading it back.

use std::path::PathBuf;
use std::time::Duration;

use super::{Backend, Deadline, Delivery, Effect, FillError, Reason};

pub const BEHAVIOR_ENV: &str = "AGENTENV_FILL_TEST_BEHAVIOR";
pub const SINK_ENV: &str = "AGENTENV_FILL_TEST_SINK";

#[derive(Clone, Debug, PartialEq, Eq)]
enum Behavior {
    Filled,
    InputSent,
    PrepareFail(Reason),
    PrepareHang,
    RevalidateHang,
    DeliverHang,
    DeliverFail,
    DeliverFailUncertain,
    ReleaseFail,
}

pub struct TestBackend {
    behavior: Behavior,
    sink: Option<PathBuf>,
}

impl TestBackend {
    /// Returns the configured backend, or `None` when the environment does not
    /// select one.
    pub fn from_environment() -> Option<Self> {
        let behavior = std::env::var(BEHAVIOR_ENV).ok()?;
        let behavior = match behavior.as_str() {
            "filled" => Behavior::Filled,
            "input-sent" => Behavior::InputSent,
            "prepare-hang" => Behavior::PrepareHang,
            "revalidate-hang" => Behavior::RevalidateHang,
            "deliver-hang" => Behavior::DeliverHang,
            "deliver-fail" => Behavior::DeliverFail,
            "deliver-fail-uncertain" => Behavior::DeliverFailUncertain,
            "release-fail" => Behavior::ReleaseFail,
            other => match other.strip_prefix("prepare-fail:") {
                Some("target-absent") => Behavior::PrepareFail(Reason::TargetAbsent),
                Some("recording-conflict") => Behavior::PrepareFail(Reason::RecordingConflict),
                Some("permission") => Behavior::PrepareFail(Reason::Permission),
                _ => return None,
            },
        };
        Some(Self {
            behavior,
            sink: std::env::var_os(SINK_ENV).map(PathBuf::from),
        })
    }
}

impl Backend for TestBackend {
    fn name(&self) -> &'static str {
        "test"
    }

    async fn prepare(&mut self, _deadline: &Deadline) -> Result<(), FillError> {
        match &self.behavior {
            Behavior::PrepareFail(reason) => {
                Err(FillError::new(*reason, "scripted preflight failure"))
            }
            Behavior::PrepareHang => {
                tokio::time::sleep(Duration::from_secs(600)).await;
                Ok(())
            }
            _ => Ok(()),
        }
    }

    async fn deliver(&mut self, delivery: &Delivery) -> Result<Effect, FillError> {
        match &self.behavior {
            Behavior::RevalidateHang => {
                tokio::time::sleep(Duration::from_secs(600)).await;
                Ok(Effect::FieldFilled)
            }
            Behavior::DeliverHang => {
                delivery.begin_mutation()?;
                tokio::time::sleep(Duration::from_secs(600)).await;
                Ok(Effect::FieldFilled)
            }
            Behavior::DeliverFail => Err(FillError::new(
                Reason::TargetChanged,
                "scripted target change",
            )),
            Behavior::DeliverFailUncertain => {
                delivery.begin_mutation()?;
                Err(FillError::uncertain(
                    Reason::DeliveryFailed,
                    "scripted interruption after delivery started",
                ))
            }
            _ => {
                let value = delivery.begin_mutation()?;
                if let Some(sink) = &self.sink {
                    std::fs::write(sink, value.as_str().as_bytes()).map_err(|_| {
                        FillError::uncertain(
                            Reason::DeliveryFailed,
                            "the test sink could not be written",
                        )
                    })?;
                }
                Ok(if self.behavior == Behavior::InputSent {
                    Effect::InputSent
                } else {
                    Effect::FieldFilled
                })
            }
        }
    }

    async fn release(&mut self, _grace: &Deadline) -> Result<(), FillError> {
        match self.behavior {
            Behavior::ReleaseFail => Err(FillError::new(
                Reason::CleanupUnconfirmed,
                "scripted detach failure",
            )),
            _ => Ok(()),
        }
    }
}
