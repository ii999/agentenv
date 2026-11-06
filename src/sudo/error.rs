//! Typed sudo execution failures.
//!
//! Every sudo failure carries a reason code that diagnostics, remediation
//! hints, and the remote helper's failure mapping select on, a detail for the
//! reader, and whether the target command may already have run.

use std::fmt;

use super::deploy;
use crate::error::AppError;

/// Whether the target command may have run when the failure occurred.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Completion {
    /// The command did not start, or its outcome was observed.
    Known,
    /// The command may have run; the caller must not retry automatically.
    Unconfirmed,
}

/// The stable reason code rendered at the start of a sudo diagnostic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    AccountMismatch,
    AccountUnavailable,
    AuthenticationRejected,
    BrokerFailed,
    BrokerInvalid,
    BrokerSetup,
    EffectiveHost,
    EffectivePort,
    EffectiveUser,
    EndpointPinFailed,
    HelloTimeout,
    HelperCheckInvalid,
    HelperHandshakeInvalid,
    HelperHandshakeMissing,
    HelperIdentityMismatch,
    HelperMismatch,
    HelperMissing,
    HelperTimeout,
    InvalidCwd,
    InvalidHelperPath,
    InvalidTarget,
    InvalidTimeout,
    JumpClientUnsupported,
    JumpCycle,
    JumpHost,
    JumpPolicyEvaluation,
    JumpPort,
    JumpRouteInvalid,
    JumpRouteUnsupported,
    JumpTrustIdentity,
    JumpUser,
    OutputWriteFailed,
    PasswordRouteUnsupported,
    PeerRejected,
    Preflight,
    Protocol,
    ProxyCommandUnsupported,
    RemoteOutput,
    RemotePreflightFailed,
    RemoteProtocolFailed,
    RemoteResultMissing,
    ResolverUnavailable,
    RuntimeUnavailable,
    SessionUnavailable,
    SetupTimeout,
    SignalUnavailable,
    SpawnFailed,
    SshAskpassRejected,
    SshClientUnsupported,
    SshCloseFailed,
    SshConfigEvaluation,
    SshConfigInvalid,
    SshConnectFailed,
    SshOptionValidation,
    SshPolicyRejected,
    SshProbeFailed,
    SshProbeOutputLimit,
    SshSetupTimeout,
    SshSpawnFailed,
    SshStreamUnavailable,
    SshUnavailable,
    StartTimeout,
    Stream,
    StreamCreditInvalid,
    SudoUnavailable,
    Transport,
    UnsupportedPlatform,
    /// A helper deployment failure, rendered as `helper-deploy-<code>`.
    Deploy(deploy::Reason),
}

impl Reason {
    fn code(self) -> &'static str {
        match self {
            Self::AccountMismatch => "account-mismatch",
            Self::AccountUnavailable => "account-unavailable",
            Self::AuthenticationRejected => "authentication-rejected",
            Self::BrokerFailed => "broker-failed",
            Self::BrokerInvalid => "broker-invalid",
            Self::BrokerSetup => "broker-setup",
            Self::EffectiveHost => "effective-host",
            Self::EffectivePort => "effective-port",
            Self::EffectiveUser => "effective-user",
            Self::EndpointPinFailed => "endpoint-pin-failed",
            Self::HelloTimeout => "hello-timeout",
            Self::HelperCheckInvalid => "helper-check-invalid",
            Self::HelperHandshakeInvalid => "helper-handshake-invalid",
            Self::HelperHandshakeMissing => "helper-handshake-missing",
            Self::HelperIdentityMismatch => "helper-identity-mismatch",
            Self::HelperMismatch => "helper-mismatch",
            Self::HelperMissing => "helper-missing",
            Self::HelperTimeout => "helper-timeout",
            Self::InvalidCwd => "invalid-cwd",
            Self::InvalidHelperPath => "invalid-helper-path",
            Self::InvalidTarget => "invalid-target",
            Self::InvalidTimeout => "invalid-timeout",
            Self::JumpClientUnsupported => "jump-client-unsupported",
            Self::JumpCycle => "jump-cycle",
            Self::JumpHost => "jump-host",
            Self::JumpPolicyEvaluation => "jump-policy-evaluation",
            Self::JumpPort => "jump-port",
            Self::JumpRouteInvalid => "jump-route-invalid",
            Self::JumpRouteUnsupported => "jump-route-unsupported",
            Self::JumpTrustIdentity => "jump-trust-identity",
            Self::JumpUser => "jump-user",
            Self::OutputWriteFailed => "output-write-failed",
            Self::PasswordRouteUnsupported => "password-route-unsupported",
            Self::PeerRejected => "peer-rejected",
            Self::Preflight => "preflight",
            Self::Protocol => "protocol",
            Self::ProxyCommandUnsupported => "proxy-command-unsupported",
            Self::RemoteOutput => "remote-output",
            Self::RemotePreflightFailed => "remote-preflight-failed",
            Self::RemoteProtocolFailed => "remote-protocol-failed",
            Self::RemoteResultMissing => "remote-result-missing",
            Self::ResolverUnavailable => "resolver-unavailable",
            Self::RuntimeUnavailable => "runtime-unavailable",
            Self::SessionUnavailable => "session-unavailable",
            Self::SetupTimeout => "setup-timeout",
            Self::SignalUnavailable => "signal-unavailable",
            Self::SpawnFailed => "spawn-failed",
            Self::SshAskpassRejected => "ssh-askpass-rejected",
            Self::SshClientUnsupported => "ssh-client-unsupported",
            Self::SshCloseFailed => "ssh-close-failed",
            Self::SshConfigEvaluation => "ssh-config-evaluation",
            Self::SshConfigInvalid => "ssh-config-invalid",
            Self::SshConnectFailed => "ssh-connect-failed",
            Self::SshOptionValidation => "ssh-option-validation",
            Self::SshPolicyRejected => "ssh-policy-rejected",
            Self::SshProbeFailed => "ssh-probe-failed",
            Self::SshProbeOutputLimit => "ssh-probe-output-limit",
            Self::SshSetupTimeout => "ssh-setup-timeout",
            Self::SshSpawnFailed => "ssh-spawn-failed",
            Self::SshStreamUnavailable => "ssh-stream-unavailable",
            Self::SshUnavailable => "ssh-unavailable",
            Self::StartTimeout => "start-timeout",
            Self::Stream => "stream",
            Self::StreamCreditInvalid => "stream-credit-invalid",
            Self::SudoUnavailable => "sudo-unavailable",
            Self::Transport => "transport",
            Self::UnsupportedPlatform => "unsupported-platform",
            Self::Deploy(reason) => reason.code(),
        }
    }
}

