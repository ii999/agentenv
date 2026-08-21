//! Process-launch coverage for the non-conflict `run` criteria.

mod helpers;

use std::fs;
use std::path::{Path, PathBuf};

use assert_cmd::cargo::CommandCargoExt;
use helpers::{assert_exit, run_ac};
use tempfile::TempDir;

#[test]
fn run_injects_credentials_and_plain_values_without_mutating_the_wrapper_environment() {
    let workspace = Workspace::new();
    let config = workspace.stage_config(RUNNER_CONFIG);
    let report = workspace.path("probe-report");
    let probe = assert_cmd::cargo::cargo_bin("test-probe");

    let run = run_ac(
        &config,
        &[
            ("SOURCE_TOKEN", "resolved-value"),
            ("OPENAI_API_KEY", "inherited-value"),
            (
                "TEST_PROBE_OUT",
                report.to_str().expect("report path is UTF-8"),
            ),
        ],
        &[
            "run",
            "--with",
            "llm",
            "--with",
            "ci",
            "--",
            probe.to_str().expect("probe path is UTF-8"),
            "probe-argument",
        ],
    );

    assert_exit(&run, 0, "injected target");
    assert_eq!(run.stdout, "out");
    assert_eq!(run.stderr, "err");

    let report = fs::read_to_string(report).expect("the target writes its environment report");
    assert!(report.contains("argv\tprobe-argument\n"));
    assert!(report.contains("env\tOPENAI_API_KEY=resolved-value\n"));
    assert!(report.contains("env\tLLM_API_KEY=resolved-value\n"));
    assert!(report.contains("env\tOPENAI_BASE_URL=https://llm.example.com/v1\n"));
    assert!(report.contains("env\tCI_ENABLED=true\n"));
}

#[cfg(unix)]
#[test]
fn run_preserves_a_target_termination_signal() {
    use std::os::unix::process::ExitStatusExt;

    let workspace = Workspace::new();
    let config = workspace.stage_config(RUNNER_CONFIG);
    let status = std::process::Command::cargo_bin("agent-context")
        .expect("agent-context is built")
        .env_clear()
        .env("AGENT_CONTEXT_FILE", config)
        .env("SOURCE_TOKEN", "signal-test-value")
        .args([
            "run",
            "--with",
            "ci",
            "--",
            "/bin/sh",
            "-c",
            "kill -TERM $$",
        ])
        .status()
        .expect("agent-context launches");

    assert_eq!(status.signal(), Some(15));
}

const RUNNER_CONFIG: &str = r#"
version = 1
default_profile = "work"

[profiles.work]
description = "Run test profile."

[profiles.work.llm]
description = "LLM entry."
credential = "credential://api?as=LLM_API_KEY"
endpoint = "https://llm.example.com/v1"

[profiles.work.llm.inject]
OPENAI_BASE_URL = "endpoint"

[profiles.work.ci]
description = "CI entry."
credential = "credential://api"
enabled = true
tags = ["credential://api"]

[profiles.work.ci.inject]
CI_ENABLED = "enabled"

[credentials.api]
description = "Credential supplied by the test process."
provider = "env"
name = "SOURCE_TOKEN"
inject_as = "OPENAI_API_KEY"
"#;

struct Workspace {
    directory: TempDir,
}

impl Workspace {
    fn new() -> Self {
        Self {
            directory: TempDir::new().expect("a temporary workspace is created"),
        }
    }

    fn path(&self, name: &str) -> PathBuf {
        self.directory.path().join(name)
    }

    fn stage_config(&self, content: &str) -> PathBuf {
        let path = self.path("context.toml");
        fs::write(&path, content).expect("the test config is written");
        restrict_permissions(&path);
        path
    }
}

#[cfg(unix)]
fn restrict_permissions(path: &Path) {
    use std::os::unix::fs::PermissionsExt;

    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
        .expect("the test config permissions are restricted");
}

#[cfg(not(unix))]
fn restrict_permissions(_path: &Path) {}
