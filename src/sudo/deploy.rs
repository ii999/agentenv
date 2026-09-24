//! Explicit installation of the remote sudo helper over the target's own SSH
//! route, as designed in `docs/design/helper-deployment.md`.
//!
//! Everything here is deterministic apart from two seams: a [`SessionRunner`]
//! runs one remote command over the prepared route, and a [`HelperSource`]
//! supplies the helper bytes for the destination. The module validates the
//! configured path, renders fixed remote command templates, parses bounded
//! preflight output strictly, decides between install, up-to-date and refusal,
//! and drives the sessions in order. It never resolves a credential and never
//! runs during ordinary execution or `--check`.

use std::future::Future;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::error::AppError;

/// Largest helper upload accepted from any source.
pub const HELPER_UPLOAD_LIMIT: usize = 64 * 1024 * 1024;
/// Largest preflight or install stdout read from the destination.
pub const SESSION_OUTPUT_LIMIT: usize = 4096;
/// Oldest glibc the published Linux helper loads on.
pub const GLIBC_FLOOR: (u32, u32) = (2, 28);
const MAX_HELPER_PATH_BYTES: usize = 1024;
const MAX_PREFLIGHT_LINE_BYTES: usize = 256;

/// The identity the destination must report for this client build.
pub fn expected_identity() -> String {
    format!(
        "agentenv-sudo-helper {} {}",
        super::PROTOCOL_VERSION,
        env!("CARGO_PKG_VERSION")
    )
}

/// One deployment failure class. Every failure leaves the configured path
/// holding the previous helper, the newly verified helper (only when the
/// install command had already renamed it), or nothing, so rerunning is
/// always safe and `--check` tells which.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    InvalidHelperPath,
    PreflightUnparseable,
    DestinationUnsupported,
    GlibcTooOld,
    PathOccupied,
    SourceUnavailable,
    ChecksumMismatch,
    UploadFailed,
    IdentityMismatch,
    ReplaceFailed,
    CheckFailed,
    /// A session could not be started, kept within its deadline, or read.
    SessionFailed,
    Cancelled,
}

impl Reason {
    pub fn code(self) -> &'static str {
        match self {
            Self::InvalidHelperPath => "invalid-helper-path",
            Self::PreflightUnparseable => "preflight-unparseable",
            Self::DestinationUnsupported => "destination-unsupported",
            Self::GlibcTooOld => "glibc-too-old",
            Self::PathOccupied => "path-occupied",
            Self::SourceUnavailable => "source-unavailable",
            Self::ChecksumMismatch => "checksum-mismatch",
            Self::UploadFailed => "upload-failed",
            Self::IdentityMismatch => "identity-mismatch",
            Self::ReplaceFailed => "replace-failed",
            Self::CheckFailed => "check-failed",
            Self::SessionFailed => "session-failed",
            Self::Cancelled => "cancelled",
        }
    }

    pub fn error(self, detail: impl AsRef<str>) -> AppError {
        AppError::SudoExecution(format!(
            "helper-deploy-{}: {}; helper_path holds the previous helper, the newly verified one, or nothing, so rerunning is safe",
            self.code(),
            detail.as_ref()
        ))
    }
}

/// Accepts the configured remote path only in the grammar both remote command
/// templates take safely as a positional parameter: absolute, at most 1024
/// bytes, characters from `[A-Za-z0-9._/+-]`, no empty, `.` or `..`
/// component, no trailing slash, and the file name `agentenv-sudo-helper`
/// that the serving helper relaunches beside itself.
pub fn validate_helper_path(path: &Path) -> Result<&str, AppError> {
    match helper_path_problem(path) {
        None => Ok(path
            .to_str()
            .expect("a path without a grammar problem is UTF-8")),
        Some(detail) => {
            Err(Reason::InvalidHelperPath.error(format!("{HELPER_PATH_GRAMMAR} ({detail})")))
        }
    }
}

