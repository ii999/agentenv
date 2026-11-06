//! Where agentenv is installed and how its pieces are replaced in place.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use crate::config::env_value;
use crate::error::AppError;

/// Skill roots the install scripts write to, relative to the home directory.
const SKILL_ROOTS: &[&[&str]] = &[&[".agents", "skills"], &[".claude", "skills"]];

/// The running binary and every installed copy of the agent skill.
#[derive(Debug)]
pub struct Installation {
    /// The binary's real path, with symlinks resolved.
    pub binary: PathBuf,
    pub sudo_helper: PathBuf,
    pub ssh_askpass: PathBuf,
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
        let directory = binary.parent().ok_or_else(|| {
            AppError::Update("the running binary has no installation directory".to_owned())
        })?;
        let sudo_helper = directory.join(format!(
            "agentenv-sudo-helper{}",
            std::env::consts::EXE_SUFFIX
        ));
        let ssh_askpass = directory.join(format!(
            "agentenv-ssh-askpass{}",
            std::env::consts::EXE_SUFFIX
        ));
        Ok(Self {
            binary,
            sudo_helper,
            ssh_askpass,
            skills,
        })
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

/// Replaces the running binary at `installed` with `new_binary`.
///
/// Unix renames the new binary over the running one in a single step.
#[cfg(unix)]
fn replace_binary(new_binary: &Path, installed: &Path) -> Result<(), AppError> {
    self_replace::self_replace(new_binary).map_err(|error| binary_update_error(installed, &error))
}

/// Replaces the running binary at `installed` with `new_binary`.
///
/// Windows cannot overwrite a running executable, but it can rename it. The
/// binary goes through the same backup swap as the companions, so every step
/// retries transient locks and a failed step restores the previous binary.
/// The backup is the running image and cannot be deleted yet; its removal is
/// scheduled for when this process exits. If scheduling fails, the backup
/// stays inert beside the binary and the next update's swap removes it.
#[cfg(windows)]
fn replace_binary(new_binary: &Path, installed: &Path) -> Result<(), AppError> {
    let swap = swap(new_binary, installed).map_err(|error| match error {
        SwapError::Failed(error) => binary_update_error(installed, &error),
        SwapError::RestoreFailed { error, restore } => AppError::Update(format!(
            "cannot replace {}: {error}; restoring the previous binary also failed: {restore}; rerun the installer to repair the installation",
            installed.display()
        )),
    })?;
    let _ = self_replace::self_delete_at(&swap.backup);
    Ok(())
}

fn binary_update_error(installed: &Path, error: &io::Error) -> AppError {
    AppError::Update(format!(
        "cannot replace {}: {error}; check that its directory is writable",
        installed.display()
    ))
}

/// Installs both companions before replacing the running executable. If the
/// main replacement fails, both companion changes are rolled back.
pub fn replace_bundle(
    extracted: &super::release::Extracted,
    installation: &Installation,
) -> Result<(), AppError> {
    replace_bundle_with(extracted, installation, || {
        replace_binary(&extracted.binary, &installation.binary)
    })
}

fn replace_bundle_with(
    extracted: &super::release::Extracted,
    installation: &Installation,
    replace_main: impl FnOnce() -> Result<(), AppError>,
) -> Result<(), AppError> {
    let mut swaps = Vec::new();
    for (source, destination) in [
        (&extracted.sudo_helper, &installation.sudo_helper),
        (&extracted.ssh_askpass, &installation.ssh_askpass),
    ] {
        match swap_file(source, destination) {
            Ok(swap) => swaps.push(swap),
            Err(error) => {
                return Err(with_rollback(error, rollback_files(&mut swaps)));
            }
        }
    }
    if let Err(error) = replace_main() {
        return Err(with_rollback(error, rollback_files(&mut swaps)));
    }
    for swap in swaps {
        let _ = remove_file_if_present(&swap.backup);
    }
    Ok(())
}

struct FileSwap {
    destination: PathBuf,
    backup: PathBuf,
    had_previous: bool,
}

fn swap_file(source: &Path, destination: &Path) -> Result<FileSwap, AppError> {
    swap(source, destination).map_err(|error| match error {
        SwapError::Failed(error) => file_update_error(error),
        SwapError::RestoreFailed { error, restore } => AppError::Update(format!(
            "cannot install {}: {error}; restoring its previous version also failed: {restore}; rerun the installer to repair the complete bundle",
            destination.display()
        )),
    })
}

enum SwapError {
    /// The swap failed and `destination` holds what it held before.
    Failed(io::Error),
    /// Installing the new file failed and putting the previous one back
    /// failed too, so `destination` is missing.
    RestoreFailed {
        error: io::Error,
        restore: io::Error,
    },
}

/// Installs a copy of `source` at `destination` through a staged copy and a
/// backup beside it, keeping the backup for the caller to remove or restore.
fn swap(source: &Path, destination: &Path) -> Result<FileSwap, SwapError> {
    let parent = destination.parent().ok_or_else(|| {
        SwapError::Failed(io::Error::other(format!(
            "{} has no installation directory",
            destination.display()
        )))
    })?;
    let name = destination
        .file_name()
        .unwrap_or_default()
        .to_string_lossy();
    let staged = parent.join(format!(".{name}.update-new"));
    let backup = parent.join(format!(".{name}.update-old"));
    remove_file_if_present(&staged).map_err(SwapError::Failed)?;
    remove_file_if_present(&backup).map_err(SwapError::Failed)?;
    copy(source, &staged).map_err(SwapError::Failed)?;
    let had_previous = destination.is_file();
    if had_previous {
        rename(destination, &backup).map_err(SwapError::Failed)?;
    }
    if let Err(error) = rename(&staged, destination) {
        if had_previous {
            if let Err(restore) = rename(&backup, destination) {
                return Err(SwapError::RestoreFailed { error, restore });
            }
        }
        return Err(SwapError::Failed(error));
    }
    Ok(FileSwap {
        destination: destination.to_path_buf(),
        backup,
        had_previous,
    })
}

fn rollback_files(swaps: &mut Vec<FileSwap>) -> io::Result<()> {
    let mut failure = None;
    for swap in swaps.drain(..).rev() {
        if let Err(error) = remove_file_if_present(&swap.destination) {
            failure.get_or_insert(error);
            continue;
        }
        if swap.had_previous {
            if let Err(error) = rename(&swap.backup, &swap.destination) {
                failure.get_or_insert(error);
            }
        }
    }
    failure.map_or(Ok(()), Err)
}

fn with_rollback(original: AppError, rollback: io::Result<()>) -> AppError {
    match rollback {
        Ok(()) => original,
        Err(error) => AppError::Update(format!(
            "{original}; restoring the previous companion bundle also failed: {error}; rerun the installer to repair the complete bundle"
        )),
    }
}

fn remove_file_if_present(path: &Path) -> io::Result<()> {
    match remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn file_update_error(error: io::Error) -> AppError {
    AppError::Update(format!(
        "cannot replace the installed companion bundle: {error}; rerun 'agentenv update --force' after repairing directory permissions"
    ))
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
    rename(destination, &retired)?;
    if let Err(error) = rename(&staged, destination) {
        // Put the previous skill back so the install is never left empty.
        let _ = rename(&retired, destination);
        return Err(error);
    }
    remove_dir_all(&retired)
}

fn remove_dir_if_present(path: &Path) -> io::Result<()> {
    match remove_dir_all(path) {
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
            copy(&entry.path(), &target)?;
        }
    }
    Ok(())
}

/// How many times a file operation runs while it fails with a transient lock.
const TRANSIENT_ATTEMPTS: u32 = 8;

/// Runs `operation`, retrying with growing pauses while Windows reports that
/// another process holds the file. Anti-malware scanners open a freshly
/// written executable for a moment, and a rename or replacement attempted
/// then fails with a sharing, lock, or access violation that clears on its
/// own. The pauses total about three and a half seconds before the last
/// attempt.
fn retry_transient<T>(mut operation: impl FnMut() -> io::Result<T>) -> io::Result<T> {
    let mut pause = Duration::from_millis(50);
    for _ in 1..TRANSIENT_ATTEMPTS {
        match operation() {
            Err(error) if is_transient(&error) => {
                thread::sleep(pause);
                pause = (pause * 2).min(Duration::from_secs(1));
            }
            result => return result,
        }
    }
    operation()
}

/// `ERROR_ACCESS_DENIED`, `ERROR_SHARING_VIOLATION`, and `ERROR_LOCK_VIOLATION`.
fn is_transient(error: &io::Error) -> bool {
    cfg!(windows) && matches!(error.raw_os_error(), Some(5 | 32 | 33))
}

fn copy(source: &Path, destination: &Path) -> io::Result<u64> {
    retry_transient(|| fs::copy(source, destination))
}

fn rename(from: &Path, to: &Path) -> io::Result<()> {
    retry_transient(|| fs::rename(from, to))
}

fn remove_file(path: &Path) -> io::Result<()> {
    retry_transient(|| fs::remove_file(path))
}

fn remove_dir_all(path: &Path) -> io::Result<()> {
    retry_transient(|| fs::remove_dir_all(path))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::update::release::Extracted;

    #[test]
    fn only_windows_lock_errors_are_retried() {
        let mut calls = 0;
        let result = retry_transient(|| {
            calls += 1;
            if calls < 3 {
                Err(io::Error::from_raw_os_error(32))
            } else {
                Ok(calls)
            }
        });
        if cfg!(windows) {
            assert_eq!(result.expect("the lock clears"), 3);
        } else {
            assert_eq!(result.expect_err("no retry").raw_os_error(), Some(32));
            assert_eq!(calls, 1);
        }

        let mut calls = 0;
        let missing = retry_transient(|| {
            calls += 1;
            Err::<(), _>(io::Error::from(io::ErrorKind::NotFound))
        });
        assert_eq!(
            missing.expect_err("permanent").kind(),
            io::ErrorKind::NotFound
        );
        assert_eq!(calls, 1, "a permanent error is not retried");
    }

    #[test]
    fn main_replacement_failure_restores_both_companions() {
        let directory = tempfile::tempdir().expect("temporary install");
        let installed = directory.path().join("installed");
        let extracted_dir = directory.path().join("extracted");
        fs::create_dir_all(&installed).expect("installed directory");
        fs::create_dir_all(&extracted_dir).expect("extracted directory");
        let installation = Installation {
            binary: installed.join("agentenv"),
            sudo_helper: installed.join("agentenv-sudo-helper"),
            ssh_askpass: installed.join("agentenv-ssh-askpass"),
            skills: Vec::new(),
        };
        fs::write(&installation.sudo_helper, b"old sudo").expect("old sudo helper");
        fs::write(&installation.ssh_askpass, b"old ssh").expect("old SSH helper");
        let extracted = Extracted {
            binary: extracted_dir.join("agentenv"),
            sudo_helper: extracted_dir.join("agentenv-sudo-helper"),
            ssh_askpass: extracted_dir.join("agentenv-ssh-askpass"),
            skill: None,
        };
        fs::write(&extracted.sudo_helper, b"new sudo").expect("new sudo helper");
        fs::write(&extracted.ssh_askpass, b"new ssh").expect("new SSH helper");

        let error = replace_bundle_with(&extracted, &installation, || {
            Err(AppError::Update("injected main swap failure".to_owned()))
        })
        .expect_err("main replacement fails");

        assert!(error.to_string().contains("injected main swap failure"));
        assert_eq!(fs::read(&installation.sudo_helper).unwrap(), b"old sudo");
        assert_eq!(fs::read(&installation.ssh_askpass).unwrap(), b"old ssh");
    }
}
