//! Per-user installation of the credential helper and signed release bundles.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::error::AppError;

use super::native;
use super::protocol::{IDENTITY, VERSION};
use super::signing::{self, LocalIdentity};

pub const HELPER_NAME: &str = "agentenv-keychain-helper";
pub const HELPER_IDENTIFIER: &str = "io.github.ii999.agentenv.keychain-helper";
const RECEIPT: &str = "helper.toml";
const ROLES: [(&str, &str); 3] = [
    ("agentenv", "io.github.ii999.agentenv"),
    (
        "agentenv-sudo-helper",
        "io.github.ii999.agentenv.sudo-helper",
    ),
    (
        "agentenv-ssh-askpass",
        "io.github.ii999.agentenv.ssh-askpass",
    ),
];

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Receipt {
    version: u32,
    protocol: u32,
    certificate_sha1: String,
    sha256: String,
}

#[derive(Debug, Serialize)]
pub struct SetupReport {
    pub helper: PathBuf,
    pub protocol: u32,
    pub certificate_sha1: String,
    pub reused: bool,
    pub replaced: bool,
}

/// The shared lock remains held through credential exchange. An explicit
/// helper replacement cannot split verification from execution.
pub(crate) struct InstalledHelper {
    pub path: PathBuf,
    _lock: File,
}

pub fn state_directory() -> Result<PathBuf, AppError> {
    let data = match std::env::var_os("XDG_DATA_HOME").filter(|value| !value.is_empty()) {
        Some(path) => PathBuf::from(path),
        None => std::env::var_os("HOME")
            .filter(|value| !value.is_empty())
            .map(|home| PathBuf::from(home).join(".local/share"))
            .ok_or_else(|| {
                error("cannot locate local Keychain installation state; set HOME or XDG_DATA_HOME")
            })?,
    };
    if !data.is_absolute() {
        return Err(error("the Keychain installation data directory must be absolute; set HOME or XDG_DATA_HOME to an absolute path"));
    }
    Ok(data.join("agentenv/macos-keychain"))
}

fn checked_directory(create: bool) -> Result<PathBuf, AppError> {
    let directory = state_directory()?;
    if create {
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&directory)
            .map_err(|cause| error(format!("cannot create {}: {cause}", directory.display())))?;
    }
    let metadata = fs::symlink_metadata(&directory).map_err(|cause| {
        error(format!(
            "cannot inspect {}: {cause}; run 'agentenv keychain setup'",
            directory.display()
        ))
    })?;
    if !metadata.is_dir()
        || metadata.file_type().is_symlink()
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.mode() & 0o077 != 0
    {
        return Err(error(format!(
            "{} must be a user-owned directory with mode 0700; repair its ownership and permissions before retrying",
            directory.display()
        )));
    }
    directory
        .canonicalize()
        .map_err(|cause| error(format!("cannot resolve {}: {cause}", directory.display())))
}

fn lock(directory: &Path, exclusive: bool) -> Result<File, AppError> {
    let path = directory.join("installation.lock");
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(exclusive)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(&path)
        .map_err(|cause| error(format!("cannot open {}: {cause}", path.display())))?;
    let metadata = file
        .metadata()
        .map_err(|cause| error(format!("cannot inspect installation lock: {cause}")))?;
    if !metadata.is_file()
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.mode() & 0o077 != 0
    {
        return Err(error(
            "the local Keychain installation lock must be a user-owned file with mode 0600",
        ));
    }
    let operation = if exclusive {
        libc::LOCK_EX
    } else {
        libc::LOCK_SH
    };
    loop {
        if unsafe { libc::flock(file.as_raw_fd(), operation) } == 0 {
            return Ok(file);
        }
        let cause = io::Error::last_os_error();
        if cause.kind() != io::ErrorKind::Interrupted {
            return Err(error(format!(
                "cannot lock the local Keychain installation: {cause}"
            )));
        }
    }
}

