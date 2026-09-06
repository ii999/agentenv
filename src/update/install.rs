//! Where agentenv is installed and how its pieces are replaced in place.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::config::env_value;
use crate::error::AppError;

/// Skill roots the install scripts write to, relative to the home directory.
const SKILL_ROOTS: &[&[&str]] = &[&[".agents", "skills"], &[".claude", "skills"]];

/// The running binary and every installed copy of the agent skill.
#[derive(Debug)]
pub struct Installation {
    /// The binary's real path, with symlinks resolved.
    pub binary: PathBuf,
    /// Existing `<root>/agentenv` skill directories, each holding `SKILL.md`.
    pub skills: Vec<PathBuf>,
}

impl Installation {
    /// Locates the running binary and refuses installs that another tool
    /// owns, so `update` never fights cargo or a package manager.
    pub fn discover(env: &impl Fn(&str) -> Option<String>) -> Result<Self, AppError> {
        let exe = std::env::current_exe().map_err(|error| {
            AppError::Update(format!("cannot locate the running binary: {error}"))
        })?;
        let binary = exe.canonicalize().map_err(|error| {
            AppError::Update(format!("cannot resolve {}: {error}", exe.display()))
        })?;
        if let Some(owner) = foreign_owner(&binary) {
            return Err(AppError::Update(format!(
                "{} is managed by {owner}; update it with {} instead",
                binary.display(),
                owner_command(owner)
            )));
        }
        let home = home_dir(env)?;
        let skills = SKILL_ROOTS
            .iter()
            .map(|segments| {
                let mut path = home.clone();
                path.extend(segments.iter());
                path.join("agentenv")
            })
            .filter(|skill| skill.join("SKILL.md").is_file())
            .collect();
        Ok(Self { binary, skills })
    }
}

fn foreign_owner(binary: &Path) -> Option<&'static str> {
    let rendered = binary.to_string_lossy().replace('\\', "/");
    if rendered.contains("/.cargo/bin/") {
        return Some("cargo");
    }
    if rendered.contains("/Cellar/") || rendered.starts_with("/opt/homebrew/") {
        return Some("Homebrew");
    }
    None
}

fn owner_command(owner: &str) -> &'static str {
    match owner {
        "cargo" => "'cargo install agentenv' or 'cargo install --path .'",
        _ => "'brew upgrade agentenv'",
    }
}

fn home_dir(env: &impl Fn(&str) -> Option<String>) -> Result<PathBuf, AppError> {
    let name = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    env_value(env, name).map(PathBuf::from).ok_or_else(|| {
        AppError::Update(format!(
            "cannot locate installed agent skills: {name} is not set; set it or pass --no-skill"
        ))
    })
}

/// Replaces the running binary with `new_binary`. The installed path is only
/// used for diagnostics: the replacement targets the running executable.
pub fn replace_binary(new_binary: &Path, installed: &Path) -> Result<(), AppError> {
    self_replace::self_replace(new_binary).map_err(|error| {
        AppError::Update(format!(
            "cannot replace {}: {error}; check that its directory is writable",
            installed.display()
        ))
    })
}

/// Replaces the skill directory at `destination` with a copy of `source`.
///
/// The copy lands beside the destination first, so a failed copy leaves the
/// installed skill untouched; the swap then happens with two renames.
pub fn replace_skill(destination: &Path, source: &Path) -> Result<(), io::Error> {
    let parent = destination.parent().ok_or_else(|| {
        io::Error::other(format!("{} has no parent directory", destination.display()))
    })?;
    let staged = parent.join("agentenv.update-new");
    let retired = parent.join("agentenv.update-old");
    remove_dir_if_present(&staged)?;
    remove_dir_if_present(&retired)?;
    copy_dir(source, &staged)?;
    fs::rename(destination, &retired)?;
    if let Err(error) = fs::rename(&staged, destination) {
        // Put the previous skill back so the install is never left empty.
        let _ = fs::rename(&retired, destination);
        return Err(error);
    }
    fs::remove_dir_all(&retired)
}

fn remove_dir_if_present(path: &Path) -> io::Result<()> {
    match fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn copy_dir(source: &Path, destination: &Path) -> io::Result<()> {
    fs::create_dir_all(destination)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let target = destination.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir(&entry.path(), &target)?;
        } else {
            fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}
