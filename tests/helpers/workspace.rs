//! A scratch directory for suites that stage their own config, fixture
//! scripts, and observation files side by side.

use std::fs;
use std::path::{Path, PathBuf};

use tempfile::TempDir;

use super::{fixture_source, restrict_permissions, set_mode};

/// A temp directory holding one test's config, fixture scripts, and
/// observation files. Held for as long as the test invokes against it.
pub struct Workspace {
    directory: TempDir,
}

impl Workspace {
    pub fn new() -> Self {
        Self {
            directory: TempDir::new().expect("a temporary workspace is created"),
        }
    }

    /// The workspace directory itself.
    pub fn root(&self) -> &Path {
        self.directory.path()
    }

    /// A path inside the workspace. Nothing is created: some tests read a
    /// path's absence as evidence.
    pub fn path(&self, name: &str) -> PathBuf {
        self.directory.path().join(name)
    }

    /// Stages `contents` as `config.toml` at mode 0600, which the Unix
    /// permission gate requires of any config that should load cleanly.
    pub fn stage_config(&self, contents: &str) -> PathBuf {
        let path = self.path("config.toml");
        fs::write(&path, contents).expect("the test config is written");
        restrict_permissions(&path);
        path
    }

    /// Copies the fixture script `name` into the workspace and makes it
    /// executable, so tests do not depend on the checked-in mode surviving a
    /// checkout.
    pub fn stage_script(&self, name: &str) -> PathBuf {
        let path = self.path(name);
        fs::copy(fixture_source(name), &path)
            .unwrap_or_else(|error| panic!("failed to stage fixture script {name}: {error}"));
        set_mode(&path, 0o755);
        path
    }
}
