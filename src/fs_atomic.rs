//! Owner-only file writes.
//!
//! Both helpers write through a file that only the current user can read on
//! Unix from the moment it exists, and flush its contents to disk. `replace`
//! is atomic: it writes a sibling temporary file and renames it over the
//! target, so a failure at any step leaves the previous file byte-intact and
//! removes the temporary file. `create_new` writes in place under the final
//! name, which must not exist yet.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// The step of a write that failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    /// The target path has no parent directory to hold a temporary file.
    NoParent,
    /// The target path has no UTF-8 file name to derive a temporary name from.
    NoFileName,
    /// No owner-only file could be created.
    Create,
    /// The contents could not be written.
    Write,
    /// The requested permission bits could not be applied.
    Permissions,
    /// The contents could not be flushed to disk.
    Flush,
    /// The temporary file could not be renamed over the target.
    Rename,
}

/// A failed write and the I/O error behind it.
#[derive(Debug)]
pub struct WriteError {
    pub stage: Stage,
    pub error: io::Error,
}

impl WriteError {
    fn at(stage: Stage) -> impl FnOnce(io::Error) -> Self {
        move |error| Self { stage, error }
    }
}

/// How many temporary names a replacement tries before giving up. A
/// collision means a leftover temporary file from an interrupted write holds
/// the name.
const TEMP_NAME_ATTEMPTS: u32 = 16;

/// Distinguishes temporary files created within one process.
static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Replaces `path` with `bytes`. The temporary file is created with `0600`
/// bits on Unix; `mode`, when given, is applied to it before the rename, so
/// a caller can carry over the replaced file's permissions. The directory is
/// synced after the rename on a best-effort basis: the renamed file's
/// contents are already durable, and a failed directory sync can only delay
/// the rename's metadata reaching disk after a crash.
pub fn replace(path: &Path, bytes: &[u8], mode: Option<u32>) -> Result<(), WriteError> {
    let unusable = |stage: Stage| WriteError {
        stage,
        error: io::ErrorKind::InvalidInput.into(),
    };
    let directory = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .ok_or_else(|| unusable(Stage::NoParent))?;
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| unusable(Stage::NoFileName))?;
    let (temporary, file) = create_temporary(directory, file_name)?;
    let result = write_and_rename(file, &temporary, path, bytes, mode);
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    } else {
        sync_directory(directory);
    }
    result
}

/// Creates `path`, which must not exist, with exactly `0600` bits on Unix,
/// regardless of the umask, and writes and flushes `bytes`.
pub fn create_new(path: &Path, bytes: &[u8]) -> Result<(), WriteError> {
    let mut file = owner_only()
        .open(path)
        .map_err(WriteError::at(Stage::Create))?;
    set_mode(&file, 0o600)?;
    file.write_all(bytes)
        .map_err(WriteError::at(Stage::Write))?;
    file.sync_all().map_err(WriteError::at(Stage::Flush))?;
    drop(file);
    if let Some(directory) = path.parent() {
        sync_directory(directory);
    }
    Ok(())
}

fn create_temporary(directory: &Path, file_name: &str) -> Result<(PathBuf, File), WriteError> {
    let mut last = None;
    for _ in 0..TEMP_NAME_ATTEMPTS {
        let ordinal = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let temporary = directory.join(format!(
            ".{file_name}.agentenv-tmp-{}-{ordinal}",
            std::process::id()
        ));
        match owner_only().open(&temporary) {
            Ok(file) => return Ok((temporary, file)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => last = Some(error),
            Err(error) => return Err(WriteError::at(Stage::Create)(error)),
        }
    }
    Err(WriteError::at(Stage::Create)(
        last.expect("every attempt failed with AlreadyExists"),
    ))
}

fn write_and_rename(
    mut file: File,
    temporary: &Path,
    path: &Path,
    bytes: &[u8],
    mode: Option<u32>,
) -> Result<(), WriteError> {
    file.write_all(bytes)
        .map_err(WriteError::at(Stage::Write))?;
    if let Some(mode) = mode {
        set_mode(&file, mode)?;
    }
    file.sync_all().map_err(WriteError::at(Stage::Flush))?;
    drop(file);
    fs::rename(temporary, path).map_err(WriteError::at(Stage::Rename))
}

/// Create-new write options; on Unix the file is created owner-only.
fn owner_only() -> OpenOptions {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
}

#[cfg(unix)]
fn set_mode(file: &File, mode: u32) -> Result<(), WriteError> {
    use std::os::unix::fs::PermissionsExt;
    file.set_permissions(fs::Permissions::from_mode(mode))
        .map_err(WriteError::at(Stage::Permissions))
}

/// Windows files carry no Unix mode bits; the profile directory's ACL
/// governs access.
#[cfg(not(unix))]
fn set_mode(_file: &File, _mode: u32) -> Result<(), WriteError> {
    Ok(())
}

#[cfg(unix)]
fn sync_directory(directory: &Path) {
    if let Ok(directory) = File::open(directory) {
        let _ = directory.sync_all();
    }
}

#[cfg(not(unix))]
fn sync_directory(_directory: &Path) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_failed_replace_keeps_the_target_and_removes_the_temporary_file() {
        let directory = tempfile::tempdir().expect("temporary directory");
        // A non-empty directory cannot be replaced by a file, so the rename
        // fails after the temporary file was written.
        let target = directory.path().join("target");
        fs::create_dir(&target).expect("target directory");
        fs::write(target.join("kept"), b"previous").expect("target content");

        let failure = replace(&target, b"new", None).expect_err("rename must fail");

        assert_eq!(failure.stage, Stage::Rename);
        assert_eq!(fs::read(target.join("kept")).expect("kept"), b"previous");
        let names: Vec<_> = fs::read_dir(directory.path())
            .expect("listing")
            .map(|entry| entry.expect("entry").file_name())
            .collect();
        assert_eq!(names, ["target"]);
    }
}
