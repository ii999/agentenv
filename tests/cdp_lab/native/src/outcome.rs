//! The shared outcome document both lab clients emit.

use serde::Serialize;
use serde_json::Value;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Reason {
    ConnectFailed,
    VersionUnsupported,
    ContextAbsent,
    PageAbsent,
    PageAmbiguous,
    FrameAbsent,
    FrameAmbiguous,
    FrameInvalid,
    TargetAbsent,
    TargetAmbiguous,
    TargetHidden,
    TargetDisabled,
    TargetReadonly,
    TargetUnfillable,
    TargetChanged,
    Timeout,
}

/// A failure with a fixed reason and a message that never carries the value.
#[derive(Debug)]
pub struct Failure {
    pub reason: Reason,
    pub message: String,
    pub phase: &'static str,
    pub versions: Versions,
}

impl Failure {
    pub fn new(reason: Reason, phase: &'static str, message: impl Into<String>) -> Self {
        Self {
            reason,
            message: message.into(),
            phase,
            versions: Versions::default(),
        }
    }

    pub fn with_versions(mut self, versions: &Versions) -> Self {
        self.versions = versions.clone();
        self
    }
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct Versions {
    pub browser: Option<String>,
    pub protocol: Option<String>,
    pub library: String,
}

#[derive(Debug, Serialize)]
pub struct Outcome {
    pub version: u32,
    pub implementation: &'static str,
    pub outcome: &'static str,
    pub reason: Option<Reason>,
    pub message: String,
    pub details: Value,
    pub versions: Versions,
}

impl Outcome {
    pub fn filled(details: Value, versions: Versions) -> Self {
        Self {
            version: 1,
            implementation: "native",
            outcome: "filled",
            reason: None,
            message: String::new(),
            details,
            versions,
        }
    }

    pub fn error(failure: Failure) -> Self {
        Self {
            version: 1,
            implementation: "native",
            outcome: "error",
            reason: Some(failure.reason),
            message: failure.message,
            details: serde_json::json!({ "phase": failure.phase }),
            versions: failure.versions,
        }
    }
}
