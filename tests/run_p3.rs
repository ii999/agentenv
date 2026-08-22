//! Process-launch coverage for the non-conflict `run` criteria.

mod helpers;

use std::fs;
use std::path::{Path, PathBuf};

use assert_cmd::cargo::CommandCargoExt;
use helpers::{assert_exit, run_ac, SENTINEL_PLAIN};
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
            ("SOURCE_TOKEN", SENTINEL_PLAIN),
            ("OPENAI_API_KEY", "inherited-value"),
            ("TEST_PROBE_EXIT", "7"),
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

    assert_exit(&run, 7, "injected target");
    assert_eq!(run.stdout, "out");
    assert_eq!(run.stderr, "err");

    let report = fs::read_to_string(report).expect("the target writes its environment report");
    assert!(report.contains("argv\tprobe-argument\n"));
    assert!(
        report.contains(&format!("env\tOPENAI_API_KEY={SENTINEL_PLAIN}\n")),
        "the probe receives the resolved credential under OPENAI_API_KEY"
    );
    assert!(
        report.contains(&format!("env\tLLM_API_KEY={SENTINEL_PLAIN}\n")),
        "the probe receives the resolved credential under LLM_API_KEY"
    );
    assert!(report.contains("env\tOPENAI_BASE_URL=https://llm.example.com/v1\n"));
    assert!(report.contains("env\tCI_ENABLED=true\n"));
}

#[test]
fn run_does_not_scan_credential_references_inside_arrays() {
    let workspace = Workspace::new();
    let canary = workspace.path("canary-ran");
    let report = workspace.path("probe-report");
    let probe = assert_cmd::cargo::cargo_bin("test-probe");
    let canary_provider =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/canary_provider.sh");
    let config = workspace.stage_config(&array_only_reference_config(&canary_provider, &canary));

    let run = run_ac(
        &config,
        &[(
            "TEST_PROBE_OUT",
            report.to_str().expect("report path is UTF-8"),
        )],
        &[
            "run",
            "--with",
            "array_only",
            "--",
            probe.to_str().expect("probe path is UTF-8"),
        ],
    );

    assert_exit(&run, 0, "array-only credential reference");
    assert!(
        !canary.exists(),
        "a credential reference inside an array must not resolve its provider"
    );
    let report = fs::read_to_string(report).expect("the target writes its environment report");
    assert!(
        !report.contains("env\tARRAY_ONLY_TOKEN="),
        "the probe receives no injection for an array-only credential reference"
    );
}

#[cfg(unix)]
#[test]
fn run_preserves_a_target_termination_signal() {
    use std::os::unix::process::ExitStatusExt;

    let workspace = Workspace::new();
    let config = workspace.stage_config(RUNNER_CONFIG);
    let output = std::process::Command::cargo_bin("agentenv")
        .expect("agentenv is built")
        .env_clear()
        .env("AGENTENV_FILE", config)
        .env("SOURCE_TOKEN", SENTINEL_PLAIN)
        .args([
            "run",
            "--with",
            "ci",
            "--",
            "/bin/sh",
            "-c",
            "kill -TERM $$",
        ])
        .output()
        .expect("agentenv launches");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stdout.contains(SENTINEL_PLAIN),
        "agentenv leaked the injected sentinel on stdout"
    );
    assert!(
        !stderr.contains(SENTINEL_PLAIN),
        "agentenv leaked the injected sentinel on stderr"
    );
    assert_eq!(output.status.signal(), Some(15));
}

fn array_only_reference_config(canary_provider: &Path, canary: &Path) -> String {
    format!(
        "{RUNNER_CONFIG}\n\
         [profiles.work.array_only]\n\
         description = \"Entry whose credential reference appears only inside an array.\"\n\
         values = [\"credential://array_credential\"]\n\
         \n\
         [credentials.array_credential]\n\
         description = \"Canary credential that must remain unresolved.\"\n\
         provider = \"command\"\n\
         argv = [{provider:?}, {canary:?}, \"unused\"]\n\
         inject_as = \"ARRAY_ONLY_TOKEN\"\n",
        provider = canary_provider.to_str().expect("fixture path is UTF-8"),
        canary = canary.to_str().expect("canary path is UTF-8"),
    )
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
        let path = self.path("config.toml");
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
