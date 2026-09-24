//! Credential filling: deliver a credential value into a destination without
//! exposing it to the caller.
//!
//! The coordinator owns the operation: one monotonic deadline that starts
//! before preflight and covers backend preparation, credential resolution,
//! and delivery; a separate short cleanup grace period; cancellation; and the
//! fixed result vocabulary. Backends own transport and targeting behind
//! [`Backend`]. Credential storage stays behind the provider seam in
//! `credential`.
//!
//! Ordering is fixed: the backend prepares and validates its target before
//! any credential is resolved, so a bad target never costs a secret lookup.
//! Env credentials resolve in-process; keychain and command credentials go
//! through the confidential resolver's fill stage, whose future is dropped on
//! expiry or cancellation so a late reply can never reach delivery.

pub mod cdp;
mod delivery;
#[cfg(windows)]
pub mod desktop;
#[cfg(all(feature = "test-keychain", debug_assertions))]
pub mod test_backend;

pub use delivery::Delivery;

use std::future::Future;
use std::path::Path;
use std::time::{Duration, Instant};

use serde::Serialize;

use crate::config::{CredentialDef, CredentialUsage, Provider};
use crate::credential::resolver::{self, ResolutionStage, ResolveError};
use crate::credential::{provider_for, Secret, FILL_VALUE_LIMIT};
use crate::error::{AppError, Violation};

/// Default whole-operation budget. Leaves room for a keychain authorization
/// dialog on first access.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);
/// Largest accepted `--timeout-ms`.
pub const MAX_TIMEOUT: Duration = Duration::from_secs(300);
/// Time allowed for detaching and reaping after the operation closes.
pub const CLEANUP_GRACE: Duration = Duration::from_secs(2);

/// A monotonic operation deadline. Every phase receives the remaining budget;
/// nothing resets it.
#[derive(Clone, Copy, Debug)]
pub struct Deadline {
    started: Instant,
    budget: Duration,
}

impl Deadline {
    pub fn start(budget: Duration) -> Self {
        Self {
            started: Instant::now(),
            budget,
        }
    }

    pub fn remaining(&self) -> Duration {
        self.budget.saturating_sub(self.started.elapsed())
    }

    pub fn expired(&self) -> bool {
        self.remaining().is_zero()
    }
}

/// Fixed failure classification. Codes are stable and documented; messages
/// are constructed, never derived from transport or provider text that could
/// carry a value.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Reason {
    BackendUnavailable,
    Permission,
    ConnectFailed,
    VersionUnsupported,
    RecordingConflict,
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
    ValueUnsupported,
    Timeout,
    Cancelled,
    DeliveryFailed,
    CleanupUnconfirmed,
}

impl Reason {
    pub fn code(self) -> &'static str {
        match self {
            Self::BackendUnavailable => "backend-unavailable",
            Self::Permission => "permission",
            Self::ConnectFailed => "connect-failed",
            Self::VersionUnsupported => "version-unsupported",
            Self::RecordingConflict => "recording-conflict",
            Self::ContextAbsent => "context-absent",
            Self::PageAbsent => "page-absent",
            Self::PageAmbiguous => "page-ambiguous",
            Self::FrameAbsent => "frame-absent",
            Self::FrameAmbiguous => "frame-ambiguous",
            Self::FrameInvalid => "frame-invalid",
            Self::TargetAbsent => "target-absent",
            Self::TargetAmbiguous => "target-ambiguous",
            Self::TargetHidden => "target-hidden",
            Self::TargetDisabled => "target-disabled",
            Self::TargetReadonly => "target-readonly",
            Self::TargetUnfillable => "target-unfillable",
            Self::TargetChanged => "target-changed",
            Self::ValueUnsupported => "value-unsupported",
            Self::Timeout => "timeout",
            Self::Cancelled => "cancelled",
            Self::DeliveryFailed => "delivery-failed",
            Self::CleanupUnconfirmed => "cleanup-unconfirmed",
        }
    }
}

/// Whether the destination may have changed when the failure was observed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mutation {
    /// Nothing was sent to the destination.
    None,
    /// Delivery started, or its result could not be confirmed.
    Possible,
}

/// A filling failure with a fixed reason and a safe message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FillError {
    pub reason: Reason,
    pub message: String,
    pub mutation: Mutation,
}

