//! Trust-on-first-use store for project files (SPEC-003).
//!
//! Approval is keyed by a project file's canonical absolute path together with
//! the SHA-256 fingerprint of the exact bytes that were approved, so any
//! content change or path change returns the file to the untrusted state. The
//! store lives outside every repository, under the user's platform state
//! directory (ARCH-002), which is why a repository can never approve itself.
//!
//! Every mutation is atomic: the replacement store is written to a temporary
//! file (created `0600` on Unix before any content reaches it) and then
//! renamed over the store, so an interrupted mutation leaves the previous
//! store byte-intact. Concurrent mutations serialize as last-writer-wins per
//! whole-store mutation; there is no cross-process locking.
//!
//! Filesystem access goes through [`StoreFs`] so the commit path can be
//! fault-injected; [`RealFs`] is the production adapter over `std::fs`.

use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::config::env_value;
use crate::error::{AppError, Violation};

const STORE_DIR: &str = "agentenv";
const STORE_FILE: &str = "trust.toml";

/// The filesystem operations the trust store needs, as one narrow seam.
///
/// Implementors own the durability details; the store itself only sequences
/// read, write-temp, and rename.
pub trait StoreFs {
    /// Reads a file's exact bytes. A missing file surfaces as
    /// [`io::ErrorKind::NotFound`], never as empty content.
    fn read(&self, path: &Path) -> io::Result<Vec<u8>>;

    /// Writes `bytes` to a fresh temporary file inside `dir`, creating `dir`
    /// when it is missing, and returns the temporary file's path. On Unix the
    /// file is created with `0600` permissions before any content is written,
    /// so the bytes are never readable by another user (AC-003.9).
    fn write_temp(&self, dir: &Path, bytes: &[u8]) -> io::Result<PathBuf>;

    /// Renames `from` over `to`, committing a prepared replacement.
    fn rename(&self, from: &Path, to: &Path) -> io::Result<()>;
}

/// The production [`StoreFs`] adapter over `std::fs`.
pub struct RealFs;

/// How many names [`RealFs::write_temp`] tries before giving up. A collision
/// means a leftover temporary file already occupies the name; the next name is
/// free unless the directory is pathologically full of them.
const TEMP_NAME_ATTEMPTS: u32 = 16;

/// Distinguishes temporary files created within one process.
static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

impl StoreFs for RealFs {
    fn read(&self, path: &Path) -> io::Result<Vec<u8>> {
        fs::read(path)
    }

    fn write_temp(&self, dir: &Path, bytes: &[u8]) -> io::Result<PathBuf> {
        fs::create_dir_all(dir)?;
        let (path, mut file) = create_temp_file(dir)?;
        match file.write_all(bytes).and_then(|()| file.sync_all()) {
            Ok(()) => Ok(path),
            Err(error) => {
                // Never leave a partial temporary file behind. The write
                // failure is what the caller must see, so a cleanup failure
                // does not replace it.
                let _ = fs::remove_file(&path);
                Err(error)
            }
        }
    }

    fn rename(&self, from: &Path, to: &Path) -> io::Result<()> {
        fs::rename(from, to)
    }
}

