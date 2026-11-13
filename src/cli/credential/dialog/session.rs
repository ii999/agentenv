use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread::{self, JoinHandle};

use agentenv::config::CredentialDef;
use agentenv::credential::{provider_for, Secret};
use eframe::egui::{Context, TextBuffer};

use super::{input::Draft, Completion, Failure};

pub(super) struct Session {
    pub definition: CredentialDef,
    pub draft: Draft,
    pub revealed: bool,
    pub error: Option<String>,
    pub error_details: Option<String>,
    pub focus_input: bool,
    saving: bool,
    job: Option<StoreJob>,
    cancel_requested: Option<i32>,
    uncertain: bool,
    completion: Option<Completion>,
}

struct StoreJob {
    result: Receiver<Result<(), StoreError>>,
    thread: JoinHandle<()>,
}

pub(super) struct StoreError {
    pub message: String,
    pub details: Option<String>,
    pub uncertain: bool,
}

impl Session {
    pub fn new(definition: CredentialDef) -> Self {
        Self {
            definition,
            draft: Draft::new(),
            revealed: false,
            error: None,
            error_details: None,
            focus_input: true,
            saving: false,
            job: None,
            cancel_requested: None,
            uncertain: false,
            completion: None,
        }
    }

    pub fn saving(&self) -> bool {
        self.saving
    }
    pub fn done(&self) -> bool {
        self.completion.is_some()
    }
    pub fn closing(&self) -> bool {
        self.cancel_requested.is_some()
    }

    pub fn clear_error(&mut self) {
        self.error = None;
        self.error_details = None;
    }

    fn prepare_save(&mut self) -> Option<Secret> {
        if self.saving || self.done() {
            return None;
        }
        self.revealed = false;
        match self.draft.secret() {
            Ok(secret) => {
                self.clear_error();
                self.saving = true;
                Some(secret)
            }
            Err(error) => {
                self.error_details = None;
                self.error = Some(format!(
                    "Cannot save: {error}. Edit the value and try again."
                ));
                self.focus_input = true;
                None
            }
        }
    }

    pub fn save(&mut self, context: &Context) {
        let Some(secret) = self.prepare_save() else {
            return;
        };
        let definition = self.definition.clone();
        let repaint = context.clone();
        let (sender, result) = mpsc::channel();
        match thread::Builder::new().name("agentenv-credential-store".into()).spawn(move || {
            // The provider is constructed on this thread; its public trait
            // does not need a new Send bound or plaintext accessors.
            let saved = catch_unwind(AssertUnwindSafe(|| provider_for(&definition).store(secret)))
                .map_err(|_| StoreError {
                    message: "The credential store did not confirm the save. Your input is retained; retry the save or cancel.".into(),
                    details: None,
                    uncertain: true,
                })
                .and_then(|result| result.map_err(|error| {
                    let uncertain = matches!(&error, agentenv::error::AppError::Keychain(failure) if failure.store_unconfirmed());
                    StoreError {
                        message: if uncertain {
                            "The credential store did not confirm the save. Your input is retained; check the credential before deciding whether to save again.".into()
                        } else {
                            "Could not save to the credential store. Your input is retained; check access to the store, then try Save again.".into()
                        },
                        details: Some(error.to_string()),
                        uncertain,
                    }
                }));
            let _ = sender.send(saved);
            repaint.request_repaint();
        }) {
            Ok(thread) => self.job = Some(StoreJob { result, thread }),
            Err(_) => self.finish_save(Err(StoreError {
                message: "The save could not start. Your input is retained; try Save again.".into(),
                details: None,
                uncertain: false,
            })),
        }
    }