fn read_receipt(directory: &Path) -> Result<Option<Receipt>, AppError> {
    let path = directory.join(RECEIPT);
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(cause) if cause.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(cause) => return Err(error(format!("cannot read {}: {cause}", path.display()))),
    };
    let receipt: Receipt = toml::from_str(&text).map_err(|_| {
        error(format!(
            "{} is not a valid credential helper receipt; restore its saved copy before retrying",
            path.display()
        ))
    })?;
    let hex = |value: &str, length| {
        value.len() == length && value.bytes().all(|byte| byte.is_ascii_hexdigit())
    };
    if receipt.version != 1
        || receipt.protocol == 0
        || !hex(&receipt.certificate_sha1, 40)
        || !hex(&receipt.sha256, 64)
    {
        return Err(error("the credential helper receipt has an unsupported version or invalid fingerprint; use a compatible agentenv release or restore the receipt"));
    }
    Ok(Some(receipt))
}

fn verify_helper(directory: &Path, receipt: &Receipt) -> Result<PathBuf, AppError> {
    if receipt.protocol != VERSION {
        return Err(error("the installed credential helper uses an unsupported protocol; run 'agentenv keychain setup --replace-helper' with this release and authorize existing credentials again"));
    }
    let path = directory.join(HELPER_NAME);
    let metadata = fs::symlink_metadata(&path)
        .map_err(|cause| error(format!("cannot inspect {}: {cause}; use 'agentenv keychain setup --replace-helper' for an intentional replacement", path.display())))?;
    if !metadata.is_file()
        || metadata.file_type().is_symlink()
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.mode() & 0o6022 != 0
        || metadata.mode() & 0o100 == 0
    {
        return Err(error("the credential helper must be a user-owned executable without set-id or group/other write permissions"));
    }
    if sha256(&path)? != receipt.sha256 {
        return Err(error("the credential helper differs from its installation receipt; restore the installed helper or use 'agentenv keychain setup --replace-helper' for an intentional replacement"));
    }
    signing::verify_signed(&path, HELPER_IDENTIFIER, &receipt.certificate_sha1).map_err(
        |cause| {
            error(format!(
                "credential helper signature verification failed: {cause}"
            ))
        },
    )?;
    Ok(path)
}

pub(crate) fn installed_helper() -> Result<InstalledHelper, AppError> {
    let directory = checked_directory(false)?;
    let lock = lock(&directory, false)?;
    let receipt = read_receipt(&directory)?.ok_or_else(|| {
        error("the credential helper is not installed; run 'agentenv keychain setup'")
    })?;
    let path = verify_helper(&directory, &receipt)?;
    Ok(InstalledHelper { path, _lock: lock })
}

pub(crate) fn has_installation(directory: &Path) -> bool {
    [HELPER_NAME, RECEIPT]
        .iter()
        .any(|name| fs::symlink_metadata(directory.join(name)).is_ok())
}

/// Inspect installation state without creating a directory, lock, or identity.
pub fn status() -> Result<Option<SetupReport>, AppError> {
    let path = state_directory()?;
    if !path
        .try_exists()
        .map_err(|cause| error(format!("cannot inspect helper state: {cause}")))?
    {
        return Ok(None);
    }
    let directory = checked_directory(false)?;
    if !directory
        .join(RECEIPT)
        .try_exists()
        .map_err(|cause| error(format!("cannot inspect helper receipt: {cause}")))?
        && !directory
            .join(HELPER_NAME)
            .try_exists()
            .map_err(|cause| error(format!("cannot inspect helper executable: {cause}")))?
    {
        return Ok(None);
    }
    let _lock = lock(&directory, false)?;
    let receipt = read_receipt(&directory)?.ok_or_else(|| error("the credential helper has no receipt; inspect the installation before using '--replace-helper'"))?;
    let path = verify_helper(&directory, &receipt)?;
    Ok(Some(report(path, &receipt, true, false)))
}

pub fn setup(source: Option<&Path>, replace: bool) -> Result<SetupReport, AppError> {
    let directory_path = checked_directory(true)?;
    let directory = directory_path.as_path();
    let _lock = lock(directory, true)?;
    if let Some(receipt) = read_receipt(directory)? {
        if !replace {
            let path = verify_helper(directory, &receipt)?;
            return Ok(report(path, &receipt, true, false));
        }
    }
    if has_installation(directory) && !replace {
        return Err(error("the credential helper installation is incomplete; inspect it with 'agentenv keychain setup' before an intentional '--replace-helper' repair"));
    }
    let source = helper_source(source)?;
    let staged = stage_helper(directory, source)?;
    let identity = ensure_identity(directory)?;
    install_helper(directory, &identity, staged, replace)
}