/// The deployment grammar in one sentence, for refusals and warnings.
pub const HELPER_PATH_GRAMMAR: &str = "helper_path must be an absolute path of at most 1024 bytes using only letters, digits, '.', '_', '/', '+' and '-', ending in /agentenv-sudo-helper";

/// The file name the helper must carry: while serving it relaunches the
/// `agentenv-sudo-helper` beside itself for the sudo side of the session.
pub const HELPER_FILE_NAME: &str = "agentenv-sudo-helper";

/// Why `path` falls outside the deployment grammar, or `None` when it fits.
pub fn helper_path_problem(path: &Path) -> Option<&'static str> {
    let Some(text) = path.to_str() else {
        return Some("not valid UTF-8");
    };
    if !text.starts_with('/') {
        return Some("not absolute");
    }
    if text.len() > MAX_HELPER_PATH_BYTES {
        return Some("too long");
    }
    if !text.bytes().all(|byte| {
        byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'/' | b'+' | b'-')
    }) {
        return Some("unsupported character");
    }
    if text.ends_with('/') {
        return Some("names a directory");
    }
    if text
        .split('/')
        .skip(1)
        .any(|component| component.is_empty() || component == "." || component == "..")
    {
        return Some("empty, '.' or '..' path component");
    }
    if text.rsplit('/').next() != Some(HELPER_FILE_NAME) {
        return Some("the file name is not agentenv-sudo-helper");
    }
    None
}

// Both scripts run under `sh -c` with the validated path as `$1`, so the
// login shell only parses one single-quoted literal and one quoted path. They
// contain no single quote, backslash or `!`, which keeps them literal under
// csh-family and fish login shells as well as POSIX shells.
// The fourth preflight line is `absent`, `occupied`, or `helper ` followed by
// whatever the existing executable printed, so a foreign program can never
// forge the two bare keywords.
const PREFLIGHT_SCRIPT: &str = "p=$1; uname -s; uname -m; getconf GNU_LIBC_VERSION 2>/dev/null || echo none; \
if [ -L \"$p\" ]; then echo occupied; \
elif [ -e \"$p\" ]; then if [ -f \"$p\" ] && [ -x \"$p\" ]; then i=$(\"$p\" --identity 2>/dev/null) && echo \"helper $i\" || echo occupied; else echo occupied; fi; \
else echo absent; fi";

// `$2` is the expected identity and `$3` the byte count. The temporary file
// carries the shell's pid, so two deployments running at once never move
// each other's upload: whichever loses its temporary file to the other's
// stale-file cleanup fails, and the path only ever receives a verified file.
// The temporary file is created with noclobber, its size and identity are checked on the
// destination, the path is checked again immediately before the rename (a
// directory or symlink appearing during the upload would otherwise receive
// the file), and the result must be a regular file; every failure removes
// the temporary file. The identity is echoed last so the client can compare
// it too.
const INSTALL_SCRIPT: &str = "p=$1; e=$2; n=$3; t=\"$p.agentenv-new.$$\"; umask 077; \
rm -f \"$p\".agentenv-new* || exit 21; \
d=$(dirname \"$p\") || exit 22; mkdir -p \"$d\" || exit 22; \
if [ -L \"$p\" ]; then exit 23; fi; \
if [ -e \"$p\" ]; then if [ -f \"$p\" ]; then :; else exit 23; fi; fi; \
set -C; cat > \"$t\" || { rm -f \"$t\"; exit 24; }; set +C; \
c=$(wc -c < \"$t\" | tr -d \" \") || { rm -f \"$t\"; exit 24; }; \
[ \"$c\" = \"$n\" ] || { rm -f \"$t\"; exit 24; }; \
chmod 0755 \"$t\" || { rm -f \"$t\"; exit 25; }; \
if [ -f \"$t\" ]; then :; else exit 24; fi; \
i=$(\"$t\" --identity 2>/dev/null) || { rm -f \"$t\"; exit 26; }; \
[ \"$i\" = \"$e\" ] || { rm -f \"$t\"; exit 26; }; \
if [ -L \"$p\" ]; then rm -f \"$t\"; exit 23; fi; \
if [ -e \"$p\" ]; then if [ -f \"$p\" ]; then :; else rm -f \"$t\"; exit 23; fi; fi; \
mv -f \"$t\" \"$p\" || { rm -f \"$t\"; exit 27; }; \
if [ -L \"$p\" ]; then exit 27; fi; \
if [ -f \"$p\" ]; then :; else exit 27; fi; \
echo \"$i\"";

