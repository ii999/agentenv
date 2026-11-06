//! A local human input channel. Only completion metadata crosses back to the
//! calling Agent; input, rendering and storage retries stay inside the window.

mod input;
mod platform;
mod session;
mod theme;
mod ui;

use std::cell::RefCell;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::rc::Rc;

use agentenv::config::CredentialDef;
use agentenv::error::AppError;
use eframe::egui;

use super::super::{signals::install_signal_forwarder, Output};
use session::Session;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Completion {
    Stored,
    Cancelled(i32),
    Failed(Failure),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Failure {
    DesktopUnavailable,
    #[cfg(target_os = "linux")]
    KeyboardUnavailable,
    DialogFailed,
    StoreFailed,
    StoreUnconfirmed,
}

impl Failure {
    fn token(self) -> &'static str {
        match self {
            Self::DesktopUnavailable => "desktop-unavailable",
            #[cfg(target_os = "linux")]
            Self::KeyboardUnavailable => "keyboard-unavailable",
            Self::DialogFailed => "dialog-failed",
            Self::StoreFailed => "store-failed",
            Self::StoreUnconfirmed => "store-unconfirmed",
        }
    }

    fn message(self) -> &'static str {
        match self {
            Self::DesktopUnavailable => "a local graphical desktop is unavailable; run the command in a desktop session, or run 'agentenv credential set <name>' yourself at a terminal",
            #[cfg(target_os = "linux")]
            Self::KeyboardUnavailable => "the desktop keyboard libraries are unavailable; use a desktop with XKB support, or run 'agentenv credential set <name>' yourself at a terminal",
            Self::DialogFailed => "the local credential window could not complete; check desktop and graphics access, then retry, or use terminal input",
            Self::StoreFailed => "the credential store failed while the window was closing; check access to the platform credential store and retry",
            Self::StoreUnconfirmed => "the credential store did not confirm the write; the saved value may have changed; inspect the credential store before retrying",
        }
    }
}

impl Completion {
    pub(super) fn output(self, name: &str, json: bool) -> Output {
        let (status, reason, code, message) = match self {
            Self::Stored => ("stored", None, 0, format!("Credential '{name}' stored.\n")),
            Self::Cancelled(signal) => (
                "cancelled",
                None,
                128 + signal,
                "Credential entry cancelled.\n".into(),
            ),
            Self::Failed(reason) => (
                "failed",
                Some(reason.token()),
                4,
                format!(
                    "credential-dialog: {}: {}\n",
                    reason.token(),
                    reason.message()
                ),
            ),
        };
        let stdout = if json {
            let mut result = serde_json::json!({"version": 1, "credential": name, "input": "dialog", "status": status});
            if let Some(reason) = reason {
                result["reason"] = reason.into();
            }
            format!("{result}\n")
        } else if self == Self::Stored {
            message.clone()
        } else {
            String::new()
        };
        Output {
            stdout,
            stderr: if code == 0 { String::new() } else { message },
            status: code,
        }
    }
}

pub(super) fn edit_and_store(definition: &CredentialDef) -> Completion {
    if let Err(reason) = platform::preflight() {
        return Completion::Failed(reason);
    }
    let Ok(runtime) = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
    else {
        return Completion::Failed(Failure::DialogFailed);
    };
    let (cancel, cancellation) = agentenv::cancel::channel();
    if install_signal_forwarder(&runtime, cancel, || {
        AppError::Credential("could not initialize dialog cancellation".into())
    })
    .is_err()
    {
        return Completion::Failed(Failure::DialogFailed);
    }
    let session = Rc::new(RefCell::new(Session::new(definition.clone())));
    let window_session = session.clone();
    let viewport = egui::ViewportBuilder::default()
        .with_title("agentenv — Save credential")
        .with_inner_size([480.0, 320.0])
        .with_min_inner_size([420.0, 300.0])
        .with_icon(egui::IconData::default());
    // On macOS the content extends under a transparent title bar, and the
    // transparent surface lets the system glass backdrop show through.
    #[cfg(target_os = "macos")]
    let viewport = viewport
        .with_transparent(true)
        .with_fullsize_content_view(true)
        .with_titlebar_shown(false)
        .with_title_shown(false);
    let options = eframe::NativeOptions {
        viewport,
        renderer: eframe::Renderer::Glow,
        centered: true,
        run_and_return: true,
        persist_window: false,
        clipboard_shortcuts: false,
        ..Default::default()
    };

    // Toolkit and provider panic payloads can contain arbitrary input. Never
    // pass them to the default stderr hook or format them as CLI diagnostics.
    let previous_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let gui = catch_unwind(AssertUnwindSafe(|| {
        eframe::run_native(
            "agentenv",
            options,
            Box::new(move |creation| {
                Ok(Box::new(ui::Dialog::new(
                    creation,
                    window_session,
                    cancellation,
                )?))
            }),
        )
    }));
    let completion = catch_unwind(AssertUnwindSafe(|| {
        session.borrow_mut().finish(!matches!(gui, Ok(Ok(()))))
    }))
    .unwrap_or(Completion::Failed(Failure::StoreUnconfirmed));
    std::panic::set_hook(previous_hook);
    completion
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn machine_results_have_only_completion_metadata_and_matching_exit_status() {
        for (completion, status, code, reason) in [
            (Completion::Stored, "stored", 0, None),
            (Completion::Cancelled(2), "cancelled", 130, None),
            (Completion::Cancelled(15), "cancelled", 143, None),
            (
                Completion::Failed(Failure::DesktopUnavailable),
                "failed",
                4,
                Some("desktop-unavailable"),
            ),
            (
                Completion::Failed(Failure::StoreUnconfirmed),
                "failed",
                4,
                Some("store-unconfirmed"),
            ),
        ] {
            let output = completion.output("fixture", true);
            let value: serde_json::Value = serde_json::from_str(&output.stdout).unwrap();
            let mut expected = serde_json::json!({"version":1,"credential":"fixture","input":"dialog","status":status});
            if let Some(reason) = reason {
                expected["reason"] = reason.into();
            }
            assert_eq!(value, expected);
            assert_eq!(output.status, code);
            if code == 0 {
                assert!(output.stderr.is_empty());
            }
        }
    }
}