impl fmt::Display for Reason {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Self::Deploy(_) = self {
            formatter.write_str("helper-deploy-")?;
        }
        formatter.write_str(self.code())
    }
}

/// One sudo failure: `[completion-unconfirmed: ][<reason>: ]<detail>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SudoError {
    reason: Option<Reason>,
    detail: String,
    completion: Completion,
}

impl SudoError {
    /// A failure before the command could run, or with its outcome known.
    pub fn new(reason: Reason, detail: impl Into<String>) -> Self {
        Self {
            reason: Some(reason),
            detail: detail.into(),
            completion: Completion::Known,
        }
    }

    /// A failure after which the command may have run.
    pub fn unconfirmed(reason: Reason, detail: impl Into<String>) -> Self {
        Self {
            reason: Some(reason),
            detail: detail.into(),
            completion: Completion::Unconfirmed,
        }
    }

    /// A local engine that could not observe or end the sudo process; the
    /// detail names what failed and no further reason code applies.
    pub fn unobserved(detail: impl Into<String>) -> Self {
        Self {
            reason: None,
            detail: detail.into(),
            completion: Completion::Unconfirmed,
        }
    }

    pub fn reason(&self) -> Option<Reason> {
        self.reason
    }

    pub fn completion(&self) -> Completion {
        self.completion
    }

    /// Appends `text` to the detail, for remediation and follow-up notes.
    pub fn append(mut self, text: &str) -> Self {
        self.detail.push_str(text);
        self
    }
}

impl fmt::Display for SudoError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.completion == Completion::Unconfirmed {
            formatter.write_str("completion-unconfirmed: ")?;
        }
        if let Some(reason) = self.reason {
            write!(formatter, "{reason}: ")?;
        }
        formatter.write_str(&self.detail)
    }
}

impl From<SudoError> for AppError {
    fn from(error: SudoError) -> Self {
        AppError::Sudo(error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diagnostics_render_reason_detail_and_completion() {
        let known: AppError = SudoError::new(Reason::SetupTimeout, "the deadline expired").into();
        assert_eq!(
            known.to_string(),
            "sudo-execution: setup-timeout: the deadline expired"
        );
        assert_eq!(known.exit_code(), 9);

        let unconfirmed: AppError =
            SudoError::unconfirmed(Reason::RemoteResultMissing, "the command may have run").into();
        assert_eq!(
            unconfirmed.to_string(),
            "sudo-execution: completion-unconfirmed: remote-result-missing: the command may have run"
        );
        assert_eq!(unconfirmed.exit_code(), 10);

        let unobserved: AppError = SudoError::unobserved("could not reap sudo").into();
        assert_eq!(
            unobserved.to_string(),
            "sudo-execution: completion-unconfirmed: could not reap sudo"
        );

        let deploy = deploy::Reason::Cancelled.error("stopped");
        assert!(deploy
            .to_string()
            .starts_with("sudo-execution: helper-deploy-cancelled: stopped; helper_path holds"));
    }
}