const INSTALL_STATUS_STALE_TEMP: i32 = 21;
/// ssh's own status: the connection ended or the remote shell was killed
/// after the install command had started.
const INSTALL_STATUS_SESSION_LOST: i32 = 255;
const INSTALL_STATUS_DIRECTORY: i32 = 22;
const INSTALL_STATUS_OCCUPIED: i32 = 23;
const INSTALL_STATUS_WRITE: i32 = 24;
const INSTALL_STATUS_MODE: i32 = 25;
const INSTALL_STATUS_IDENTITY: i32 = 26;
const INSTALL_STATUS_RENAME: i32 = 27;

fn remote_command(script: &str, name: &str, parameters: &[&str]) -> String {
    debug_assert!(!script.contains(['\'', '\\', '!']));
    debug_assert!(parameters
        .iter()
        .all(|parameter| !parameter.contains(['\'', '\\', '!', '\n'])));
    let mut command = format!("sh -c '{script}' {name}");
    for parameter in parameters {
        command.push_str(" '");
        command.push_str(parameter);
        command.push('\'');
    }
    command
}

/// The remote command that reports platform, machine, glibc and the current
/// state of the configured path without writing anything.
pub fn preflight_command(helper_path: &str) -> String {
    remote_command(PREFLIGHT_SCRIPT, "agentenv-preflight", &[helper_path])
}

/// The remote command that reads exactly `byte_count` helper bytes from stdin
/// into a sibling temporary file, verifies on the destination that it reports
/// `expected_identity`, and only then renames it over the path.
pub fn install_command(helper_path: &str, expected_identity: &str, byte_count: usize) -> String {
    remote_command(
        INSTALL_SCRIPT,
        "agentenv-install",
        &[helper_path, expected_identity, &byte_count.to_string()],
    )
}

/// What the configured path currently holds on the destination.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Existing {
    Absent,
    /// A symlink, directory, non-executable or executable that does not
    /// report a helper identity.
    Occupied,
    Helper(String),
}

/// Strictly parsed preflight output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Preflight {
    /// `uname -s`, for example `Linux` or `Darwin`.
    pub system: String,
    /// `uname -m`, for example `x86_64`, `aarch64` or `arm64`.
    pub machine: String,
    /// `(major, minor)` from `getconf GNU_LIBC_VERSION`, when glibc answered.
    pub glibc: Option<(u32, u32)>,
    pub existing: Existing,
}

fn printable_line(line: &str) -> bool {
    !line.is_empty()
        && line.len() <= MAX_PREFLIGHT_LINE_BYTES
        && line.bytes().all(|byte| (0x20..0x7f).contains(&byte))
}

fn token(line: &str) -> bool {
    printable_line(line)
        && line
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
}

/// Recognizes exactly one helper identity line, `agentenv-sudo-helper <protocol> <version>`.
/// The version grammar is the one `remote_command` can interpolate safely.
pub fn parse_identity(line: &str) -> Option<&str> {
    let mut fields = line.split(' ');
    let (Some("agentenv-sudo-helper"), Some(protocol), Some(version), None) =
        (fields.next(), fields.next(), fields.next(), fields.next())
    else {
        return None;
    };
    let protocol_ok = !protocol.is_empty() && protocol.bytes().all(|byte| byte.is_ascii_digit());
    let version_ok = !version.is_empty()
        && version
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'+'));
    (protocol_ok && version_ok).then_some(line)
}

