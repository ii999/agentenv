//! Integration coverage for the project command group's frozen status report.

mod helpers;

use std::fs;
use std::path::PathBuf;

use helpers::{assert_exit, command_with_project_discovery, staged_config};
use serde_json::Value;
use tempfile::TempDir;

const CONFIG: &str = r#"
version = 1
default_profile = "work"

[profiles.work]
description = "Work"

[profiles.work.llm]
description = "Language model"
endpoint = "https://example.invalid"
credential = "credential://token"

[profiles.work.llm.auth]
endpoint = "https://auth.example.invalid"

[profiles.work.table_value]
description = "Table value"
[profiles.work.table_value.settings]
enabled = true

[credentials.token]
description = "Token"
provider = "command"
argv = ["tests/fixtures/counting_provider.sh", "/tmp/project-status-provider-count", "not-a-secret"]
inject_as = "TOKEN"
"#;

struct Fixture {
    _config_dir: TempDir,
    config: PathBuf,
    cwd: PathBuf,
    state: PathBuf,
}

impl Fixture {
    fn new(project_text: Option<&str>) -> Self {
        let (config_dir, config) = staged_config(CONFIG);
        let root = config.parent().expect("config has a parent").to_owned();
        let cwd = root.join("workspace/nested");
        fs::create_dir_all(&cwd).expect("working directory is created");
        let project = root.join("workspace/.agentenv.toml");
        if let Some(project_text) = project_text {
            fs::write(&project, project_text).expect("project file is written");
        }
        Self {
            _config_dir: config_dir,
            config,
            cwd,
            state: root.join("state"),
        }
    }

    fn approve(&self) {
        let env = |name: &str| match name {
            "XDG_STATE_HOME" => Some(self.state.display().to_string()),
            _ => None,
        };
        agentenv::project::allow(&self.cwd, &env).expect("project is approved");
    }

    fn run(&self, args: &[&str]) -> helpers::Run {
        let mut command = command_with_project_discovery(&self.config);
        command.current_dir(&self.cwd);
        command.env("XDG_STATE_HOME", &self.state);
        command.args(args);
        into_run(command.output().expect("agentenv runs"))
    }

    fn run_without_state_base(&self, args: &[&str]) -> helpers::Run {
        let mut command = command_with_project_discovery(&self.config);
        command.current_dir(&self.cwd);
        command.env_remove("XDG_STATE_HOME");
        command.env_remove("HOME");
        command.args(args);
        into_run(command.output().expect("agentenv runs"))
    }
}

fn into_run(output: std::process::Output) -> helpers::Run {
    helpers::Run {
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        code: output.status.code(),
    }
}

fn project_with_requirements() -> &'static str {
    r#"
version = 1
profile = "work"

[requires.llm]
reason = "LLM access is required"
fields = ["auth", "credential"]

[requires.table_value]
reason = "Table values are accepted"
fields = ["settings"]
"#
}

fn status_json(run: &helpers::Run) -> Value {
    assert!(run.stderr.is_empty(), "project commands emit no notice");
    serde_json::from_str(&run.stdout).expect("status output is JSON")
}

fn normalized(mut value: Value) -> String {
    let project = value
        .pointer_mut("/project/path")
        .expect("status has a project path member");
    if project.is_string() {
        *project = Value::String("<project>".to_owned());
    }
    if let Some(violations) = value
        .pointer_mut("/project/violations")
        .and_then(Value::as_array_mut)
    {
        for violation in violations {
            if let Some(path) = violation.get_mut("path") {
                if path.is_string() {
                    *path = Value::String("<project>".to_owned());
                }
            }
        }
    }
    serde_json::to_string_pretty(&value).expect("normalized JSON serializes") + "\n"
}

fn assert_snapshot(state: &str, value: Value) {
    let expected = match state {
        "no-file" => include_str!("snapshots/project-status-no-file.json"),
        "untrusted" => include_str!("snapshots/project-status-untrusted.json"),
        "invalid" => include_str!("snapshots/project-status-invalid.json"),
        "unavailable" => include_str!("snapshots/project-status-unavailable.json"),
        "degraded" => include_str!("snapshots/project-status-degraded.json"),
        "checked" => include_str!("snapshots/project-status-checked.json"),
        _ => panic!("unknown snapshot state: {state}"),
    };
    assert_eq!(normalized(value), expected, "{state} status snapshot");
}