/// Creates an empty temporary file in `dir` under a name no other file holds,
/// with `0600` permissions on Unix applied at creation time.
fn create_temp_file(dir: &Path) -> io::Result<(PathBuf, fs::File)> {
    for _ in 0..TEMP_NAME_ATTEMPTS {
        let ordinal = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = dir.join(format!("{STORE_FILE}.{}.{ordinal}.tmp", std::process::id()));
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        match options.open(&path) {
            Ok(file) => return Ok((path, file)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        format!(
            "no free temporary file name in {}; remove leftover {STORE_FILE}.*.tmp files",
            dir.display()
        ),
    ))
}

/// Resolves the trust store path from the environment.
///
/// `$XDG_STATE_HOME/agentenv/trust.toml` when that names an absolute path,
/// else `$HOME/.local/state/agentenv/trust.toml` on Unix;
/// `%LOCALAPPDATA%\agentenv\trust.toml` on Windows, where `XDG_STATE_HOME` is
/// not consulted. Empty values count as unset (SPEC-AS-028), as does a
/// relative `XDG_STATE_HOME`. Pure environment logic with no filesystem
/// access, so callers can re-resolve it deterministically to name the store in
/// a diagnostic.
pub fn store_path(env: &impl Fn(&str) -> Option<String>) -> Result<PathBuf, AppError> {
    state_base(env).map(|base| base.join(STORE_DIR).join(STORE_FILE))
}

#[cfg(unix)]
fn state_base(env: &impl Fn(&str) -> Option<String>) -> Result<PathBuf, AppError> {
    if let Some(xdg) = env_value(env, "XDG_STATE_HOME") {
        let base = Path::new(&xdg);
        if base.is_absolute() {
            return Ok(base.to_path_buf());
        }
        // An empty or relative value is treated as unset, as for the config
        // file's XDG base (SPEC-001, AC-001.4).
    }
    match env_value(env, "HOME") {
        Some(home) => Ok(Path::new(&home).join(".local").join("state")),
        None => Err(AppError::Config(vec![Violation {
            path: "XDG_STATE_HOME".to_owned(),
            message: "cannot locate the trust store: neither XDG_STATE_HOME nor HOME is set; \
                      set XDG_STATE_HOME to an absolute path or set HOME, then re-run \
                      `agentenv project allow`"
                .to_owned(),
        }])),
    }
}

#[cfg(windows)]
fn state_base(env: &impl Fn(&str) -> Option<String>) -> Result<PathBuf, AppError> {
    // XDG_STATE_HOME is not consulted on Windows (ARCH-002).
    match env_value(env, "LOCALAPPDATA") {
        Some(local) => Ok(PathBuf::from(local)),
        None => Err(AppError::Config(vec![Violation {
            path: "LOCALAPPDATA".to_owned(),
            message: "cannot locate the trust store: LOCALAPPDATA is not set; set LOCALAPPDATA, \
                      then re-run `agentenv project allow`"
                .to_owned(),
        }])),
    }
}

/// The set of approved project files.
///
/// A record binds one canonical absolute project-file path to the hex SHA-256
/// fingerprint of the bytes approved for it. A file is trusted exactly when a
/// record exists for its canonical path and the fingerprint of its current
/// bytes equals the recorded one.
#[derive(Debug)]
pub struct TrustStore {
    /// Canonical absolute project-file path to hex SHA-256 of the approved
    /// bytes. Ordered so a saved store is stable across mutations.
    // The serialized representation is internal; only the behavior of the
    // methods below is contractual.
    #[allow(dead_code)] // Read once the bodies below land (T005).
    records: BTreeMap<String, String>,
}

// The bodies land in T005; the parameter names below are the pinned contract.
#[allow(unused_variables)]
impl TrustStore {
    /// Loads the store at `path` through `fs`.
    ///
    /// A store that does not exist yet loads as an empty store — no approvals
    /// recorded. A store that exists but cannot be parsed is an
    /// [`AppError::Config`] (exit 2) naming the store path and the remedy, and
    /// is never treated as empty (AC-003.8); an unreadable store fails the
    /// same way.
    pub fn load(path: &Path, fs: &dyn StoreFs) -> Result<TrustStore, AppError> {
        todo!("T005: read the store, parse it, and reject a corrupt store explicitly")
    }

    /// The fingerprint approved for `canonical`, if any.
    ///
    /// `canonical` is the project file's canonical absolute path; trust
    /// identity is that path alone, never the spelling a command reached it
    /// through (AC-003.7).
    pub fn lookup(&self, canonical: &Path) -> Option<&str> {
        todo!("T005: look up the approval record for the canonical path")
    }

    /// Records approval of `content` for `canonical`, replacing any earlier
    /// approval of that path.
    ///
    /// Approval binds to exactly these bytes: the caller passes the single
    /// snapshot it validated, so a file changed afterwards resolves as
    /// untrusted (AC-003.12).
    pub fn allow(&mut self, canonical: &Path, content: &[u8]) {
        todo!("T005: fingerprint the snapshot and record it under the canonical path")
    }

    /// Removes the approval for `canonical`, reporting whether one existed.
    ///
    /// Path-only by contract: no content is read and nothing is validated, so
    /// a changed, invalid, or unreadable file can always be revoked
    /// (AC-003.13).
    pub fn revoke(&mut self, canonical: &Path) -> bool {
        todo!("T005: drop the record for the canonical path and report whether it existed")
    }

    /// Writes the store to `path` through `fs`, atomically.
    ///
    /// The serialized store goes to a temporary file in `path`'s parent
    /// directory, which is then renamed over `path`, so a failure at any step
    /// leaves the previous store byte-intact. Every failure is an
    /// [`AppError::Config`] (exit 2) naming the store path and a next action
    /// (AC-003.11).
    pub fn save(&self, path: &Path, fs: &dyn StoreFs) -> Result<(), AppError> {
        todo!("T005: serialize, write to a temporary file, and rename it over the store")
    }
}

/// The hex-encoded SHA-256 fingerprint of `content`.
///
/// Byte-exact: any difference in the content, whitespace included, produces a
/// different fingerprint (AC-003.3).
#[allow(unused_variables)] // The body lands in T005.
pub fn fingerprint(content: &[u8]) -> String {
    todo!("T005: hex-encode the SHA-256 digest of the content")
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};
    use std::collections::BTreeMap;
    use std::io;
    use std::path::{Path, PathBuf};

    use super::{store_path, StoreFs, TrustStore};
    use crate::error::{AppError, Violation};

    fn env_of<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |name| {
            pairs
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| value.to_string())
        }
    }

    /// Unwraps the exit-2 configuration error every trust-store failure uses.
    fn config_violations(error: AppError) -> Vec<Violation> {
        assert_eq!(error.exit_code(), 2, "store failures exit 2: {error}");
        match error {
            AppError::Config(violations) => violations,
            other => panic!("expected a configuration error, got {other:?}"),
        }
    }

    fn rendered(violations: &[Violation]) -> String {
        violations
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("; ")
    }

    /// An in-memory [`StoreFs`] that can fail the commit on demand.
    ///
    /// It models a single store file: `committed` is what a reader sees, and a
    /// successful rename is the only way to change it. That makes "the
    /// previous store survived" a direct assertion (AC-003.11), which no real
    /// filesystem lets a test schedule deterministically.
    struct FaultyFs {
        committed: RefCell<Option<Vec<u8>>>,
        temps: RefCell<BTreeMap<PathBuf, Vec<u8>>>,
        rename_fails: bool,
        next_temp: Cell<u32>,
    }

    impl FaultyFs {
        fn with_store(bytes: Vec<u8>) -> Self {
            Self {
                committed: RefCell::new(Some(bytes)),
                temps: RefCell::new(BTreeMap::new()),
                rename_fails: false,
                next_temp: Cell::new(0),
            }
        }

        fn failing_rename(mut self) -> Self {
            self.rename_fails = true;
            self
        }

        fn committed(&self) -> Option<Vec<u8>> {
            self.committed.borrow().clone()
        }
    }

    impl StoreFs for FaultyFs {
        fn read(&self, path: &Path) -> io::Result<Vec<u8>> {
            match self.committed.borrow().clone() {
                Some(bytes) => Ok(bytes),
                None => Err(io::Error::new(
                    io::ErrorKind::NotFound,
                    format!("{} does not exist", path.display()),
                )),
            }
        }

        fn write_temp(&self, dir: &Path, bytes: &[u8]) -> io::Result<PathBuf> {
            let ordinal = self.next_temp.get();
            self.next_temp.set(ordinal + 1);
            let path = dir.join(format!("trust.toml.{ordinal}.tmp"));
            self.temps.borrow_mut().insert(path.clone(), bytes.to_vec());
            Ok(path)
        }

        fn rename(&self, from: &Path, to: &Path) -> io::Result<()> {
            if self.rename_fails {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    format!("injected failure renaming onto {}", to.display()),
                ));
            }
            match self.temps.borrow_mut().remove(from) {
                Some(bytes) => {
                    *self.committed.borrow_mut() = Some(bytes);
                    Ok(())
                }
                None => Err(io::Error::new(
                    io::ErrorKind::NotFound,
                    format!("{} does not exist", from.display()),
                )),
            }
        }
    }

    #[test]
    fn a_failing_commit_leaves_the_previous_store_intact() {
        // AC-003.11. A comment-only store is a valid, empty store in any
        // TOML-based serialization, so the assertion holds whatever record
        // format the store adopts.
        let previous = b"# agentenv trust store\n".to_vec();
        let fs = FaultyFs::with_store(previous.clone()).failing_rename();
        let path = Path::new("/state/agentenv/trust.toml");

        let mut store = TrustStore::load(path, &fs).expect("a parseable store loads");
        store.allow(Path::new("/repo/.agentenv.toml"), b"version = 1\n");

        let error = store
            .save(path, &fs)
            .expect_err("a commit that cannot rename must fail loudly");
        let text = rendered(&config_violations(error));
        assert!(
            text.contains(&path.display().to_string()),
            "the error names the store path: {text}"
        );
        assert!(
            text.contains("agentenv project allow"),
            "the error states a next action: {text}"
        );
        assert_eq!(
            fs.committed(),
            Some(previous),
            "the previous store is byte-intact after a failed commit"
        );
    }

    #[cfg(unix)]
    #[test]
    fn an_absolute_xdg_state_home_locates_the_store() {
        let env = env_of(&[("XDG_STATE_HOME", "/xdg/state"), ("HOME", "/home/user")]);
        let path = store_path(&env).expect("XDG_STATE_HOME wins");
        assert_eq!(path, PathBuf::from("/xdg/state/agentenv/trust.toml"));
    }

    #[cfg(unix)]
    #[test]
    fn home_locates_the_store_without_xdg_state_home() {
        let env = env_of(&[("HOME", "/home/user")]);
        let path = store_path(&env).expect("HOME is used");
        assert_eq!(
            path,
            PathBuf::from("/home/user/.local/state/agentenv/trust.toml")
        );
    }

    #[cfg(unix)]
    #[test]
    fn an_empty_xdg_state_home_counts_as_unset() {
        let env = env_of(&[("XDG_STATE_HOME", ""), ("HOME", "/home/user")]);
        let path = store_path(&env).expect("HOME is used");
        assert_eq!(
            path,
            PathBuf::from("/home/user/.local/state/agentenv/trust.toml")
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_relative_xdg_state_home_counts_as_unset() {
        let env = env_of(&[("XDG_STATE_HOME", "relative/state"), ("HOME", "/home/user")]);
        let path = store_path(&env).expect("HOME is used");
        assert_eq!(
            path,
            PathBuf::from("/home/user/.local/state/agentenv/trust.toml")
        );
    }

    #[cfg(unix)]
    #[test]
    fn an_unset_state_base_names_the_variables_and_a_next_action() {
        // EDGE-004b: allow and revoke cannot write without a state base.
        let env = env_of(&[("HOME", "")]);
        let violations = config_violations(store_path(&env).expect_err("no state base exists"));
        let text = rendered(&violations);
        assert!(text.contains("XDG_STATE_HOME"), "{text}");
        assert!(text.contains("HOME"), "{text}");
        assert!(
            text.contains("agentenv project allow"),
            "the error states a next action: {text}"
        );
    }

    #[cfg(windows)]
    #[test]
    fn localappdata_locates_the_store_on_windows() {
        let env = env_of(&[("LOCALAPPDATA", r"C:\Users\u\AppData\Local")]);
        let path = store_path(&env).expect("LOCALAPPDATA is used");
        assert_eq!(
            path,
            PathBuf::from(r"C:\Users\u\AppData\Local\agentenv\trust.toml")
        );
    }

    #[cfg(windows)]
    #[test]
    fn xdg_state_home_is_not_consulted_on_windows() {
        let env = env_of(&[
            ("XDG_STATE_HOME", r"C:\xdg"),
            ("LOCALAPPDATA", r"C:\Users\u\AppData\Local"),
        ]);
        let path = store_path(&env).expect("LOCALAPPDATA is used");
        assert_eq!(
            path,
            PathBuf::from(r"C:\Users\u\AppData\Local\agentenv\trust.toml")
        );
    }

    #[cfg(windows)]
    #[test]
    fn an_unset_localappdata_names_the_variable_and_a_next_action() {
        // EDGE-004b on Windows.
        let env = env_of(&[("LOCALAPPDATA", "")]);
        let violations = config_violations(store_path(&env).expect_err("no state base exists"));
        let text = rendered(&violations);
        assert!(text.contains("LOCALAPPDATA"), "{text}");
        assert!(
            text.contains("agentenv project allow"),
            "the error states a next action: {text}"
        );
    }
}