/// Parses preflight stdout. Any prefix that is not the expected four lines,
/// including a login banner, is a failure. The fourth part must be exactly
/// `absent`, `occupied`, or `helper ` plus one identity line; anything else,
/// including extra lines, means the path is occupied.
pub fn parse_preflight(output: &[u8]) -> Result<Preflight, AppError> {
    let unparseable = |detail: &str| {
        Reason::PreflightUnparseable.error(format!(
            "the destination did not answer the preflight as expected ({detail}); a login banner on stdout or a non-POSIX login shell prevents deployment"
        ))
    };
    if output.len() > SESSION_OUTPUT_LIMIT {
        return Err(unparseable("output too long"));
    }
    let text = std::str::from_utf8(output).map_err(|_| unparseable("not UTF-8"))?;
    let Some(text) = text.strip_suffix('\n') else {
        return Err(unparseable("missing final newline"));
    };
    let mut lines = text.splitn(4, '\n');
    let (Some(system), Some(machine), Some(glibc), Some(state)) =
        (lines.next(), lines.next(), lines.next(), lines.next())
    else {
        return Err(unparseable("fewer than four lines"));
    };
    if !token(system) || !token(machine) {
        return Err(unparseable("unexpected system or machine line"));
    }
    let glibc = match glibc {
        "none" => None,
        _ => {
            let version = glibc
                .strip_prefix("glibc ")
                .ok_or_else(|| unparseable("unexpected glibc line"))?;
            let digits = |part: &str| {
                (!part.is_empty()
                    && part.len() <= 6
                    && part.bytes().all(|byte| byte.is_ascii_digit()))
                .then(|| part.parse::<u32>().ok())
                .flatten()
            };
            // Development snapshots report a third component (`2.40.9000`);
            // it never decides the floor, so it is accepted and ignored.
            let mut parts = version.splitn(3, '.');
            match (parts.next(), parts.next(), parts.next()) {
                (Some(major), Some(minor), patch)
                    if patch.is_none_or(|patch| digits(patch).is_some()) =>
                {
                    match (digits(major), digits(minor)) {
                        (Some(major), Some(minor)) => Some((major, minor)),
                        _ => return Err(unparseable("unexpected glibc version")),
                    }
                }
                _ => return Err(unparseable("unexpected glibc version")),
            }
        }
    };
    // Only `helper ` may be followed by more text, because it carries
    // whatever the existing executable printed; text after a bare keyword
    // did not come from the preflight script.
    let existing = match state {
        "absent" => Existing::Absent,
        "occupied" => Existing::Occupied,
        other => match other.strip_prefix("helper ") {
            Some(answer) => match parse_identity(answer) {
                Some(identity) => Existing::Helper(identity.to_owned()),
                None => Existing::Occupied,
            },
            None => return Err(unparseable("unexpected state line")),
        },
    };
    Ok(Preflight {
        system: system.to_owned(),
        machine: machine.to_owned(),
        glibc,
        existing,
    })
}

/// The destination as a release target, derived from the preflight.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Destination {
    /// `linux` or `macos`.
    pub platform: &'static str,
    pub machine: String,
    /// The Rust target triple whose helper the destination runs.
    pub target: &'static str,
    /// `major.minor` when the destination reported glibc.
    pub glibc: Option<String>,
}

/// Maps `uname -s`/`uname -m` to a published helper target.
pub fn destination_target(system: &str, machine: &str) -> Option<(&'static str, &'static str)> {
    match (system, machine) {
        ("Linux", "x86_64") => Some(("linux", "x86_64-unknown-linux-gnu")),
        ("Linux", "aarch64") => Some(("linux", "aarch64-unknown-linux-gnu")),
        ("Darwin", "arm64") => Some(("macos", "aarch64-apple-darwin")),
        ("Darwin", "x86_64") => Some(("macos", "x86_64-apple-darwin")),
        _ => None,
    }
}