    pub fn poll(&mut self) {
        let Some(job) = &self.job else {
            return;
        };
        let result = match job.result.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => Err(unconfirmed()),
        };
        if let Some(job) = self.job.take() {
            let _ = job.thread.join();
        }
        self.finish_save(result);
    }

    fn finish_save(&mut self, result: Result<(), StoreError>) {
        self.saving = false;
        match result {
            Ok(()) => self.complete(Completion::Stored),
            Err(error) => {
                self.uncertain |= error.uncertain;
                self.error = Some(error.message);
                self.error_details = error.details;
                self.focus_input = true;
                if let Some(signal) = self.cancel_requested {
                    self.cancel(signal);
                }
            }
        }
    }

    pub fn cancel(&mut self, signal: i32) {
        if self.done() {
            return;
        }
        self.revealed = false;
        self.cancel_requested = Some(signal);
        if !self.saving {
            self.complete(if self.uncertain {
                Completion::Failed(Failure::StoreUnconfirmed)
            } else {
                Completion::Cancelled(signal)
            });
        }
    }

    fn complete(&mut self, completion: Completion) {
        self.revealed = false;
        self.draft.clear();
        self.completion = Some(completion);
    }

    /// Even an unexpected GUI shutdown must observe an already dispatched
    /// store before the CLI reports an outcome. A successful store wins over
    /// a concurrent close request; it cannot honestly be reported as cancelled.
    pub fn finish(&mut self, gui_failed: bool) -> Completion {
        if let Some(job) = self.job.take() {
            let result = job.result.recv().unwrap_or_else(|_| Err(unconfirmed()));
            let _ = job.thread.join();
            self.finish_save(result);
            if !self.done() {
                self.complete(Completion::Failed(if self.uncertain {
                    Failure::StoreUnconfirmed
                } else {
                    Failure::StoreFailed
                }));
            }
        }
        if !self.done() {
            self.complete(if self.uncertain {
                Completion::Failed(Failure::StoreUnconfirmed)
            } else if gui_failed {
                Completion::Failed(Failure::DialogFailed)
            } else {
                Completion::Cancelled(2)
            });
        }
        self.completion
            .expect("finish assigns a completion on every path")
    }
}

fn unconfirmed() -> StoreError {
    StoreError { message: "The credential store did not confirm the save. Your input is retained; retry the save or cancel.".into(), details: None, uncertain: true }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentenv::config::{CredentialUsage, Provider};

    pub(super) fn fixture() -> Session {
        Session::new(CredentialDef {
            name: "dialog_fixture".into(),
            description: "Dialog test credential.".into(),
            // State and renderer tests never dispatch storage. This provider
            // also refuses stores if an unintended UI action reaches it.
            provider: Provider::Env {
                name: "UNUSED_DIALOG_FIXTURE".into(),
            },
            inject_as: Some("FIXTURE_TOKEN".into()),
            usages: vec![CredentialUsage::Environment],
        })
    }

    #[test]
    fn validation_and_store_failure_retain_exact_input_for_retry() {
        let mut session = fixture();
        assert!(session.prepare_save().is_none());
        assert!(session.error.as_deref().unwrap().contains("empty"));
        session.draft.replace_with("bad\0value");
        assert!(session.prepare_save().is_none());
        assert_eq!(session.draft.as_str(), "bad\0value");
        let value = "  token-密钥-🙂-e\u{301}\r\nsecond\tline\n";
        session.draft.replace_with(value);
        session.revealed = true;
        assert!(session.prepare_save().is_some());
        assert!(!session.revealed);
        assert!(
            session.prepare_save().is_none(),
            "duplicate saves are suppressed while one is pending"
        );
        session.finish_save(Err(StoreError {
            message: "test store is unavailable".into(),
            details: None,
            uncertain: false,
        }));
        assert!(!session.saving());
        assert!(!session.done());
        assert_eq!(session.draft.as_str(), value);
        assert!(session.prepare_save().is_some());
        session.finish_save(Ok(()));
        assert_eq!(session.finish(false), Completion::Stored);
        assert!(session.draft.as_str().is_empty());
    }

    #[test]
    fn cancellation_waits_for_pending_save_and_reports_observed_success() {
        let mut session = fixture();
        session.draft.replace_with("synthetic-store-value");
        assert!(session.prepare_save().is_some());
        session.cancel(15);
        assert!(!session.done());
        assert!(session.closing());
        session.finish_save(Ok(()));
        assert_eq!(
            session.finish(true),
            Completion::Stored,
            "a later GUI failure cannot undo a committed save"
        );

        let mut session = fixture();
        session.draft.replace_with("unsaved");
        session.cancel(2);
        assert_eq!(session.finish(false), Completion::Cancelled(2));
        assert!(session.draft.as_str().is_empty());
    }

    #[test]
    fn an_unconfirmed_store_cannot_be_reported_as_an_ordinary_cancellation() {
        let mut session = fixture();
        session.draft.replace_with("unknown-store-value");
        assert!(session.prepare_save().is_some());
        session.finish_save(Err(unconfirmed()));
        assert_eq!(session.draft.as_str(), "unknown-store-value");
        session.cancel(2);
        assert_eq!(
            session.finish(false),
            Completion::Failed(Failure::StoreUnconfirmed)
        );
    }

    #[test]
    fn a_later_gui_failure_preserves_an_earlier_unconfirmed_store() {
        let mut session = fixture();
        session.draft.replace_with("unknown-store-value");
        assert!(session.prepare_save().is_some());
        session.finish_save(Err(unconfirmed()));
        assert!(!session.saving());
        assert_eq!(
            session.finish(true),
            Completion::Failed(Failure::StoreUnconfirmed)
        );
        assert!(session.draft.as_str().is_empty());
    }
}
