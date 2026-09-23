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
    let parent = destination.parent().ok_or_else(|| {
        AppError::Update(format!(
            "{} has no installation directory",
            destination.display()
        ))
    })?;
    let name = destination
        .file_name()
        .unwrap_or_default()
        .to_string_lossy();
    let staged = parent.join(format!(".{name}.update-new"));
    let backup = parent.join(format!(".{name}.update-old"));
    remove_file_if_present(&staged).map_err(file_update_error)?;
    remove_file_if_present(&backup).map_err(file_update_error)?;
    fs::copy(source, &staged).map_err(file_update_error)?;
    let had_previous = destination.is_file();
    if had_previous {
        fs::rename(destination, &backup).map_err(file_update_error)?;
    }
    if let Err(error) = fs::rename(&staged, destination) {
        if had_previous {
            fs::rename(&backup, destination).map_err(|restore| {
                AppError::Update(format!(
                    "cannot install {}: {error}; restoring its previous version also failed: {restore}; rerun the installer to repair the complete bundle",
                    destination.display()
                ))
            })?;
        }
        return Err(file_update_error(error));
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
            if let Err(error) = fs::rename(&swap.backup, &swap.destination) {
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
    match fs::remove_file(path) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::update::release::Extracted;

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