/// What the preflight calls for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    UpToDate {
        destination: Destination,
        identity: String,
    },
    Install {
        destination: Destination,
        previous: Option<String>,
    },
}

/// Applies the design's decision table: refuse an occupied path, refuse an
/// unsupported or too-old destination, report an exact identity match as
/// up to date unless forced, otherwise install.
pub fn decide(preflight: &Preflight, expected: &str, force: bool) -> Result<Decision, AppError> {
    if preflight.existing == Existing::Occupied {
        return Err(Reason::PathOccupied.error(
            "helper_path exists but is not a runnable agentenv-sudo-helper (a symlink, directory, other file, or a helper that cannot run on this destination); fix helper_path or remove the file, then rerun",
        ));
    }
    let Some((platform, target)) = destination_target(&preflight.system, &preflight.machine) else {
        return Err(Reason::DestinationUnsupported.error(format!(
            "no prebuilt helper exists for {} {}",
            preflight.system, preflight.machine
        )));
    };
    if platform == "linux" {
        match preflight.glibc {
            None => {
                return Err(Reason::DestinationUnsupported.error(
                    "the destination reports no glibc; the Linux helper needs glibc 2.28 or newer and musl systems build from source",
                ));
            }
            Some(version) if version < GLIBC_FLOOR => {
                return Err(Reason::GlibcTooOld.error(format!(
                    "the destination has glibc {}.{} and the helper needs {}.{} or newer",
                    version.0, version.1, GLIBC_FLOOR.0, GLIBC_FLOOR.1
                )));
            }
            Some(_) => {}
        }
    }
    let destination = Destination {
        platform,
        machine: preflight.machine.clone(),
        target,
        glibc: preflight
            .glibc
            .map(|(major, minor)| format!("{major}.{minor}")),
    };
    match &preflight.existing {
        Existing::Helper(identity) if identity == expected && !force => Ok(Decision::UpToDate {
            destination,
            identity: identity.clone(),
        }),
        Existing::Helper(identity) => Ok(Decision::Install {
            destination,
            previous: Some(identity.clone()),
        }),
        Existing::Absent => Ok(Decision::Install {
            destination,
            previous: None,
        }),
        Existing::Occupied => unreachable!("occupied paths are refused above"),
    }
}

/// What one remote command produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionOutput {
    /// The remote exit status, `None` when the session ended by signal or
    /// without a status.
    pub status: Option<i32>,
    pub stdout: Vec<u8>,
}

/// One remote command over the target's prepared SSH route.
///
/// The runner writes `stdin` completely and then half-closes it, closing it
/// immediately when there is nothing to send. It drains stdout and stderr
/// concurrently with that write, because both commands may run a foreign
/// executable that blocks on a full pipe. It reads stdout to `stdout_limit`
/// bytes and fails the session if more arrives, discards stderr, and ends the
/// session at its setup deadline or on cancellation. Login authentication and
/// host verification belong to the runner.
pub trait SessionRunner {
    fn run(
        &mut self,
        remote_command: &str,
        stdin: Vec<u8>,
        stdout_limit: usize,
    ) -> impl Future<Output = Result<SessionOutput, AppError>> + Send;
}

/// Helper bytes chosen for a destination, with their provenance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HelperBytes {
    /// `file`, `bundle` or `release`.
    pub kind: &'static str,
    /// The file path, bundle path, or release asset name.
    pub name: String,
    pub bytes: Vec<u8>,
}

/// Supplies verified helper bytes for the destination's target.
pub trait HelperSource {
    fn bytes(
        self,
        destination: &Destination,
    ) -> impl Future<Output = Result<HelperBytes, AppError>> + Send;
}

/// Where the bytes came from, for the report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SourceReport {
    pub kind: &'static str,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    /// The helper was installed from `source`.
    Deployed { source: SourceReport },
    /// The destination already reported this build; nothing changed.
    UpToDate,
}

