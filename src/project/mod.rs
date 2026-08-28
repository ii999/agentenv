//! Project-scoped configuration: discovery, closed-schema validation, and the
//! trust-on-first-use gate for a checked-in `.agentenv.toml` (change
//! 003-project-config).

use std::fs;
use std::path::{Path, PathBuf};

use crate::error::{AppError, Violation};

pub mod locate;
pub mod model;
pub mod trust;

use model::ProjectFileMeta;
use trust::{fingerprint, store_path, RealFs, TrustStore};

/// Why a discovered project file cannot affect configuration selection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UntrustedReason {
    /// The file has never been approved at this canonical path.
    New,
    /// The approved file's exact bytes no longer match its approval record.
    Changed,
    /// The file cannot be read or fails closed-schema validation.
    Invalid(Vec<Violation>),
    /// The trust store's state directory could not be resolved from the
    /// environment, so the file must remain inert for this invocation.
    StateUnavailable(String),
}

/// The trust classification of the nearest discovered project file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProjectContext {
    /// No regular `.agentenv.toml` exists between the working directory and
    /// filesystem root.
    None,
    /// A discovered file is inert until its exact valid bytes are approved.
    Untrusted {
        path: PathBuf,
        reason: UntrustedReason,
        meta: Option<ProjectFileMeta>,
    },
    /// A discovered file whose exact valid bytes match its approval record.
    Trusted {
        path: PathBuf,
        meta: ProjectFileMeta,
    },
}

/// The observable result of approving a project file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AllowOutcome {
    pub path: PathBuf,
    pub already_current: bool,
}

/// The observable result of revoking approval for a project file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RevokeOutcome {
    pub path: PathBuf,
    pub record_existed: bool,
}

/// Resolves the nearest project file's trust state from one immutable content
/// snapshot. A missing or unavailable state base leaves a project file inert;
/// a corrupt trust store is an explicit configuration error.
pub fn resolve(
    cwd: &Path,
    env: &impl Fn(&str) -> Option<String>,
) -> Result<ProjectContext, AppError> {
    let Some(discovered) = locate::discover(cwd) else {
        return Ok(ProjectContext::None);
    };
    let canonical = match canonicalize_project_file(&discovered) {
        Ok(path) => path,
        Err(violation) => return Ok(invalid_context(discovered, vec![violation])),
    };

    let store_path = match store_path(env) {
        Ok(path) => path,
        Err(error) => return Ok(state_unavailable_context(canonical, error)),
    };
    let store = TrustStore::load(&store_path, &RealFs)?;
    let approved_fingerprint = store.lookup(&canonical).map(str::to_owned);

    let snapshot = match fs::read(&canonical) {
        Ok(bytes) => bytes,
        Err(_) if approved_fingerprint.is_some() => {
            return Err(project_read_after_approval_error(&canonical));
        }
        Err(error) => {
            let violation = read_violation(&canonical, &error);
            return Ok(invalid_context(canonical, vec![violation]));
        }
    };

    let matches_approval = approved_fingerprint.as_deref() == Some(fingerprint(&snapshot).as_str());
    match model::parse(&snapshot, &canonical) {
        Ok(meta) if matches_approval => Ok(ProjectContext::Trusted {
            path: canonical,
            meta,
        }),
        Ok(meta) => Ok(ProjectContext::Untrusted {
            path: canonical,
            reason: match approved_fingerprint {
                Some(_) => UntrustedReason::Changed,
                None => UntrustedReason::New,
            },
            meta: Some(meta),
        }),
        Err(violations) => Ok(invalid_context(canonical, violations)),
    }
}

/// Validates and records approval of the nearest project file's one-read byte
/// snapshot. An approval is only written after validation succeeds.
pub fn allow(cwd: &Path, env: &impl Fn(&str) -> Option<String>) -> Result<AllowOutcome, AppError> {
    let discovered = discovered_project_file(cwd)?;
    let canonical = canonicalize_or_config_error(&discovered)?;
    let store_path = store_path(env)?;
    let mut store = TrustStore::load(&store_path, &RealFs)?;
    let snapshot = read_project_file_for_allow(&canonical)?;
    let _meta = model::parse(&snapshot, &canonical).map_err(AppError::Config)?;
    let current_fingerprint = fingerprint(&snapshot);
    let already_current = store.lookup(&canonical) == Some(current_fingerprint.as_str());

    if !already_current {
        store.allow(&canonical, &snapshot);
        store.save(&store_path, &RealFs)?;
    }

    Ok(AllowOutcome {
        path: canonical,
        already_current,
    })
}

/// Removes the nearest project file's approval by canonical path without
/// reading or validating the file contents.
pub fn revoke(
    cwd: &Path,
    env: &impl Fn(&str) -> Option<String>,
) -> Result<RevokeOutcome, AppError> {
    let discovered = discovered_project_file(cwd)?;
    let canonical = canonicalize_or_config_error(&discovered)?;
    let store_path = store_path(env)?;
    let mut store = TrustStore::load(&store_path, &RealFs)?;
    let record_existed = store.revoke(&canonical);
    store.save(&store_path, &RealFs)?;

    Ok(RevokeOutcome {
        path: canonical,
        record_existed,
    })
}

fn discovered_project_file(cwd: &Path) -> Result<PathBuf, AppError> {
    locate::discover(cwd).ok_or_else(|| {
        AppError::ProjectTrust(
            "no .agentenv.toml was found in the working directory or its ancestors; create one, then run `agentenv project allow`"
                .to_owned(),
        )
    })
}

fn canonicalize_project_file(path: &Path) -> Result<PathBuf, Violation> {
    path.canonicalize().map_err(|_| Violation {
        path: path.display().to_string(),
        message: "could not canonicalize the project file; restore the file, then run `agentenv project allow`"
            .to_owned(),
    })
}

fn canonicalize_or_config_error(path: &Path) -> Result<PathBuf, AppError> {
    canonicalize_project_file(path).map_err(|violation| AppError::Config(vec![violation]))
}

fn state_unavailable_context(path: PathBuf, error: AppError) -> ProjectContext {
    ProjectContext::Untrusted {
        path,
        reason: UntrustedReason::StateUnavailable(error.to_string()),
        meta: None,
    }
}

fn invalid_context(path: PathBuf, violations: Vec<Violation>) -> ProjectContext {
    ProjectContext::Untrusted {
        path,
        reason: UntrustedReason::Invalid(violations),
        meta: None,
    }
}

fn read_violation(path: &Path, error: &std::io::Error) -> Violation {
    Violation {
        path: path.display().to_string(),
        message: format!(
            "could not read the project file ({:?}); restore it, then run `agentenv project allow`",
            error.kind()
        ),
    }
}

fn project_read_after_approval_error(path: &Path) -> AppError {
    AppError::Config(vec![Violation {
        path: path.display().to_string(),
        message: "could not read an approved project file; restore the file or run `agentenv project revoke`"
            .to_owned(),
    }])
}

fn read_project_file_for_allow(path: &Path) -> Result<Vec<u8>, AppError> {
    fs::read(path).map_err(|error| AppError::Config(vec![read_violation(path, &error)]))
}
