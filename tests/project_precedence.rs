//! Integration coverage for trusted project profile pins.

mod helpers;

use std::fs;
use std::path::PathBuf;

use helpers::{assert_exit, command_with_project_discovery, read_config, staged_config};
use tempfile::TempDir;

const PROFILES: &str = r#"
version = 1
default_profile = "personal"

[profiles.work]
description = "Work profile"

[profiles.work.llm]
description = "Work model"
model = "work-model"

[profiles.personal]
description = "Personal profile"

[profiles.personal.llm]
description = "Personal model"
model = "personal-model"
"#;

struct ProjectFixture {
    _config_dir: TempDir,
    config: PathBuf,
    cwd: PathBuf,
    project: PathBuf,
    state: PathBuf,
}

impl ProjectFixture {
    fn new(config_text: &str, project_text: &str) -> Self {
        let (config_dir, config) = staged_config(config_text);
        let root = config.parent().expect("config has a parent").to_owned();
        let cwd = root.join("nested/deeper");
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
}

#[test]
fn trusted_pin_precedes_default_and_yields_to_env_and_flag() {
    let fixture = ProjectFixture::new(PROFILES, "version = 1\nprofile = \"work\"\n");
    fixture.approve();

    let pinned = fixture.run(&["get", "llm.model"], &[]);
    assert_exit(&pinned, 0, "trusted pin selects the work profile");
    assert_eq!(pinned.stdout, "work-model\n");
    assert!(pinned.stderr.is_empty(), "trusted project emits no notice");

    let environment = fixture.run(&["get", "llm.model"], &[("AGENTENV_PROFILE", "personal")]);
    assert_exit(&environment, 0, "environment selection beats the pin");
    assert_eq!(environment.stdout, "personal-model\n");

    let flag = fixture.run(&["--profile", "personal", "get", "llm.model"], &[]);
    assert_exit(&flag, 0, "explicit profile selection beats the pin");
    assert_eq!(flag.stdout, "personal-model\n");
}

#[test]
fn dangling_trusted_pin_names_its_project_file() {
    let fixture = ProjectFixture::new(PROFILES, "version = 1\nprofile = \"missing\"\n");
    fixture.approve();

    let run = fixture.run(&["get", "llm.model"], &[]);
    assert_exit(
        &run,
        3,
        "an undefined trusted pin is a name-resolution error",
    );
    assert!(run.stderr.contains("missing"));
    assert!(run.stderr.contains(&fixture.project.display().to_string()));
}

#[test]
fn set_and_unset_follow_the_trusted_pin_but_create_profile_does_not() {
    let fixture = ProjectFixture::new(PROFILES, "version = 1\nprofile = \"work\"\n");
    fixture.approve();

    let set = fixture.run(&["set", "llm.model", "changed"], &[]);
    assert_exit(&set, 0, "set follows the trusted pin");
    let written = read_config(&fixture.config);
    assert!(
        written.contains("[profiles.work.llm]\ndescription = \"Work model\"\nmodel = \"changed\"")
    );
    assert!(written.contains(
        "[profiles.personal.llm]\ndescription = \"Personal model\"\nmodel = \"personal-model\""
    ));

    let unset = fixture.run(&["unset", "llm.model"], &[]);
    assert_exit(&unset, 0, "unset follows the trusted pin");
    let written = read_config(&fixture.config);
    assert!(written.contains("[profiles.work.llm]\ndescription = \"Work model\""));
    assert!(written.contains("model = \"personal-model\""));

    let create = fixture.run(&["set", "--create-profile", "new", "llm.model", "x"], &[]);
    assert_exit(
        &create,
        1,
        "create-profile still requires an explicit profile flag",
    );
    assert!(create
        .stderr
        .contains("--create-profile requires an explicit --profile"));
}

#[test]
fn run_uses_the_trusted_pin_for_injection_planning() {
    let config = r#"
version = 1
default_profile = "personal"

[profiles.work]
description = "Work"
[profiles.work.runner]
description = "Runner"
flag = "work"
[profiles.work.runner.inject]
TEST_PROJECT_PIN = "flag"

[profiles.personal]
description = "Personal"
[profiles.personal.runner]
description = "Runner"
flag = "personal"
[profiles.personal.runner.inject]
TEST_PROJECT_PIN = "flag"
"#;
    let fixture = ProjectFixture::new(config, "version = 1\nprofile = \"work\"\n");
    fixture.approve();
    let report = fixture.cwd.join("probe-report");
    let probe = assert_cmd::cargo::cargo_bin("test-probe");

    let run = fixture.run(
        &[
            "run",
            "--with",
            "runner",
            "--",
            probe.to_str().expect("probe path is UTF-8"),
        ],
        &[(
            "TEST_PROBE_OUT",
            report.to_str().expect("report path is UTF-8"),
        )],
    );
    assert_exit(&run, 0, "run follows the trusted pin");
    assert_eq!(run.stdout, "out");
    assert_eq!(run.stderr, "err");
    let report = fs::read_to_string(report).expect("probe report is written");
    assert!(report.contains("env\tTEST_PROJECT_PIN=work\n"));
}