impl Status {
    /// The status as the JSON report spells it.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Deployed { .. } => "deployed",
            Self::UpToDate => "up-to-date",
        }
    }
}

/// The outcome of one deployment, before the acceptance handshake.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    pub status: Status,
    pub helper_path: String,
    pub destination: Destination,
    /// The identity found at the path before deployment, if any.
    pub previous: Option<String>,
    /// The identity at the path afterwards.
    pub installed: String,
}

impl Report {
    /// Where the installed bytes came from; `None` when nothing changed.
    pub fn source(&self) -> Option<&SourceReport> {
        match &self.status {
            Status::Deployed { source } => Some(source),
            Status::UpToDate => None,
        }
    }
}

/// Runs preflight, decides, obtains bytes and installs. The caller performs
/// the check-mode handshake afterwards as the acceptance criterion.
pub async fn deploy<R, S>(
    runner: &mut R,
    helper_path: &Path,
    force: bool,
    source: S,
) -> Result<Report, AppError>
where
    R: SessionRunner,
    S: HelperSource,
{
    let helper_path = validate_helper_path(helper_path)?;
    let expected = expected_identity();
    let preflight = runner
        .run(
            &preflight_command(helper_path),
            Vec::new(),
            SESSION_OUTPUT_LIMIT,
        )
        .await?;
    match preflight.status {
        Some(0) => {}
        Some(status) => {
            return Err(Reason::PreflightUnparseable
                .error(format!("the preflight command exited with status {status}")));
        }
        None => {
            return Err(Reason::SessionFailed.error(
                "the preflight session ended without an exit status (deadline or ssh stopped); nothing was written",
            ));
        }
    }
    let preflight = parse_preflight(&preflight.stdout)?;
    let (destination, previous) = match decide(&preflight, &expected, force)? {
        Decision::UpToDate {
            destination,
            identity,
        } => {
            return Ok(Report {
                status: Status::UpToDate,
                helper_path: helper_path.to_owned(),
                destination,
                previous: Some(identity.clone()),
                installed: identity,
            });
        }
        Decision::Install {
            destination,
            previous,
        } => (destination, previous),
    };
    let helper = source.bytes(&destination).await?;
    if helper.bytes.is_empty() || helper.bytes.len() > HELPER_UPLOAD_LIMIT {
        return Err(Reason::SourceUnavailable.error(format!(
            "{} is empty or larger than {} bytes",
            helper.name, HELPER_UPLOAD_LIMIT
        )));
    }
    let installed = runner
        .run(
            &install_command(helper_path, &expected, helper.bytes.len()),
            helper.bytes,
            SESSION_OUTPUT_LIMIT,
        )
        .await?;
    let expected_line = format!("{expected}\n");
    match installed.status {
        Some(0) if installed.stdout == expected_line.as_bytes() => {}
        Some(0) => {
            return Err(Reason::IdentityMismatch.error(
                "the destination reported success but echoed an unexpected identity; --check tells which helper is installed",
            ));
        }
        Some(INSTALL_STATUS_STALE_TEMP) => {
            return Err(Reason::ReplaceFailed
                .error("a stale temporary file beside helper_path could not be removed"));
        }
        Some(INSTALL_STATUS_DIRECTORY) => {
            return Err(Reason::ReplaceFailed
                .error("the directory containing helper_path could not be created"));
        }
        Some(INSTALL_STATUS_OCCUPIED) => {
            return Err(Reason::PathOccupied
                .error("helper_path changed during deployment and is no longer a regular file"));
        }
        Some(INSTALL_STATUS_WRITE) => {
            return Err(
                Reason::UploadFailed.error("the helper could not be written completely beside helper_path (write failure, a truncated upload, or a concurrent deployment removing the upload)")
            );
        }
        Some(INSTALL_STATUS_MODE) => {
            return Err(
                Reason::UploadFailed.error("the uploaded helper could not be made executable")
            );
        }
        Some(INSTALL_STATUS_IDENTITY) => {
            return Err(Reason::IdentityMismatch.error("the uploaded helper did not run on the destination or reported a different identity; the wrong architecture or libc, a noexec mount, or a mismatched source file prevents it"));
        }
        Some(INSTALL_STATUS_RENAME) => {
            return Err(Reason::ReplaceFailed.error(
                "the verified helper could not be renamed over helper_path, or helper_path was not a regular file afterwards; --check tells which helper is installed",
            ));
        }
        Some(INSTALL_STATUS_SESSION_LOST) => {
            return Err(Reason::SessionFailed.error(
                "the SSH connection ended or the remote shell was killed during the install session; --check tells which helper is installed",
            ));
        }
        None => {
            return Err(Reason::SessionFailed.error(
                "the install session ended without an exit status (deadline or ssh stopped); --check tells which helper is installed",
            ));
        }
        Some(other) => {
            return Err(Reason::UploadFailed.error(format!(
                "the install command ended with status {other}; --check tells which helper is installed"
            )));
        }
    }
    Ok(Report {
        status: Status::Deployed {
            source: SourceReport {
                kind: helper.kind,
                name: helper.name,
            },
        },
        helper_path: helper_path.to_owned(),
        destination,
        previous,
        installed: expected,
    })
}

