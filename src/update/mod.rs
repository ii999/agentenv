//! Self-update against GitHub Releases.
//!
//! An update has two halves that share one interface: [`check`] resolves the
//! installed binary, the platform's release asset, and the newest published
//! version without writing anything; [`apply`] does the same and then
//! downloads, verifies, and replaces the main executable, both companions,
//! and every installed copy of the agent skill. Everything the command needs
//! is derived from the running binary and the user's home directory, so there
//! is no install manifest to keep in sync with the install scripts.
//!
//! Release discovery goes through the `SHA256SUMS` file each release ships:
//! its asset names carry the tag and its digests verify the download, so one
//! request answers "what is the latest version" and "what bytes do I
//! expect". `AGENTENV_RELEASE_BASE_URL` overrides the release base for
//! mirrors and tests.

mod install;
mod release;

use std::fmt;
use std::io::Write;
use std::path::PathBuf;

use semver::Version;

use crate::config::env_value;
use crate::error::AppError;

pub use install::Installation;

/// The target triple this binary was built for, injected by `build.rs`. It
/// selects the release asset, so a binary can only ever update to another
/// build of the same platform.
pub const TARGET: &str = env!("AGENTENV_TARGET");

/// The environment variable that overrides the release base URL.
pub const BASE_URL_ENV: &str = "AGENTENV_RELEASE_BASE_URL";

/// What an update run should do beyond the defaults.
#[derive(Debug, Default)]
pub struct Options {
    /// A release tag to install instead of the latest release. An explicit
    /// tag may move to an older version.
    pub tag: Option<String>,
    /// Reinstall even when the installed version already matches.
    pub force: bool,
    /// Leave installed agent-skill copies untouched.
    pub skip_skill: bool,
}

/// The comparison between the installed binary and a published release.
#[derive(Debug)]
pub struct Status {
    pub current: Version,
    pub available: Version,
    pub tag: String,
    pub target: &'static str,
    pub binary: PathBuf,
    pub skills: Vec<PathBuf>,
}

impl Status {
    pub fn is_newer(&self) -> bool {
        self.available > self.current
    }

    pub fn is_current(&self) -> bool {
        self.available == self.current
    }
}

/// A skill copy that could not be refreshed after the binary was replaced.
#[derive(Debug)]
pub struct SkillFailure {
    pub path: PathBuf,
    pub error: String,
}

/// What [`apply`] changed.
#[derive(Debug)]
pub struct Report {
    pub from: Version,
    pub to: Version,
    pub tag: String,
    pub binary: PathBuf,
    pub skills: Vec<PathBuf>,
    pub skill_failures: Vec<SkillFailure>,
}

/// The result of [`apply`]: either nothing needed to change, or the binary
/// was replaced (with the skill outcome recorded in the report).
#[derive(Debug)]
pub enum Outcome {
    UpToDate(Status),
    Updated(Report),
}

/// Resolves the installation and the requested release without writing.
pub fn check(
    options: &Options,
    env: &impl Fn(&str) -> Option<String>,
    progress: &mut dyn Write,
) -> Result<Status, AppError> {
    let installation = Installation::discover(env)?;
    let base_url = base_url(env);
    let client = release::Client::new(&base_url);
    write_progress(progress, format!("Checking {base_url} for {TARGET}..."))?;
    let release = client.fetch(options.tag.as_deref())?;
    Ok(Status {
        current: current_version(),
        available: release.version.clone(),
        tag: release.tag.clone(),
        target: TARGET,
        binary: installation.binary,
        skills: if options.skip_skill {
            Vec::new()
        } else {
            installation.skills
        },
    })
}

/// Installs the requested release over the running executable bundle and every
/// installed skill copy.
///
/// The download is verified against `SHA256SUMS`, and every extracted
/// executable must report its exact identity and release version before the
/// installed bundle is replaced. A skill copy that fails to refresh after the
/// bundle is in place is recorded in the report instead of failing the whole
/// run, because the binary replacement has already happened and the report
/// must say so.
pub fn apply(
    options: &Options,
    env: &impl Fn(&str) -> Option<String>,
    progress: &mut dyn Write,
) -> Result<Outcome, AppError> {
    let status = check(options, env, progress)?;
    if status.is_current() && !options.force {
        return Ok(Outcome::UpToDate(status));
    }
    if !status.is_newer() && options.tag.is_none() && !options.force {
        return Err(AppError::Update(format!(
            "the latest release {} is older than the installed {}; pass --version {} to move to it deliberately",
            status.available, status.current, status.tag
        )));
    }

    let base_url = base_url(env);
    let client = release::Client::new(&base_url);
    let release = client.fetch(Some(&status.tag))?;
    let workdir = tempfile::Builder::new()
        .prefix("agentenv-update-")
        .tempdir()
        .map_err(|error| {
            AppError::Update(format!("cannot create a temporary directory: {error}"))
        })?;

    write_progress(progress, format!("Downloading {}...", release.asset.name))?;
    let archive = client.download(&release, workdir.path())?;
    write_progress(
        progress,
        format!("Verified {} against SHA256SUMS", release.asset.name),
    )?;
    let extracted = release::extract(&archive, &release, workdir.path())?;
    release::verify_bundle(&extracted, &release.version)?;

    let installation = Installation::discover(env)?;
    install::replace_bundle(&extracted, &installation)?;
    write_progress(
        progress,
        format!(
            "Replaced the matching executable bundle at {}",
            status.binary.display()
        ),
    )?;

    let mut skills = Vec::new();
    let mut skill_failures = Vec::new();
    for root in &status.skills {
        match &extracted.skill {
            Some(source) => match install::replace_skill(root, source) {
                Ok(()) => {
                    write_progress(
                        progress,
                        format!("Refreshed the agent skill at {}", root.display()),
                    )?;
                    skills.push(root.clone());
                }
                Err(error) => skill_failures.push(SkillFailure {
                    path: root.clone(),
                    error: error.to_string(),
                }),
            },
            None => skill_failures.push(SkillFailure {
                path: root.clone(),
                error: format!("release {} ships no agent skill", release.tag),
            }),
        }
    }

    Ok(Outcome::Updated(Report {
        from: status.current,
        to: release.version,
        tag: release.tag,
        binary: status.binary,
        skills,
        skill_failures,
    }))
}

/// The version compiled into this binary.
pub fn current_version() -> Version {
    Version::parse(env!("CARGO_PKG_VERSION")).expect("Cargo.toml carries a semver version")
}

fn base_url(env: &impl Fn(&str) -> Option<String>) -> String {
    env_value(env, BASE_URL_ENV)
        .map(|url| url.trim_end_matches('/').to_owned())
        .unwrap_or_else(|| release::DEFAULT_BASE_URL.to_owned())
}

fn write_progress(progress: &mut dyn Write, line: impl fmt::Display) -> Result<(), AppError> {
    writeln!(progress, "{line}")
        .map_err(|error| AppError::Update(format!("cannot write progress output: {error}")))
}
