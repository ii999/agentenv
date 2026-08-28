//! Cross-cutting security coverage for project discovery, trust, and injection.
//!
//! The project-file surfaces must not expose project values or execute providers.
//! A trusted pin may select a profile for `run`, but it cannot introduce any
//! injection name or credential source of its own.

mod helpers;

use std::fs;
use std::path::{Path, PathBuf};

use helpers::{
    assert_exit, command_with_project_discovery, staged_config, Run, SENTINELS, STATE_BASE_ENV,
};
use tempfile::TempDir;

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
        let cwd = root.join("workspace/nested");
        fs::create_dir_all(&cwd).expect("test working directory is created");
        let project = root.join("workspace/.agentenv.toml");
        fs::write(&project, project_text).expect("project file is written");

        Self {
            _config_dir: config_dir,
            config,
            cwd,
            project,
            state: root.join("state"),
        }
    }

    fn run(&self, args: &[&str], envs: &[(&str, &str)]) -> Run {
        let mut command = command_with_project_discovery(&self.config);
        command.current_dir(&self.cwd);
        command.env(STATE_BASE_ENV, &self.state);
        for (name, value) in envs {
            command.env(name, value);
        }
        command.args(args);

        let output = command.output().expect("agentenv runs");
        let run = Run {
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            code: output.status.code(),
        };
        assert_no_sentinels(&run, args);
        run
    }

    fn allow(&self) -> Run {
        self.run(&["project", "allow"], &[])
    }
}

#[test]
fn ac_010_1_invalid_project_values_never_reach_allow_status_or_notices() {
    let fixture = ProjectFixture::new(
        basic_config(),
        &format!(
            "version = 1\n\
             unknown = \"{}\"\n\
             profile = \"credential://{}\"\n\
             [requires.runner]\n\
             reason = \"credential://{}\"\n\
             fields = [\"model\", \"{}\", \"{}\"]\n",
            SENTINELS[0], SENTINELS[1], SENTINELS[2], SENTINELS[3], SENTINELS[3]
        ),
    );

    let allow = fixture.allow();
    assert_exit(&allow, 2, "allow rejects an invalid project file");

    let status = fixture.run(&["project", "status", "--json"], &[]);
    assert_exit(&status, 5, "status reports an invalid project file");

    let notice = fixture.run(&["get", "llm.model"], &[]);
    assert_exit(&notice, 0, "an invalid untrusted project remains inert");
    assert!(notice
        .stderr
        .contains(&fixture.project.display().to_string()));
    assert!(notice.stderr.contains("agentenv project status"));
}

#[test]
fn ac_010_2_untrusted_profile_and_reason_values_never_reach_regular_commands() {
    let fixture = ProjectFixture::new(
        basic_config(),
        &format!(
            "version = 1\n\
             profile = \"{}\"\n\
             [requires.runner]\n\
             reason = \"{}\"\n",
            SENTINELS[0], SENTINELS[1]
        ),
    );

    let run = fixture.run(&["get", "llm.model"], &[]);
    assert_exit(&run, 0, "an untrusted valid project remains inert");
    assert_eq!(run.stdout, "default-model\n");
    assert!(run.stderr.contains(&fixture.project.display().to_string()));
    assert!(run.stderr.contains("agentenv project status"));
}