/// Where deployment takes helper bytes from, in the design's order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// An operator-supplied file; accepted as is and verified on the
    /// destination.
    File(PathBuf),
    /// The client's own bundle companion when the destination runs the same
    /// target and it reports this build's identity, otherwise the standalone
    /// release asset for this exact version verified against `SHA256SUMS`.
    Automatic {
        bundle: PathBuf,
        release_base_url: String,
    },
}

impl HelperSource for Source {
    async fn bytes(self, destination: &Destination) -> Result<HelperBytes, AppError> {
        match self {
            Self::File(path) => file_bytes(&path).await,
            Self::Automatic {
                bundle,
                release_base_url,
            } => {
                if destination.target == crate::update::TARGET {
                    if let Some(bytes) = bundle_bytes(&bundle).await? {
                        return Ok(bytes);
                    }
                }
                release_bytes(&release_base_url, destination.target).await
            }
        }
    }
}

async fn read_regular_file(path: PathBuf) -> Result<Vec<u8>, String> {
    // Local reads stay off the async runtime; they are small but blocking.
    tokio::task::spawn_blocking(move || {
        use std::io::Read;
        // A FIFO would block on open, so the path is classified first; the
        // opened handle is checked again because the path can change.
        let kind = std::fs::metadata(&path).map_err(|error| format!("cannot read it: {error}"))?;
        if !kind.is_file() {
            return Err("not a regular file".to_owned());
        }
        let file =
            std::fs::File::open(&path).map_err(|error| format!("cannot read it: {error}"))?;
        let metadata = file
            .metadata()
            .map_err(|error| format!("cannot read it: {error}"))?;
        if !metadata.is_file() {
            return Err("not a regular file".to_owned());
        }
        // The handle that was checked is the one that is read, bounded past
        // the limit so a file that grows meanwhile is still refused.
        let mut bytes = Vec::new();
        file.take(HELPER_UPLOAD_LIMIT as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|error| format!("cannot read it: {error}"))?;
        if bytes.is_empty() || bytes.len() > HELPER_UPLOAD_LIMIT {
            return Err(format!(
                "must be between 1 and {HELPER_UPLOAD_LIMIT} bytes; release helpers are, unstripped debug builds usually are not"
            ));
        }
        Ok(bytes)
    })
    .await
    .map_err(|_| "the read task failed".to_owned())?
}

