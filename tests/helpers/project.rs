//! A staged config plus a project file, for suites that exercise project
//! discovery and trust through the binary.

use std::fs;
use std::path::{Path, PathBuf};

use tempfile::TempDir;

use super::{command_with_project_discovery, staged_config, Run, STATE_BASE_ENV};

/// A temp tree holding a mode-0600 config, a project file, a nested working
/// directory, and a trust-store state base:
///
/// ```text
/// <root>/config.toml
/// <root>/state/                       trust-store state base
/// <root>/workspace/.agentenv.toml     project file
/// <root>/workspace/nested/deeper/     working directory
/// ```
///
/// The working directory sits two levels below the project file, so every
/// invocation exercises the upward discovery walk. The tree is removed when
/// the value is dropped.
pub struct ProjectFixture {
    _config_dir: TempDir,
    pub config: PathBuf,
    pub cwd: PathBuf,
    pub project: PathBuf,
    pub state: PathBuf,
}

impl ProjectFixture {
    /// Stages `config_text` and writes `project_text` as the project file.
    pub fn new(config_text: &str, project_text: &str) -> Self {
        let fixture = Self::without_project_file(config_text);
        fs::write(&fixture.project, project_text).expect("project file is written");
        fixture
    }

    /// Stages `config_text` and the directory tree, leaving the project file
    /// path unwritten.
    pub fn without_project_file(config_text: &str) -> Self {
        let (config_dir, config) = staged_config(config_text);
        let root = config.parent().expect("config has a parent").to_owned();
        let workspace = root.join("workspace");
        let cwd = workspace.join("nested").join("deeper");
        fs::create_dir_all(&cwd).expect("nested working directory is created");

        Self {
            _config_dir: config_dir,
            config,
            cwd,
            project: workspace.join(".agentenv.toml"),
            state: root.join("state"),
        }
    }

    /// Approves the current project file through the library, recording the
    /// approval in this fixture's state base.
    pub fn approve(&self) {
        let env = |name: &str| match name {
            name if name == STATE_BASE_ENV => Some(self.state.display().to_string()),
            _ => None,
        };
        agentenv::project::allow(&self.cwd, &env).expect("project file is approved");
    }

    /// Runs agentenv from the working directory with discovery enabled and the
    /// fixture's state base set, adding `envs` on top.
    pub fn run(&self, args: &[&str], envs: &[(&str, &str)]) -> Run {
        self.invoke(Some(&self.state), args, envs)
    }

    /// Runs agentenv from the working directory with no trust-store state
    /// base in its environment, so trust state is unavailable unless `envs`
    /// supplies one.
    pub fn run_without_state_base(&self, args: &[&str], envs: &[(&str, &str)]) -> Run {
        self.invoke(None, args, envs)
    }

    fn invoke(&self, state: Option<&Path>, args: &[&str], envs: &[(&str, &str)]) -> Run {
        let mut command = command_with_project_discovery(&self.config);
        command.current_dir(&self.cwd);
        if let Some(state) = state {
            command.env(STATE_BASE_ENV, state);
        }
        for (name, value) in envs {
            command.env(name, value);
        }
        command.args(args);

        let output = command
            .output()
            .unwrap_or_else(|error| panic!("failed to run agentenv {args:?}: {error}"));
        Run::from_output(output, args)
    }
}