#[cfg(unix)]
#[test]
fn ac_010_4_project_operations_do_not_execute_counting_providers() {
    let workspace = TempDir::new().expect("a provider workspace is created");
    let counter = workspace.path().join("provider-count");
    let provider =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/counting_provider.sh");
    let config = format!(
        "version = 1\n\
         default_profile = \"work\"\n\
         \n\
         [profiles.work]\n\
         description = \"Work\"\n\
         \n\
         [profiles.work.runner]\n\
         description = \"Runner\"\n\
         credential = \"credential://counted\"\n\
         \n\
         [credentials.counted]\n\
         description = \"Counted provider\"\n\
         provider = \"command\"\n\
         argv = [{provider:?}, {counter:?}, \"provider-value\"]\n\
         inject_as = \"COUNTED_TOKEN\"\n"
    );
    let fixture = ProjectFixture::new(&config, "version = 1\nprofile = \"work\"\n");

    let untrusted_status = fixture.run(&["project", "status", "--json"], &[]);
    assert_exit(
        &untrusted_status,
        5,
        "untrusted status does not resolve providers",
    );

    let allow = fixture.allow();
    assert_exit(&allow, 0, "allow does not resolve providers");

    let trusted_status = fixture.run(&["project", "status", "--json"], &[]);
    assert_exit(
        &trusted_status,
        0,
        "trusted status does not resolve providers",
    );

    let revoke = fixture.run(&["project", "revoke"], &[]);
    assert_exit(&revoke, 0, "revoke does not resolve providers");
    assert!(
        !counter.exists(),
        "project status, allow, and revoke must leave the counting provider untouched"
    );
}

#[test]
fn ac_010_5_trusted_pin_selects_exactly_the_pinned_profiles_injection_plan() {
    let fixture = ProjectFixture::new(
        pinned_injection_config(),
        "version = 1\nprofile = \"work\"\n",
    );
    let allowed = fixture.allow();
    assert_exit(&allowed, 0, "the project pin is approved before run");

    let report_path = fixture.cwd.join("probe-report");
    let probe = assert_cmd::cargo::cargo_bin("test-probe");
    let run = fixture.run(
        &[
            "run",
            "--with",
            "runner",
            "--",
            probe.to_str().expect("probe path is UTF-8"),
        ],
        &[
            (
                "TEST_PROBE_OUT",
                report_path.to_str().expect("report path is UTF-8"),
            ),
            ("WORK_CREDENTIAL_SOURCE", SENTINELS[0]),
            ("PERSONAL_CREDENTIAL_SOURCE", SENTINELS[1]),
        ],
    );

    assert_exit(&run, 0, "run succeeds with the trusted pinned profile");
    assert_eq!(run.stdout, "out");
    assert_eq!(run.stderr, "err");

    let report = fs::read_to_string(report_path).expect("the probe report is written");
    assert_probe_value(&report, "WORK_CREDENTIAL_TARGET", SENTINELS[0]);
    assert_probe_value(&report, "WORK_PROJECT_MARKER", "work");
    assert_probe_absent(&report, "PERSONAL_CREDENTIAL_TARGET");
    assert_probe_absent(&report, "PERSONAL_PROJECT_MARKER");
}

#[cfg(unix)]
#[test]
fn ac_005_3_unreadable_approved_project_file_fails_loudly_with_exit_2() {
    // A trusted file that becomes unreadable while its approval record
    // remains must fail profile resolution with exit 2, naming the file and
    // a next action — never degrade silently to no-project behavior.
    use std::os::unix::fs::PermissionsExt;

    let fixture = ProjectFixture::new(basic_config(), "version = 1\nprofile = \"default\"\n");
    let allow = fixture.allow();
    assert_exit(&allow, 0, "the project file is approved while readable");

    fs::set_permissions(&fixture.project, fs::Permissions::from_mode(0o000))
        .expect("project file permissions are restricted");

    let run = fixture.run(&["get", "llm.model"], &[]);
    assert_exit(
        &run,
        2,
        "an unreadable approved project file is a hard error",
    );
    assert!(
        run.stdout.is_empty(),
        "no value is produced: {}",
        run.stdout
    );
    assert!(
        run.stderr.contains(&fixture.project.display().to_string()),
        "the diagnostic names the project file: {}",
        run.stderr
    );
    assert!(
        run.stderr.contains("agentenv project revoke"),
        "the diagnostic names a next action: {}",
        run.stderr
    );

    fs::set_permissions(&fixture.project, fs::Permissions::from_mode(0o644))
        .expect("project file permissions are restored for cleanup");
}