impl FillError {
    pub fn new(reason: Reason, message: impl Into<String>) -> Self {
        Self {
            reason,
            message: message.into(),
            mutation: Mutation::None,
        }
    }

    pub fn uncertain(reason: Reason, message: impl Into<String>) -> Self {
        Self {
            reason,
            message: message.into(),
            mutation: Mutation::Possible,
        }
    }
}

impl From<FillError> for AppError {
    fn from(error: FillError) -> Self {
        let text = format!("{}: {}", error.reason.code(), error.message);
        match error.mutation {
            Mutation::None => AppError::Fill(text),
            Mutation::Possible => AppError::FillUncertain(format!(
                "{text}; the destination may have changed, inspect it before retrying"
            )),
        }
    }
}

/// What a successful delivery established. Neither effect claims readback
/// verification, persistence, submission, or login success.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Effect {
    /// The destination's content was replaced through an API that reports
    /// success.
    FieldFilled,
    /// Input events were delivered to a focused control.
    InputSent,
}

/// The only data a successful fill returns.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Outcome {
    pub version: u32,
    pub backend: &'static str,
    pub effect: Effect,
}

/// How a requested page URL is compared with open pages.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PageMatch {
    /// The whole URL, byte for byte.
    Exact,
    /// Scheme, host, port, and path; query and fragment are ignored.
    OriginPath,
}

/// A browser filling destination as named on the command line. Carries no
/// credential and no connection authentication.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BrowserTarget {
    pub endpoint: String,
    pub page_url: String,
    pub page_match: PageMatch,
    pub context_index: Option<usize>,
    pub frame_selectors: Vec<String>,
    pub selector: String,
}

/// A filling destination. Implementations own transport and targeting.
///
/// `prepare` runs before any credential exists and must fail on every
/// condition it can detect. `deliver` revalidates the prepared target and
/// mutates it at most once; the value is available only from
/// [`Delivery::begin_mutation`], called immediately before the single
/// value-bearing send, which reports expiry without mutation. Any error the
/// backend returns after that call is reported as uncertain whether or not
/// it says so. Both futures must be cancel-safe: dropping them
/// stops every further send, so no detached task may carry a value-bearing
/// message. `release` detaches and reaps whatever the backend owns without
/// touching the destination; the coordinator calls it after the operation
/// future has been dropped.
pub trait Backend {
    fn name(&self) -> &'static str;
    fn prepare(
        &mut self,
        deadline: &Deadline,
    ) -> impl Future<Output = Result<(), FillError>> + Send;
    fn deliver(
        &mut self,
        delivery: &Delivery,
    ) -> impl Future<Output = Result<Effect, FillError>> + Send;
    fn release(&mut self, grace: &Deadline) -> impl Future<Output = Result<(), FillError>> + Send;
}