async fn file_bytes(path: &Path) -> Result<HelperBytes, AppError> {
    let bytes = read_regular_file(path.to_path_buf())
        .await
        .map_err(|detail| {
            Reason::SourceUnavailable.error(format!("--from {}: {detail}", path.display()))
        })?;
    Ok(HelperBytes {
        kind: "file",
        name: path.display().to_string(),
        bytes,
    })
}

/// The bundle companion is used only when it exists and reports exactly this
/// build's identity; anything else falls through to the release asset.
async fn bundle_bytes(bundle: &Path) -> Result<Option<HelperBytes>, AppError> {
    let Ok(bytes) = read_regular_file(bundle.to_path_buf()).await else {
        return Ok(None);
    };
    let output = tokio::process::Command::new(bundle)
        .arg("--identity")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .output();
    let Ok(Ok(output)) = tokio::time::timeout(std::time::Duration::from_secs(10), output).await
    else {
        return Ok(None);
    };
    if !output.status.success() || output.stdout != format!("{}\n", expected_identity()).as_bytes()
    {
        return Ok(None);
    }
    Ok(Some(HelperBytes {
        kind: "bundle",
        name: bundle.display().to_string(),
        bytes,
    }))
}

/// The standalone helper asset name for `target` in this build's release.
pub fn release_asset_name(target: &str) -> (String, String) {
    let tag = format!("v{}", env!("CARGO_PKG_VERSION"));
    let name = format!("agentenv-sudo-helper-{tag}-{target}");
    (tag, name)
}

async fn release_bytes(base_url: &str, target: &str) -> Result<HelperBytes, AppError> {
    let (tag, name) = release_asset_name(target);
    let base_url = base_url.to_owned();
    let target = target.to_owned();
    let asset_name = name.clone();
    // ureq is synchronous; the fetch and download run off the async runtime.
    let result = tokio::task::spawn_blocking(move || {
        let client = crate::update::Client::new(&base_url);
        let asset = client.fetch_asset(&tag, &asset_name).map_err(|failure| {
            let detail = match failure {
                crate::update::AssetLookup::NoRelease(_) => format!(
                    "release {tag} is not published at {base_url}, so no prebuilt helper exists for this build; pass --from <file> with a helper for {target} built from this exact version"
                ),
                crate::update::AssetLookup::NoAsset(_) => format!(
                    "release {tag} ships no helper for {target}; pass --from <file> with a helper built from this exact version"
                ),
                crate::update::AssetLookup::Transport(detail) => format!(
                    "release {tag} at {base_url} could not be read ({detail}); pass --from <file> to deploy without reaching the release"
                ),
                crate::update::AssetLookup::Malformed(detail) => format!(
                    "release {tag} at {base_url} has an unusable SHA256SUMS ({detail}); pass --from <file> with a verified helper"
                ),
            };
            Reason::SourceUnavailable.error(detail)
        })?;
        let workdir = tempfile::Builder::new()
            .prefix("agentenv-helper-")
            .tempdir()
            .map_err(|error| {
                Reason::SourceUnavailable.error(format!(
                    "cannot create a temporary directory: {error}"
                ))
            })?;
        let path = client
            .download_asset(&tag, &asset, workdir.path(), HELPER_UPLOAD_LIMIT as u64 + 1)
            .map_err(|failure| match failure {
                crate::update::DownloadFailure::Checksum { .. } => {
                    Reason::ChecksumMismatch.error(failure.to_string())
                }
                crate::update::DownloadFailure::Transport(detail) => {
                    Reason::SourceUnavailable.error(format!(
                        "{detail}; pass --from <file> to deploy without reaching the release"
                    ))
                }
            })?;
        std::fs::read(&path).map_err(|error| {
            Reason::SourceUnavailable.error(format!(
                "cannot read the downloaded {}: {error}",
                asset.name
            ))
        })
    })
    .await
    .map_err(|_| Reason::SourceUnavailable.error("the release download task failed"))?;
    Ok(HelperBytes {
        kind: "release",
        name,
        bytes: result?,
    })
}
