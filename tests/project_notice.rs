//! Integration coverage for the pre-dispatch project trust notice.

mod helpers;

use std::fs;
use std::path::{Path, PathBuf};

use helpers::{assert_exit, command_with_project_discovery, staged_config};
use tempfile::TempDir;

const CONFIG: &str = r#"
version = 1
default_profile = "personal"

[profiles.personal]
description = "Personal"
[profiles.personal.llm]
description = "Personal model"
model = "personal-model"

[profiles.personal.runner]
description = "Runner"
flag = "personal"

[profiles.personal.runner.inject]
TEST_PROJECT_NOTICE = "flag"
"#;

struct ProjectFixture {
    _config_dir: TempDir,
    config: PathBuf,
    cwd: PathBuf,
    project: PathBuf,
    state: PathBuf,
}

impl ProjectFixture {
    fn new(project_text: &str) -> Self {
        let (config_dir, config) = staged_config(CONFIG);
        let root = config.parent().expect("config has a parent").to_owned();
        let cwd = root.join("nested");
        fs::create_dir_all(&cwd).expect("nested working directory is created");
        let project = root.join(".agentenv.toml");
        fs::write(&project, project_text).expect("project file is written");
        let state = root.join("state");
        Self {
            _config_dir: config_dir,
            config,
            cwd,
            project,
            state,
        }
    }

    fn approve(&self) {
        let env = |name: &str| match name {
            name if name == helpers::STATE_BASE_ENV => Some(self.state.display().to_string()),
            _ => None,
        };
        agentenv::project::allow(&self.cwd, &env).expect("project file is approved");
    }

    fn run(&self, args: &[&str], envs: &[(&str, &str)]) -> helpers::Run {
        let mut command = command_with_project_discovery(&self.config);
        command.current_dir(&self.cwd);
        command.env(helpers::STATE_BASE_ENV, &self.state);
        for (name, value) in envs {
            command.env(name, value);
        }
        command.args(args);
        let output = command.output().expect("agentenv runs");
        helpers::Run {
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            code: output.status.code(),
        }
    }

    fn bypassed(&self, args: &[&str]) -> helpers::Run {
        let mut command = command_with_project_discovery(&self.config);
        command.current_dir(&self.cwd);
        command.env("AGENTENV_NO_PROJECT", "1");
        command.args(args);
        let output = command
            .output()
            .expect("agentenv runs with discovery bypassed");
        helpers::Run {
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            code: output.status.code(),
        }
    }
}

fn assert_single_notice(run: &helpers::Run, project: &Path) {
    assert_eq!(
        run.stderr.lines().count(),
        1,
        "stderr contains exactly one notice: {}",
        run.stderr
    );
    assert!(run.stderr.contains(&helpers::canonical_display(project)));
    assert!(run.stderr.contains("agentenv project status"));
}

#[test]
fn untrusted_files_are_inert_except_for_one_notice() {
    let fixture = ProjectFixture::new("version = 1\nprofile = \"missing\"\n");

    let baseline = fixture.bypassed(&["get", "llm.model"]);
    let discovered = fixture.run(&["get", "llm.model"], &[]);
    assert_exit(&baseline, 0, "bypassed baseline");
    assert_exit(&discovered, 0, "untrusted project remains inert");
    assert_eq!(
        discovered.stdout, baseline.stdout,
        "notice does not alter stdout"
    );
    assert_single_notice(&discovered, &fixture.project);
}

#[test]
fn invalid_untrusted_files_remain_inert_and_bypass_suppresses_discovery() {
    let fixture = ProjectFixture::new("version = \"wrong\"\n");

    let discovered = fixture.run(&["get", "llm.model"], &[]);
    assert_exit(&discovered, 0, "invalid untrusted project remains inert");
    assert_eq!(discovered.stdout, "personal-model\n");
    assert_single_notice(&discovered, &fixture.project);

    let flag = fixture.run(&["--no-project", "get", "llm.model"], &[]);
    assert_exit(&flag, 0, "flag bypasses project discovery");
    assert!(flag.stderr.is_empty());

    let environment = fixture.run(&["get", "llm.model"], &[("AGENTENV_NO_PROJECT", "1")]);
    assert_exit(&environment, 0, "environment bypasses project discovery");
    assert!(environment.stderr.is_empty());
}

#[test]
fn notice_precedes_command_errors_and_is_absent_for_parse_failures() {
    let fixture = ProjectFixture::new("version = 1\nprofile = \"personal\"\n");

    let failure = fixture.run(&["get", "missing.value"], &[]);
    assert_exit(&failure, 3, "unrelated command error keeps its status");
    assert!(failure.stderr.contains("name resolution error"));
    assert_eq!(failure.stderr.matches("agentenv project status").count(), 1);
    assert!(failure.stderr.starts_with("project file "));

    let parse_failure = fixture.run(&["--not-a-real-flag"], &[]);
    assert_exit(&parse_failure, 1, "parse failure remains a usage error");
    assert!(!parse_failure.stderr.contains("agentenv project status"));
}

#[test]
fn notice_is_flushed_before_a_run_target_replaces_the_process() {
    let fixture = ProjectFixture::new("version = 1\nprofile = \"personal\"\n");
    let probe = assert_cmd::cargo::cargo_bin("test-probe");
    let run = fixture.run(
        &[
            "run",
            "--with",
            "runner",
            "--",
            probe.to_str().expect("probe path is UTF-8"),
        ],
        &[],
    );

    assert_exit(&run, 0, "untrusted project remains inert for run");
    assert_eq!(run.stdout, "out");
    let notice_end = run.stderr.find('\n').expect("notice ends with a newline");
    assert!(run.stderr[..notice_end].contains("agentenv project status"));
    assert_eq!(&run.stderr[notice_end + 1..], "err");
}

#[test]
fn trusted_files_and_unavailable_state_follow_their_notice_rules() {
    let fixture = ProjectFixture::new("version = 1\nprofile = \"personal\"\n");
    fixture.approve();
    let trusted = fixture.run(&["get", "llm.model"], &[]);
    assert_exit(&trusted, 0, "trusted file succeeds");
    assert!(trusted.stderr.is_empty(), "trusted file emits no notice");

    let unavailable = fixture.run(&["get", "llm.model"], &[(helpers::STATE_BASE_ENV, "")]);
    assert_exit(&unavailable, 0, "unavailable state leaves the file inert");
    assert_single_notice(&unavailable, &fixture.project);
    assert!(unavailable.stderr.contains(helpers::STATE_BASE_ENV));
    assert!(unavailable.stderr.contains("HOME"));
}