/// Rejects a definition that may not be filled. Authentication credentials
/// never reach a fill destination; this is a configuration error, reported
/// before any resolution.
pub fn check_fillable(definition: &CredentialDef) -> Result<(), AppError> {
    if definition.permits(CredentialUsage::Environment) {
        return Ok(());
    }
    Err(AppError::Config(vec![Violation {
        path: format!("credentials.{}.usages", definition.name),
        message: format!(
            "credential '{}' permits only authentication use ({}) and cannot be filled; use a credential with environment usage",
            definition.name,
            definition
                .usages
                .iter()
                .map(|usage| usage.token())
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }]))
}

/// Whether this platform can resolve keychain and command credentials for
/// filling. Env credentials are always resolvable in-process.
pub fn resolver_available(definition: &CredentialDef) -> bool {
    matches!(definition.provider, Provider::Env { .. }) || cfg!(any(unix, windows))
}

/// Runs one filling operation end to end.
///
/// `executable` is the agentenv binary used for the confidential resolver.
/// `cancel` resolves when the caller wants the operation stopped, for
/// example on a signal; the operation future is dropped, which kills and reaps
/// an owned resolver, and the backend is still released. Returns the success
/// outcome, a configuration or credential error, or a [`FillError`] mapped to
/// exit 8 or 11.
pub async fn run<B: Backend>(
    backend: &mut B,
    definition: &CredentialDef,
    executable: &Path,
    timeout: Duration,
    cancel: impl Future<Output = ()>,
) -> Result<Outcome, AppError> {
    check_fillable(definition)?;
    if !resolver_available(definition) {
        return Err(FillError::new(
            Reason::BackendUnavailable,
            format!(
                "credential '{}' needs the confidential resolver, which is unavailable on this platform; use an env credential",
                definition.name
            ),
        )
        .into());
    }
    let delivery = Delivery::new(Deadline::start(timeout.min(MAX_TIMEOUT)));
    let result = {
        let operation = operate(backend, definition, executable, &delivery);
        tokio::pin!(operation);
        tokio::pin!(cancel);
        tokio::select! {
            biased;
            () = &mut cancel => Err(cancelled(&delivery)),
            result = &mut operation => result,
        }
    };
    // The value is not needed for cleanup; release it before the grace
    // period rather than at the end of the operation.
    drop(delivery);
    let grace = Deadline::start(CLEANUP_GRACE);
    let released = tokio::time::timeout(grace.remaining(), backend.release(&grace)).await;
    let cleanup_failure = match released {
        Ok(Ok(())) => None,
        Ok(Err(error)) => Some(error.message),
        Err(_) => Some("cleanup did not finish within the grace period".to_owned()),
    };
    match (result, cleanup_failure) {
        (Ok(outcome), None) => Ok(outcome),
        (Ok(_), Some(detail)) => Err(FillError::uncertain(
            Reason::CleanupUnconfirmed,
            format!(
                "the value was delivered but owned resources were not confirmed released: {detail}"
            ),
        )
        .into()),
        (Err(error), None) => Err(error),
        (Err(error), Some(detail)) => Err(match error {
            AppError::Fill(text) => {
                AppError::Fill(format!("{text}; cleanup unconfirmed: {detail}"))
            }
            AppError::FillUncertain(text) => {
                AppError::FillUncertain(format!("{text}; cleanup unconfirmed: {detail}"))
            }
            AppError::Credential(text) => {
                AppError::Credential(format!("{text}; cleanup unconfirmed: {detail}"))
            }
            // Only the three variants above can arise after preflight:
            // usage, configuration, and credential-name errors are raised
            // before `prepare`.
            other => other,
        }),
    }
}

fn cancelled(delivery: &Delivery) -> AppError {
    if delivery.mutation_started() {
        FillError::uncertain(
            Reason::Cancelled,
            "the operation was cancelled during delivery",
        )
        .into()
    } else {
        FillError::new(
            Reason::Cancelled,
            "the operation was cancelled before delivery; nothing was changed",
        )
        .into()
    }
}

async fn operate<B: Backend>(
    backend: &mut B,
    definition: &CredentialDef,
    executable: &Path,
    delivery: &Delivery,
) -> Result<Outcome, AppError> {
    let backend_name = backend.name();
    let deadline = delivery.deadline();

    tokio::time::timeout(deadline.remaining(), backend.prepare(deadline))
        .await
        .map_err(|_| {
            FillError::new(
                Reason::Timeout,
                "the deadline expired while preparing the destination; nothing was changed",
            )
        })??;

    let secret = resolve_value(definition, executable, deadline).await?;
    if delivery.provide(secret).is_err() {
        return Err(FillError::new(
            Reason::DeliveryFailed,
            "the credential was resolved twice in one operation",
        )
        .into());
    }

    let remaining = deadline.remaining();
    if remaining.is_zero() {
        return Err(FillError::new(
            Reason::Timeout,
            "the deadline expired before delivery; nothing was changed",
        )
        .into());
    }
    let effect = match tokio::time::timeout(remaining, backend.deliver(delivery)).await {
        Err(_) if delivery.mutation_started() => Err(FillError::uncertain(
            Reason::Timeout,
            "the deadline expired during delivery",
        )),
        Err(_) => Err(FillError::new(
            Reason::Timeout,
            "the deadline expired before insertion; nothing was changed",
        )),
        // Once the value has been handed out, no failure can claim that
        // nothing changed.
        Ok(Err(error)) if delivery.mutation_started() => Err(FillError {
            mutation: Mutation::Possible,
            ..error
        }),
        // A success that never took the value sent nothing; report it
        // rather than claim a fill.
        Ok(Ok(_)) if !delivery.mutation_started() => Err(FillError::new(
            Reason::DeliveryFailed,
            "the backend reported success without sending the value",
        )),
        Ok(result) => result,
    }?;
    Ok(Outcome {
        version: 1,
        backend: backend_name,
        effect,
    })
}

async fn resolve_value(
    definition: &CredentialDef,
    executable: &Path,
    deadline: &Deadline,
) -> Result<Secret, AppError> {
    let unsupported = || {
        FillError::new(
            Reason::ValueUnsupported,
            format!(
                "credential '{}' is not a single line of at most {FILL_VALUE_LIMIT} bytes without control, line-separator, or bidirectional-control characters; store a value the destination can accept",
                definition.name
            ),
        )
    };
    if let Provider::Env { .. } = definition.provider {
        let secret = provider_for(definition).resolve()?;
        secret
            .validate_fill(FILL_VALUE_LIMIT)
            .map_err(|_| unsupported())?;
        return Ok(secret);
    }
    let remaining = deadline.remaining();
    if remaining.is_zero() {
        return Err(FillError::new(
            Reason::Timeout,
            "the deadline expired before credential lookup; nothing was changed",
        )
        .into());
    }
    match resolver::resolve_detailed(
        executable,
        definition,
        ResolutionStage::Fill,
        FILL_VALUE_LIMIT,
        remaining,
    )
    .await
    {
        Ok(secret) => Ok(secret),
        Err(ResolveError::Value) => Err(unsupported().into()),
        Err(ResolveError::Timeout) => Err(FillError::new(
            Reason::Timeout,
            "the deadline expired during credential lookup; nothing was changed",
        )
        .into()),
        Err(ResolveError::NotPermitted) => Err(check_fillable(definition).err().unwrap_or_else(|| {
            AppError::Credential(format!(
                "credential '{}' cannot be resolved for filling; check its provider and usages",
                definition.name
            ))
        })),
        Err(ResolveError::Provider) => Err(AppError::Credential(format!(
            "credential '{}' could not be resolved; run 'agentenv credential check {}' to diagnose the provider",
            definition.name, definition.name
        ))),
        Err(ResolveError::Unavailable) => Err(FillError::new(
            Reason::BackendUnavailable,
            format!(
                "credential '{}' needs the confidential resolver, which is unavailable on this platform; use an env credential",
                definition.name
            ),
        )
        .into()),
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex};

    use super::*;

    #[derive(Clone, Copy)]
    enum Script {
        Fill,
        PrepareFail,
        PrepareHang,
        RevalidateHang,
        DeliverHang,
        DeliverFailUncertain,
        ReleaseFail,
    }

    struct Scripted {
        script: Script,
        delivered: Arc<Mutex<Option<String>>>,
        prepared: bool,
        released: bool,
    }

    impl Backend for Scripted {
        fn name(&self) -> &'static str {
            "scripted"
        }

        async fn prepare(&mut self, _deadline: &Deadline) -> Result<(), FillError> {
            match self.script {
                Script::PrepareFail => Err(FillError::new(Reason::TargetAbsent, "no target")),
                Script::PrepareHang => {
                    tokio::time::sleep(Duration::from_secs(60)).await;
                    Ok(())
                }
                _ => {
                    self.prepared = true;
                    Ok(())
                }
            }
        }

        async fn deliver(&mut self, delivery: &Delivery) -> Result<Effect, FillError> {
            assert!(self.prepared, "deliver before prepare");
            match self.script {
                Script::RevalidateHang => {
                    tokio::time::sleep(Duration::from_secs(60)).await;
                    Ok(Effect::FieldFilled)
                }
                Script::DeliverHang => {
                    delivery.begin_mutation()?;
                    tokio::time::sleep(Duration::from_secs(60)).await;
                    Ok(Effect::FieldFilled)
                }
                Script::DeliverFailUncertain => {
                    delivery.begin_mutation()?;
                    // Deliberately not marked uncertain: the coordinator
                    // must upgrade it because the value was handed out.
                    Err(FillError::new(
                        Reason::DeliveryFailed,
                        "input was interrupted",
                    ))
                }
                _ => {
                    let value = delivery.begin_mutation()?;
                    *self.delivered.lock().unwrap() = Some(value.as_str().to_owned());
                    Ok(Effect::FieldFilled)
                }
            }
        }

        async fn release(&mut self, _grace: &Deadline) -> Result<(), FillError> {
            self.released = true;
            match self.script {
                Script::ReleaseFail => {
                    Err(FillError::new(Reason::CleanupUnconfirmed, "detach failed"))
                }
                _ => Ok(()),
            }
        }
    }

    fn env_definition(variable: &str) -> CredentialDef {
        CredentialDef {
            name: "token".to_owned(),
            description: String::new(),
            provider: Provider::Env {
                name: variable.to_owned(),
            },
            inject_as: Some("TOKEN".to_owned()),
            usages: vec![CredentialUsage::Environment],
        }
    }

    fn backend(script: Script) -> (Scripted, Arc<Mutex<Option<String>>>) {
        let delivered = Arc::new(Mutex::new(None));
        (
            Scripted {
                script,
                delivered: delivered.clone(),
                prepared: false,
                released: false,
            },
            delivered,
        )
    }

    fn exe() -> PathBuf {
        PathBuf::from("/nonexistent/agentenv")
    }

    async fn run_plain(
        backend: &mut Scripted,
        definition: &CredentialDef,
        timeout: Duration,
    ) -> Result<Outcome, AppError> {
        run(backend, definition, &exe(), timeout, std::future::pending()).await
    }

    #[tokio::test]
    async fn fills_env_credential_and_reports_effect_only() {
        let variable = "AGENTENV_FILL_TEST_OK";
        std::env::set_var(variable, "  correct horse 🔑  ");
        let (mut scripted, delivered) = backend(Script::Fill);
        let outcome = run_plain(&mut scripted, &env_definition(variable), DEFAULT_TIMEOUT)
            .await
            .unwrap();
        assert_eq!(
            outcome,
            Outcome {
                version: 1,
                backend: "scripted",
                effect: Effect::FieldFilled
            }
        );
        assert_eq!(
            delivered.lock().unwrap().as_deref(),
            Some("  correct horse 🔑  ")
        );
        assert!(scripted.released);
        assert_eq!(
            serde_json::to_string(&outcome).unwrap(),
            r#"{"version":1,"backend":"scripted","effect":"field-filled"}"#
        );
    }

    #[tokio::test]
    async fn authentication_credentials_are_rejected_before_preflight() {
        let mut definition = env_definition("AGENTENV_FILL_TEST_AUTH");
        definition.usages = vec![CredentialUsage::Sudo];
        let (mut scripted, _) = backend(Script::Fill);
        let error = run_plain(&mut scripted, &definition, DEFAULT_TIMEOUT)
            .await
            .unwrap_err();
        assert_eq!(error.exit_code(), 2);
        assert!(!scripted.prepared);
    }

    #[tokio::test]
    async fn preflight_failure_costs_no_lookup_and_exits_8() {
        let variable = "AGENTENV_FILL_TEST_UNSET_FOR_PREFLIGHT";
        std::env::remove_var(variable);
        let (mut scripted, _) = backend(Script::PrepareFail);
        let error = run_plain(&mut scripted, &env_definition(variable), DEFAULT_TIMEOUT)
            .await
            .unwrap_err();
        assert_eq!(error.exit_code(), 8);
        assert!(error
            .to_string()
            .starts_with("credential-fill: target-absent:"));
    }

    #[tokio::test]
    async fn multi_line_values_are_rejected_before_delivery() {
        let variable = "AGENTENV_FILL_TEST_MULTILINE";
        std::env::set_var(variable, "line one\nline two");
        let (mut scripted, delivered) = backend(Script::Fill);
        let error = run_plain(&mut scripted, &env_definition(variable), DEFAULT_TIMEOUT)
            .await
            .unwrap_err();
        assert_eq!(error.exit_code(), 8);
        assert!(error.to_string().contains("value-unsupported"));
        assert!(!error.to_string().contains("line one"));
        assert!(delivered.lock().unwrap().is_none());
    }

    #[tokio::test]
    async fn expiry_before_delivery_is_code_8_and_during_delivery_is_code_11() {
        let variable = "AGENTENV_FILL_TEST_TIMEOUTS";
        std::env::set_var(variable, "value");
        let (mut hanging_prepare, delivered) = backend(Script::PrepareHang);
        let error = run_plain(
            &mut hanging_prepare,
            &env_definition(variable),
            Duration::from_millis(50),
        )
        .await
        .unwrap_err();
        assert_eq!(error.exit_code(), 8);
        assert!(error.to_string().contains("timeout"));
        assert!(delivered.lock().unwrap().is_none());
        assert!(hanging_prepare.released);

        let (mut hanging_revalidate, _) = backend(Script::RevalidateHang);
        let error = run_plain(
            &mut hanging_revalidate,
            &env_definition(variable),
            Duration::from_millis(50),
        )
        .await
        .unwrap_err();
        assert_eq!(
            error.exit_code(),
            8,
            "expiry before the mutation gate is not uncertain"
        );
        assert!(error.to_string().contains("nothing was changed"));
        assert!(hanging_revalidate.released);

        let (mut hanging_deliver, _) = backend(Script::DeliverHang);
        let error = run_plain(
            &mut hanging_deliver,
            &env_definition(variable),
            Duration::from_millis(50),
        )
        .await
        .unwrap_err();
        assert_eq!(error.exit_code(), 11);
        assert!(error.to_string().contains("may have changed"));
        assert!(hanging_deliver.released);
    }

    #[tokio::test]
    async fn cancellation_releases_and_distinguishes_phases() {
        let variable = "AGENTENV_FILL_TEST_CANCEL";
        std::env::set_var(variable, "value");
        let (mut hanging_prepare, delivered) = backend(Script::PrepareHang);
        let error = run(
            &mut hanging_prepare,
            &env_definition(variable),
            &exe(),
            DEFAULT_TIMEOUT,
            tokio::time::sleep(Duration::from_millis(30)),
        )
        .await
        .unwrap_err();
        assert_eq!(error.exit_code(), 8);
        assert!(error.to_string().starts_with("credential-fill: cancelled:"));
        assert!(delivered.lock().unwrap().is_none());
        assert!(hanging_prepare.released);

        let (mut hanging_revalidate, _) = backend(Script::RevalidateHang);
        let error = run(
            &mut hanging_revalidate,
            &env_definition(variable),
            &exe(),
            DEFAULT_TIMEOUT,
            tokio::time::sleep(Duration::from_millis(30)),
        )
        .await
        .unwrap_err();
        assert_eq!(
            error.exit_code(),
            8,
            "cancellation before the mutation gate is not uncertain"
        );
        assert!(error.to_string().starts_with("credential-fill: cancelled:"));
        assert!(hanging_revalidate.released);

        let (mut hanging_deliver, _) = backend(Script::DeliverHang);
        let error = run(
            &mut hanging_deliver,
            &env_definition(variable),
            &exe(),
            DEFAULT_TIMEOUT,
            tokio::time::sleep(Duration::from_millis(30)),
        )
        .await
        .unwrap_err();
        assert_eq!(error.exit_code(), 11);
        assert!(error.to_string().starts_with("credential-fill: cancelled:"));
        assert!(hanging_deliver.released);
    }

    #[tokio::test]
    async fn uncertain_delivery_and_unconfirmed_cleanup_are_code_11() {
        let variable = "AGENTENV_FILL_TEST_UNCERTAIN";
        std::env::set_var(variable, "value");
        let (mut failing, _) = backend(Script::DeliverFailUncertain);
        let error = run_plain(&mut failing, &env_definition(variable), DEFAULT_TIMEOUT)
            .await
            .unwrap_err();
        assert_eq!(error.exit_code(), 11);
        assert!(error
            .to_string()
            .starts_with("credential-fill: delivery-failed:"));

        let (mut leaking, delivered) = backend(Script::ReleaseFail);
        let error = run_plain(&mut leaking, &env_definition(variable), DEFAULT_TIMEOUT)
            .await
            .unwrap_err();
        assert_eq!(error.exit_code(), 11);
        assert!(error.to_string().contains("cleanup-unconfirmed"));
        assert_eq!(delivered.lock().unwrap().as_deref(), Some("value"));
    }

    #[tokio::test]
    async fn cleanup_failure_is_reported_alongside_a_credential_error() {
        let variable = "AGENTENV_FILL_TEST_UNSET_WITH_RELEASE_FAIL";
        std::env::remove_var(variable);
        let (mut leaking, _) = backend(Script::ReleaseFail);
        let error = run_plain(&mut leaking, &env_definition(variable), DEFAULT_TIMEOUT)
            .await
            .unwrap_err();
        assert_eq!(error.exit_code(), 4);
        assert!(error.to_string().contains("cleanup unconfirmed"));
    }
}