#[test]
fn status_json_matches_the_frozen_member_state_table() {
    let no_file = Fixture::new(None);
    let no_file_run = no_file.run(&["project", "status", "--json"]);
    assert_exit(&no_file_run, 0, "no file exits successfully");
    assert_snapshot("no-file", status_json(&no_file_run));

    let untrusted = Fixture::new(Some("version = 1\nprofile = \"work\"\n"));
    let untrusted_run = untrusted.run(&["project", "status", "--json"]);
    assert_exit(&untrusted_run, 5, "new file exits with trust-state failure");
    assert_snapshot("untrusted", status_json(&untrusted_run));

    let invalid = Fixture::new(Some("version = \"wrong\"\n"));
    let invalid_run = invalid.run(&["project", "status", "--json"]);
    assert_exit(
        &invalid_run,
        5,
        "invalid file exits with trust-state failure",
    );
    assert_snapshot("invalid", status_json(&invalid_run));

    let unavailable = Fixture::new(Some("version = 1\n"));
    let unavailable_run = unavailable.run_without_state_base(&["project", "status", "--json"]);
    assert_exit(
        &unavailable_run,
        5,
        "unavailable trust state exits with trust-state failure",
    );
    assert_snapshot("unavailable", status_json(&unavailable_run));

    let degraded = Fixture::new(Some("version = 1\nprofile = \"work\"\n"));
    degraded.approve();
    fs::write(&degraded.config, "not TOML = [\n").expect("config becomes invalid");
    let degraded_run = degraded.run(&["project", "status", "--json"]);
    assert_exit(
        &degraded_run,
        0,
        "zero requirements ignore degraded selection for exit status",
    );
    assert_snapshot("degraded", status_json(&degraded_run));

    let checked = Fixture::new(Some(project_with_requirements()));
    checked.approve();
    let checked_run = checked.run(&["project", "status", "--json"]);
    assert_exit(&checked_run, 0, "satisfied requirements exit successfully");
    assert_snapshot("checked", status_json(&checked_run));
}

#[test]
fn status_reports_unsatisfied_requirements_without_running_providers() {
    let fixture = Fixture::new(Some(
        r#"
version = 1
profile = "work"

[requires.llm]
reason = "Needs a missing field"
fields = ["missing"]

[requires.missing_entry]
reason = "Needs an entry that is absent"
"#,
    ));
    fixture.approve();
    let counter = fixture.cwd.join("provider-count");
    let mut command = command_with_project_discovery(&fixture.config);
    command.current_dir(&fixture.cwd);
    command.env("XDG_STATE_HOME", &fixture.state);
    command.env("PROJECT_STATUS_COUNTER", &counter);
    command.args(["project", "status", "--json"]);
    let run = into_run(command.output().expect("agentenv runs"));

    assert_exit(&run, 6, "unsatisfied requirements exit with status 6");
    assert!(
        !counter.exists(),
        "structural checks never execute providers"
    );
    let report = status_json(&run);
    assert_eq!(
        report["project"]["requirements"]["entries"][0]["missing"],
        serde_json::json!(["missing"])
    );
    assert_eq!(
        report["project"]["requirements"]["entries"][1]["missing"],
        serde_json::json!(["entry missing_entry"])
    );
}

#[test]
fn allow_and_revoke_render_their_outcomes() {
    let fixture = Fixture::new(Some("version = 1\nprofile = \"work\"\n"));

    let allowed = fixture.run(&["project", "allow"]);
    assert_exit(&allowed, 0, "allow succeeds");
    assert!(allowed.stdout.contains("Approved project file"));
    assert!(allowed.stdout.contains("agentenv project status"));

    let already_current = fixture.run(&["project", "allow"]);
    assert_exit(&already_current, 0, "repeat allow succeeds");
    assert!(already_current.stdout.contains("already approved"));

    let revoked = fixture.run(&["project", "revoke"]);
    assert_exit(&revoked, 0, "revoke succeeds");
    assert!(revoked.stdout.contains("Revoked approval"));

    let missing = fixture.run(&["project", "revoke"]);
    assert_exit(&missing, 0, "second revoke succeeds");
    assert!(missing.stdout.contains("no approval record"));
}

#[test]
fn status_infrastructure_failures_leave_json_stdout_empty() {
    let fixture = Fixture::new(Some("version = 1\n"));
    let store = fixture.state.join("agentenv/trust.toml");
    fs::create_dir_all(store.parent().expect("trust store has a parent"))
        .expect("trust directory is created");
    fs::write(&store, "not a trust store").expect("corrupt trust store is written");

    let run = fixture.run(&["project", "status", "--json"]);
    assert_exit(&run, 2, "corrupt trust store is a configuration error");
    assert!(run.stdout.is_empty(), "exit 2 emits no JSON report");
    assert!(run.stderr.contains("trust.toml"));
}