#[test]
fn ac_010_3_status_renders_the_full_envelope_without_config_values() {
    // Sentinels sit in every user-config position a secret could occupy:
    // open-schema field values, nested values, profile and entry
    // descriptions, and credential definition members. The full status
    // report must render every envelope member and leak none of them.
    let config = format!(
        "version = 1\n\
         default_profile = \"default\"\n\
         \n\
         [profiles.default]\n\
         description = \"Profile description {}\"\n\
         \n\
         [profiles.default.llm]\n\
         description = \"Entry description {}\"\n\
         model = \"{}\"\n\
         \n\
         [profiles.default.llm.limits]\n\
         budget = \"{}\"\n\
         \n\
         [credentials.tool]\n\
         description = \"Credential description {}\"\n\
         provider = \"command\"\n\
         argv = [\"/bin/echo\", \"{}\"]\n\
         inject_as = \"TOOL_TOKEN\"\n",
        SENTINELS[0], SENTINELS[1], SENTINELS[2], SENTINELS[3], SENTINELS[4], SENTINELS[0]
    );
    let fixture = ProjectFixture::new(
        &config,
        "version = 1\n\
         profile = \"default\"\n\
         [requires.llm]\n\
         reason = \"Model access is required for runs\"\n\
         fields = [\"model\"]\n",
    );
    let allow = fixture.allow();
    assert_exit(&allow, 0, "the pinned project file is approved");

    // fixture.run asserts no sentinel reaches stdout or stderr on every
    // invocation; the assertions below pin the envelope's presence.
    let json = fixture.run(&["project", "status", "--json"], &[]);
    assert_exit(&json, 0, "a satisfied requirement reports status 0");
    for member in [
        "\"version\"",
        "\"default\"",
        "\"llm\"",
        "Model access is required for runs",
    ] {
        assert!(
            json.stdout.contains(member),
            "the JSON report renders {member}: {}",
            json.stdout
        );
    }

    let text = fixture.run(&["project", "status"], &[]);
    assert_exit(&text, 0, "the text report also succeeds");
    assert!(
        text.stdout.contains("Model access is required for runs"),
        "the text report renders requirement reasons: {}",
        text.stdout
    );
}

fn basic_config() -> &'static str {
    r#"
version = 1
default_profile = "default"

[profiles.default]
description = "Default"

[profiles.default.llm]
description = "Language model"
model = "default-model"
"#
}

fn pinned_injection_config() -> &'static str {
    r#"
version = 1
default_profile = "personal"

[profiles.work]
description = "Work"

[profiles.work.runner]
description = "Work runner"
credential = "credential://work"
marker = "work"

[profiles.work.runner.inject]
WORK_PROJECT_MARKER = "marker"

[profiles.personal]
description = "Personal"

[profiles.personal.runner]
description = "Personal runner"
credential = "credential://personal"
marker = "personal"

[profiles.personal.runner.inject]
PERSONAL_PROJECT_MARKER = "marker"

[credentials.work]
description = "Work credential"
provider = "env"
name = "WORK_CREDENTIAL_SOURCE"
inject_as = "WORK_CREDENTIAL_TARGET"

[credentials.personal]
description = "Personal credential"
provider = "env"
name = "PERSONAL_CREDENTIAL_SOURCE"
inject_as = "PERSONAL_CREDENTIAL_TARGET"
"#
}

fn assert_no_sentinels(run: &Run, args: &[&str]) {
    for (index, sentinel) in SENTINELS.iter().enumerate() {
        for text in [&run.stdout, &run.stderr] {
            assert!(
                !text.contains(sentinel),
                "agentenv {args:?} leaked sentinel #{index}"
            );
        }
    }
}

fn assert_probe_value(report: &str, name: &str, expected: &str) {
    assert!(
        probe_value(report, name) == Some(expected),
        "the probe must receive {name} from the pinned profile"
    );
}

fn assert_probe_absent(report: &str, name: &str) {
    assert!(
        probe_value(report, name).is_none(),
        "the probe must not receive {name} from another profile"
    );
}

fn probe_value<'a>(report: &'a str, name: &str) -> Option<&'a str> {
    report.lines().find_map(|line| {
        let record = line.strip_prefix("env\t")?;
        record.strip_prefix(name)?.strip_prefix('=')
    })
}