/// The caller has already verified the release checksum and executable
/// versions. All files here are staged copies, not currently running inodes.
pub fn prepare_bundle(directory: &Path, source: Option<&Path>) -> Result<SetupReport, AppError> {
    let state = checked_directory(true)?;
    let _lock = lock(&state, true)?;
    let existing = read_receipt(&state)?;
    if let Some(receipt) = &existing {
        verify_helper(&state, receipt)?;
    }
    let staged = if existing.is_none() {
        let source = helper_source(source)?;
        Some(stage_helper(&state, source)?)
    } else {
        None
    };
    let identity = ensure_identity(&state)?;
    if let Some(receipt) = &existing {
        if !receipt
            .certificate_sha1
            .eq_ignore_ascii_case(identity.certificate_sha1())
        {
            return Err(error("the signing identity differs from the credential helper receipt; restore the original identity before updating"));
        }
    }
    for (name, identifier) in ROLES {
        let path = directory.join(name);
        let metadata = fs::symlink_metadata(&path).map_err(|cause| {
            error(format!(
                "cannot inspect staged executable {}: {cause}",
                path.display()
            ))
        })?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err(error(format!(
                "{} must be a regular staged executable",
                path.display()
            )));
        }
        identity
            .sign(&path, identifier)
            .map_err(|cause| error(format!("cannot sign staged {name}: {cause}")))?;
        identity
            .verify(&path, identifier)
            .map_err(|cause| error(format!("cannot verify staged {name}: {cause}")))?;
    }
    let candidate = directory.join(HELPER_NAME);
    if candidate.is_file() {
        let metadata = fs::symlink_metadata(&candidate).map_err(|cause| {
            error(format!(
                "cannot inspect the staged credential helper candidate: {cause}"
            ))
        })?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err(error(
                "the staged credential helper candidate must be a regular file",
            ));
        }
        identity
            .sign(&candidate, HELPER_IDENTIFIER)
            .map_err(|cause| {
                error(format!(
                    "cannot sign the staged credential helper candidate: {cause}"
                ))
            })?;
    }
    if let Some(receipt) = existing {
        Ok(report(state.join(HELPER_NAME), &receipt, true, false))
    } else {
        install_helper(
            &state,
            &identity,
            staged.ok_or_else(|| error("the release has no credential helper"))?,
            false,
        )
    }
}

fn ensure_identity(directory: &Path) -> Result<LocalIdentity, AppError> {
    let mut keychain = native::open_keychain().map_err(AppError::Keychain)?;
    native::unlock(&mut keychain).map_err(AppError::Keychain)?;
    signing::ensure(directory, &keychain).map_err(|cause| {
        error(format!(
            "cannot prepare the local signing identity: {cause}"
        ))
    })
}

fn helper_source(source: Option<&Path>) -> Result<PathBuf, AppError> {
    if let Some(source) = source {
        return Ok(source.to_path_buf());
    }
    let executable = std::env::current_exe()
        .map_err(|cause| error(format!("cannot locate this executable: {cause}")))?;
    let directory = executable
        .parent()
        .ok_or_else(|| error("this executable has no parent directory"))?;
    Ok(directory.join(HELPER_NAME))
}

struct StagedHelper {
    directory: tempfile::TempDir,
    executable: PathBuf,
}

