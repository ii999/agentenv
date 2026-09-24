//! `credential fill`: deliver a credential into a browser or desktop
//! destination, and `credential fill --capabilities`.

use std::time::Duration;

use agentenv::config::{Config, CredentialDef};
use agentenv::error::AppError;
use agentenv::fill::cdp::CdpBackend;
use agentenv::fill::{self, Backend, BrowserTarget, FillError, Outcome, PageMatch, Reason};
use clap::{Args, ValueEnum};

use super::credential::credential_for;
use super::Output;

#[derive(Debug, Args)]
pub struct CredentialFillArgs {
    /// Credential to fill. Omitted only with --capabilities.
    pub name: Option<String>,
    /// Report compiled backends and runtime availability; resolves nothing.
    #[arg(long)]
    pub capabilities: bool,
    /// Destination backend.
    #[arg(long, value_enum, value_name = "BACKEND")]
    pub backend: Option<FillBackendKind>,
    /// Browser endpoint: the CDP remote-debugging HTTP URL, or the host's
    /// shared Playwright WebSocket URL.
    #[arg(long, value_name = "URL")]
    pub endpoint: Option<String>,
    /// URL of the page to fill.
    #[arg(long, value_name = "URL")]
    pub page_url: Option<String>,
    /// How --page-url is compared with open pages (default: exact).
    #[arg(long, value_enum, value_name = "MODE")]
    pub page_match: Option<PageMatchArg>,
    /// Browser context index; 0 is the default context.
    #[arg(long, value_name = "N")]
    pub context_index: Option<usize>,
    /// Strict chain of iframe selectors from the main frame; repeatable.
    #[arg(long = "frame-selector", value_name = "CSS")]
    pub frame_selectors: Vec<String>,
    /// CSS selector of the single target element.
    #[arg(long, value_name = "CSS")]
    pub selector: Option<String>,
    /// Playwright browser family.
    #[arg(long, value_enum, value_name = "FAMILY")]
    pub browser: Option<BrowserFamilyArg>,
    /// Desktop: the expected foreground process id.
    #[arg(long, value_name = "PID")]
    pub expect_pid: Option<u32>,
    /// Whole-operation deadline in milliseconds (default 30000).
    #[arg(long, value_parser = clap::value_parser!(u64).range(1..=300_000), value_name = "MS")]
    pub timeout_ms: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum FillBackendKind {
    /// Chrome DevTools Protocol connection to an existing Chromium browser.
    Cdp,
    /// Playwright protocol connection to a host's shared browser endpoint.
    Playwright,
    /// Focused desktop input.
    Desktop,
    /// Scripted destination for the integration tests (debug builds only).
    #[cfg(all(feature = "test-keychain", debug_assertions))]
    Test,
}

impl FillBackendKind {
    fn token(self) -> &'static str {
        match self {
            Self::Cdp => "cdp",
            Self::Playwright => "playwright",
            Self::Desktop => "desktop",
            #[cfg(all(feature = "test-keychain", debug_assertions))]
            Self::Test => "test",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum PageMatchArg {
    Exact,
    OriginPath,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum BrowserFamilyArg {
    Chromium,
    Firefox,
    Webkit,
}

pub(super) fn execute(
    config: &Config,
    args: CredentialFillArgs,
    json: bool,
) -> Result<Output, AppError> {
    if args.capabilities {
        return capabilities(&args, json);
    }
    let Some(name) = args.name.as_deref() else {
        return Err(AppError::Usage(
            "credential fill requires a credential name, or --capabilities".to_owned(),
        ));
    };
    let Some(kind) = args.backend else {
        return Err(AppError::Usage(
            "credential fill requires --backend <cdp|playwright|desktop>".to_owned(),
        ));
    };
    validate_backend_arguments(&args, kind)?;

    let definition = credential_for(config, name)?;
    fill::check_fillable(definition)?;
    if !fill::resolver_available(definition) {
        return Err(FillError::new(
            Reason::BackendUnavailable,
            format!(
                "credential '{name}' needs the confidential resolver, which is unavailable on this platform; use an env credential"
            ),
        )
        .into());
    }

    let executable = std::env::current_exe().map_err(|_| {
        AppError::Fill(
            "backend-unavailable: could not determine the agentenv executable path".to_owned(),
        )
    })?;
    let timeout = args
        .timeout_ms
        .map(Duration::from_millis)
        .unwrap_or(fill::DEFAULT_TIMEOUT);
    let outcome = run_selected(kind, &args, definition, &executable, timeout)?;
    let effect = match outcome.effect {
        fill::Effect::FieldFilled => "field-filled",
        fill::Effect::InputSent => "input-sent",
    };
    let stdout = if json {
        serde_json::to_string(&outcome).map_err(|_| {
            AppError::Fill("delivery-failed: the outcome could not be encoded".to_owned())
        })? + "\n"
    } else {
        format!(
            "Credential '{name}' delivered through the {} backend ({effect}).\n",
            outcome.backend
        )
    };
    Ok(Output::success(stdout, String::new()))
}

/// Constructs the selected backend and runs the operation on it.
fn run_selected(
    kind: FillBackendKind,
    args: &CredentialFillArgs,
    definition: &CredentialDef,
    executable: &std::path::Path,
    timeout: Duration,
) -> Result<Outcome, AppError> {
    match kind {
        FillBackendKind::Cdp => {
            let mut backend = CdpBackend::new(browser_target(args)?);
            run_backend(&mut backend, definition, executable, timeout)
        }
        FillBackendKind::Playwright | FillBackendKind::Desktop => Err(FillError::new(
            Reason::BackendUnavailable,
            format!(
                "the {} backend is not available in this build; run 'agentenv credential fill --capabilities' to see what is",
                kind.token()
            ),
        )
        .into()),
        #[cfg(all(feature = "test-keychain", debug_assertions))]
        FillBackendKind::Test => {
            let mut backend =
                fill::test_backend::TestBackend::from_environment().ok_or_else(|| {
                    FillError::new(
                        Reason::BackendUnavailable,
                        "the test backend needs AGENTENV_FILL_TEST_BEHAVIOR",
                    )
                })?;
            run_backend(&mut backend, definition, executable, timeout)
        }
    }
}

/// Runs the coordinator on its own runtime with SIGINT, SIGTERM, and SIGHUP
/// forwarded as cancellation so the resolver is reaped and the backend
/// released instead of dying on the default signal action.
fn run_backend<B: Backend>(
    backend: &mut B,
    definition: &CredentialDef,
    executable: &std::path::Path,
    timeout: Duration,
) -> Result<Outcome, AppError> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|_| {
            AppError::Fill("backend-unavailable: could not initialize the fill runtime".to_owned())
        })?;
    let (cancel, mut cancellation) = tokio::sync::watch::channel(0);
    super::signals::install_signal_forwarder(&runtime, cancel, || {
        AppError::Fill("backend-unavailable: could not install cancellation handlers".to_owned())
    })?;
    let cancelled = async move {
        while cancellation.changed().await.is_ok() {
            if *cancellation.borrow() != 0 {
                return;
            }
        }
        std::future::pending::<()>().await;
    };
    runtime.block_on(fill::run(
        backend, definition, executable, timeout, cancelled,
    ))
}

/// The browser target named by CDP or Playwright arguments, validated.
pub(super) fn browser_target(args: &CredentialFillArgs) -> Result<BrowserTarget, AppError> {
    let backend = args.backend.map(|kind| kind.token()).unwrap_or("browser");
    let required = |value: &Option<String>, flag: &str| {
        value
            .clone()
            .ok_or_else(|| AppError::Usage(format!("--backend {backend} requires {flag}")))
    };
    let endpoint = required(&args.endpoint, "--endpoint")?;
    let page_url = required(&args.page_url, "--page-url")?;
    if !page_url.contains("://") {
        return Err(AppError::Usage(
            "--page-url must be an absolute URL with a scheme and host".to_owned(),
        ));
    }
    Ok(BrowserTarget {
        endpoint,
        page_url,
        page_match: match args.page_match.unwrap_or(PageMatchArg::Exact) {
            PageMatchArg::Exact => PageMatch::Exact,
            PageMatchArg::OriginPath => PageMatch::OriginPath,
        },
        context_index: args.context_index,
        frame_selectors: args.frame_selectors.clone(),
        selector: required(&args.selector, "--selector")?,
    })
}

fn validate_backend_arguments(
    args: &CredentialFillArgs,
    kind: FillBackendKind,
) -> Result<(), AppError> {
    let forbid = |present: bool, flag: &str| {
        if present {
            Err(AppError::Usage(format!(
                "{flag} does not apply to --backend {}",
                kind.token()
            )))
        } else {
            Ok(())
        }
    };
    match kind {
        FillBackendKind::Cdp => {
            browser_target(args)?;
            forbid(args.browser.is_some(), "--browser")?;
            forbid(args.expect_pid.is_some(), "--expect-pid")?;
        }
        FillBackendKind::Playwright => {
            browser_target(args)?;
            if args.browser.is_none() {
                return Err(AppError::Usage(
                    "--backend playwright requires --browser <chromium|firefox|webkit>".to_owned(),
                ));
            }
            forbid(args.expect_pid.is_some(), "--expect-pid")?;
        }
        FillBackendKind::Desktop => {
            if args.expect_pid.is_none() {
                return Err(AppError::Usage(
                    "--backend desktop requires --expect-pid".to_owned(),
                ));
            }
            forbid(args.endpoint.is_some(), "--endpoint")?;
            forbid(args.page_url.is_some(), "--page-url")?;
            forbid(args.page_match.is_some(), "--page-match")?;
            forbid(args.selector.is_some(), "--selector")?;
            forbid(!args.frame_selectors.is_empty(), "--frame-selector")?;
            forbid(args.context_index.is_some(), "--context-index")?;
            forbid(args.browser.is_some(), "--browser")?;
        }
        #[cfg(all(feature = "test-keychain", debug_assertions))]
        FillBackendKind::Test => {}
    }
    Ok(())
}

fn capabilities(args: &CredentialFillArgs, json: bool) -> Result<Output, AppError> {
    if args.name.is_some()
        || args.backend.is_some()
        || args.endpoint.is_some()
        || args.page_url.is_some()
        || args.page_match.is_some()
        || args.selector.is_some()
        || !args.frame_selectors.is_empty()
        || args.context_index.is_some()
        || args.browser.is_some()
        || args.expect_pid.is_some()
        || args.timeout_ms.is_some()
    {
        return Err(AppError::Usage(
            "--capabilities takes no credential name, destination, or timeout arguments".to_owned(),
        ));
    }
    let backends = [
        (
            "cdp",
            true,
            true,
            "attaches to a Chromium-family browser through a loopback remote debugging port",
        ),
        ("playwright", false, false, "not included in this build"),
        ("desktop", false, false, "not included in this build"),
    ];
    let resolver_confidential = cfg!(unix);
    if json {
        let mut table = serde_json::Map::new();
        for (name, compiled, available, detail) in backends {
            table.insert(
                name.to_owned(),
                serde_json::json!({ "compiled": compiled, "available": available, "detail": detail }),
            );
        }
        let document = serde_json::json!({
            "version": 1,
            "backends": table,
            "resolver": { "confidential": resolver_confidential },
        });
        return Ok(Output::success(
            serde_json::to_string(&document).map_err(|_| {
                AppError::Fill("delivery-failed: capabilities could not be encoded".to_owned())
            })? + "\n",
            String::new(),
        ));
    }
    let mut text = String::new();
    for (name, compiled, available, detail) in backends {
        text.push_str(&format!(
            "{name}: {} ({detail})\n",
            match (compiled, available) {
                (true, true) => "available",
                (true, false) => "compiled, unavailable",
                _ => "not compiled",
            }
        ));
    }
    text.push_str(&format!(
        "confidential resolver: {}\n",
        if resolver_confidential {
            "available"
        } else {
            "unavailable on this platform; only env credentials can be filled"
        }
    ));
    Ok(Output::success(text, String::new()))
}