fn stage_helper(directory: &Path, source: PathBuf) -> Result<StagedHelper, AppError> {
    let mut input = OpenOptions::new().read(true).custom_flags(libc::O_NOFOLLOW).open(&source)
        .map_err(|cause| error(format!("cannot read credential helper source {}: {cause}; repair the selected executable or install a complete release", source.display())))?;
    if !input
        .metadata()
        .map_err(|cause| error(format!("cannot inspect the helper source: {cause}")))?
        .is_file()
    {
        return Err(error(
            "the credential helper source must be a regular executable",
        ));
    }
    let staging = tempfile::Builder::new()
        .prefix(".helper-stage-")
        .tempdir_in(directory)
        .map_err(|cause| error(format!("cannot stage the credential helper: {cause}")))?;
    let staged = staging.path().join(HELPER_NAME);
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o700)
        .open(&staged)
        .map_err(|cause| error(format!("cannot create the staged helper: {cause}")))?;
    io::copy(&mut input, &mut output)
        .map_err(|cause| error(format!("cannot copy the credential helper: {cause}")))?;
    drop(output);
    let result = Command::new(&staged)
        .arg("--identity")
        .stdin(Stdio::null())
        .output()
        .map_err(|cause| {
            error(format!(
                "cannot run the credential helper identity check: {cause}"
            ))
        })?;
    if !result.status.success()
        || result.stdout != format!("{IDENTITY}\n").as_bytes()
        || !result.stderr.is_empty()
    {
        return Err(error("the release credential helper has an unsupported identity; install a complete compatible release"));
    }
    Ok(StagedHelper {
        directory: staging,
        executable: staged,
    })
}

fn install_helper(
    directory: &Path,
    identity: &LocalIdentity,
    staged: StagedHelper,
    replace: bool,
) -> Result<SetupReport, AppError> {
    let path = directory.join(HELPER_NAME);
    let had_helper = path.exists();
    if had_helper && !replace {
        return Err(error("a credential helper exists without a usable receipt; inspect it before using 'agentenv keychain setup --replace-helper'"));
    }
    let StagedHelper {
        directory: staging,
        executable: staged,
    } = staged;
    identity
        .sign(&staged, HELPER_IDENTIFIER)
        .map_err(|cause| error(format!("cannot sign the credential helper: {cause}")))?;
    identity
        .verify(&staged, HELPER_IDENTIFIER)
        .map_err(|cause| error(format!("cannot verify the credential helper: {cause}")))?;
    let receipt = Receipt {
        version: 1,
        protocol: VERSION,
        certificate_sha1: identity.certificate_sha1().to_owned(),
        sha256: sha256(&staged)?,
    };
    let encoded = toml::to_string(&receipt)
        .map_err(|cause| error(format!("cannot encode the helper receipt: {cause}")))?;
    let previous = staging.path().join("previous-helper");
    if had_helper {
        fs::copy(&path, &previous).map_err(|cause| {
            error(format!(
                "cannot retain the previous helper for rollback: {cause}"
            ))
        })?;
    }
    fs::rename(&staged, &path)
        .map_err(|cause| error(format!("cannot install the credential helper: {cause}")))?;
    if let Err(cause) =
        crate::fs_atomic::replace(&directory.join(RECEIPT), encoded.as_bytes(), Some(0o600))
    {
        let rollback = if had_helper {
            fs::rename(&previous, &path)
        } else {
            fs::remove_file(&path)
        };
        return Err(error(match rollback {
            Ok(()) => format!("cannot save the credential helper receipt: {}; the previous helper state was restored", cause.error),
            Err(rollback) => format!("cannot save the credential helper receipt: {}; restoring the previous helper also failed: {rollback}; repair with 'agentenv keychain setup --replace-helper'", cause.error),
        }));
    }
    Ok(report(path, &receipt, false, had_helper))
}

fn report(path: PathBuf, receipt: &Receipt, reused: bool, replaced: bool) -> SetupReport {
    SetupReport {
        helper: path,
        protocol: receipt.protocol,
        certificate_sha1: receipt.certificate_sha1.clone(),
        reused,
        replaced,
    }
}

fn sha256(path: &Path) -> Result<String, AppError> {
    let mut source = File::open(path)
        .map_err(|cause| error(format!("cannot open {}: {cause}", path.display())))?;
    let mut hasher = Sha256::new();
    let mut bytes = [0_u8; 64 * 1024];
    loop {
        let count = source
            .read(&mut bytes)
            .map_err(|cause| error(format!("cannot hash {}: {cause}", path.display())))?;
        if count == 0 {
            break;
        }
        hasher.update(&bytes[..count]);
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

fn error(message: impl Into<String>) -> AppError {
    AppError::Update(message.into())
}
