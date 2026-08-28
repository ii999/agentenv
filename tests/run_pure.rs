//! Pure-environment coverage for `run --pure` and `--keep` (change 004).
//!
//! The probe target reports the environment it observes, so every assertion
//! about the child environment reads the probe report. Probe control
//! variables (`TEST_PROBE_OUT`, `TEST_PROBE_EXIT`) are not in the curated
//! base, so pure invocations carry them with `--keep` — which also keeps the
//! escape hatch itself under permanent test.

mod helpers;

use std::fs;
use std::path::{Path, PathBuf};

use helpers::{assert_exit, assert_mentions, run_ac, SENTINEL_NESTED, SENTINEL_PLAIN};
use tempfile::TempDir;

const RUNNER_CONFIG: &str = r#"
version = 1
default_profile = "work"

[profiles.work]
description = "Run test profile."

[profiles.work.llm]
description = "LLM entry."
credential = "credential://api"
endpoint = "https://llm.example.com/v1"

[profiles.work.llm.inject]
OPENAI_BASE_URL = "endpoint"

[profiles.alternate]
description = "Alternate profile for nested-invocation coverage."

[profiles.alternate.marker]
description = "Entry that exists only in the alternate profile."
flag = "alternate-only"

[credentials.api]
description = "Credential supplied by the test process."
provider = "env"
name = "SOURCE_TOKEN"
inject_as = "OPENAI_API_KEY"
"#;

/// Two entries targeting the same environment name: an injection conflict.
const CONFLICT_CONFIG: &str = r#"
version = 1
default_profile = "work"

[profiles.work]
description = "Conflict test profile."

[profiles.work.first]
description = "First entry."
value = "one"

[profiles.work.first.inject]
SHARED_TARGET = "value"

[profiles.work.second]
description = "Second entry."
value = "two"

[profiles.work.second.inject]
SHARED_TARGET = "value"
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
        .unwrap_or_else(|error| panic!("failed to restrict {}: {error}", path.display()));
}

#[cfg(not(unix))]
fn restrict_permissions(_path: &Path) {}

fn probe() -> String {
    assert_cmd::cargo::cargo_bin("test-probe")
        .to_str()
        .expect("probe path is UTF-8")
        .to_owned()
}

fn read_report(path: &Path) -> String {
    fs::read_to_string(path).expect("the target writes its environment report")
}

/// A curated-base name present on each platform's list, set by the test so
/// its carriage is observable.
#[cfg(unix)]
const PLATFORM_BASE_NAME: &str = "HOME";
#[cfg(windows)]
const PLATFORM_BASE_NAME: &str = "TEMP";

#[test]
fn pure_carries_base_keeps_and_injections_and_nothing_else() {
    // AC-001.1: base + keeps + injections reach the probe; a stray parent
    // variable does not.
    let workspace = Workspace::new();
    let config = workspace.stage_config(RUNNER_CONFIG);
    let report = workspace.path("probe-report");
    let home = workspace.directory.path().display().to_string();
    let probe = probe();

    let run = run_ac(
        &config,
        &[
            ("SOURCE_TOKEN", SENTINEL_PLAIN),
            (PLATFORM_BASE_NAME, &home),
            ("PARENT_SECRET", "stray-parent-value"),
            ("TEST_PROBE_OUT", report.to_str().expect("report is UTF-8")),
        ],
        &[
            "run",
            "--pure",
            "--keep",
            "TEST_PROBE_OUT",
            "--with",
            "llm",
            "--",
            &probe,
        ],
    );

    assert_exit(&run, 0, "pure run with a valid keep");
    assert_eq!(run.stdout, "out");
    assert_eq!(run.stderr, "err", "no missing-keep line is emitted");

    let report = read_report(&report);
    assert!(report.contains("env\tPATH="), "the base carries PATH");
    assert!(
        report.contains(&format!("env\t{PLATFORM_BASE_NAME}={home}\n")),
        "the base carries the parent {PLATFORM_BASE_NAME} value"
    );
    assert!(
        report.contains(&format!("env\tOPENAI_API_KEY={SENTINEL_PLAIN}\n")),
        "the injection reaches the probe"
    );
    assert!(
        report.contains("env\tOPENAI_BASE_URL=https://llm.example.com/v1\n"),
        "inject values reach the probe"
    );
    assert!(
        !report.contains("PARENT_SECRET"),
        "an unlisted parent variable never reaches the probe"
    );
}

#[cfg(unix)]
#[test]
fn pure_excludes_unlisted_lc_names() {
    // AC-001.2 (Unix-scoped by the spec) and AC-001.6: the locale names are
    // in the Unix base and the base is a closed list; LC_ prefix grants
    // nothing. On Windows the base carries no LC_ name at all, so exclusion
    // holds trivially and is covered by the seam unit test.
    let workspace = Workspace::new();
    let config = workspace.stage_config(RUNNER_CONFIG);
    let report = workspace.path("probe-report");
    let probe = probe();

    let run = run_ac(
        &config,
        &[
            ("SOURCE_TOKEN", "token"),
            ("LC_ALL", "C"),
            ("LC_CTYPE", "en_US.UTF-8"),
            ("LC_SECRET_TOKEN", "lc-smuggled-value"),
            ("TEST_PROBE_OUT", report.to_str().expect("report is UTF-8")),
        ],
        &[
            "run",
            "--pure",
            "--keep",
            "TEST_PROBE_OUT",
            "--with",
            "llm",
            "--",
            &probe,
        ],
    );

    assert_exit(&run, 0, "pure run with locale variables");
    let report = read_report(&report);
    assert!(report.contains("env\tLC_ALL=C\n"), "LC_ALL is in the base");
    assert!(
        report.contains("env\tLC_CTYPE=en_US.UTF-8\n"),
        "LC_CTYPE is in the base"
    );
    assert!(
        !report.contains("LC_SECRET_TOKEN"),
        "an unlisted LC_ name is excluded like any other name"
    );
}

#[test]
fn pure_base_names_absent_from_parent_stay_absent() {
    // AC-001.3: nothing is synthesized.
    let workspace = Workspace::new();
    let config = workspace.stage_config(RUNNER_CONFIG);
    let report = workspace.path("probe-report");
    let probe = probe();

    let run = run_ac(
        &config,
        &[
            ("SOURCE_TOKEN", "token"),
            ("TEST_PROBE_OUT", report.to_str().expect("report is UTF-8")),
        ],
        &[
            "run",
            "--pure",
            "--keep",
            "TEST_PROBE_OUT",
            "--with",
            "llm",
            "--",
            &probe,
        ],
    );

    assert_exit(&run, 0, "pure run without TERM in the parent");
    let report = read_report(&report);
    assert!(
        !report.contains("env\tTERM="),
        "a base name unset in the parent is absent from the child"
    );
}

#[test]
fn injection_overrides_an_inherited_value_under_pure() {
    // AC-001.4: injections are layered last.
    let workspace = Workspace::new();
    let config = workspace.stage_config(RUNNER_CONFIG);
    let report = workspace.path("probe-report");
    let probe = probe();

    let run = run_ac(
        &config,
        &[
            ("SOURCE_TOKEN", SENTINEL_PLAIN),
            ("OPENAI_API_KEY", "inherited-value"),
            ("TEST_PROBE_OUT", report.to_str().expect("report is UTF-8")),
        ],
        &[
            "run",
            "--pure",
            "--keep",
            "TEST_PROBE_OUT",
            "--keep",
            "OPENAI_API_KEY",
            "--with",
            "llm",
            "--",
            &probe,
        ],
    );

    assert_exit(&run, 0, "pure run with an overridden keep");
    let report = read_report(&report);
    assert!(
        report.contains(&format!("env\tOPENAI_API_KEY={SENTINEL_PLAIN}\n")),
        "the injected value wins over the kept parent value"
    );
    assert!(!report.contains("inherited-value"));
}

#[test]
fn default_run_still_inherits_every_non_overridden_variable() {
    // AC-001.5: without --pure, behavior is unchanged.
    let workspace = Workspace::new();
    let config = workspace.stage_config(RUNNER_CONFIG);
    let report = workspace.path("probe-report");
    let probe = probe();

    let run = run_ac(
        &config,
        &[
            ("SOURCE_TOKEN", "token"),
            ("PARENT_SECRET", "stray-parent-value"),
            ("TEST_PROBE_OUT", report.to_str().expect("report is UTF-8")),
        ],
        &["run", "--with", "llm", "--", &probe],
    );

    assert_exit(&run, 0, "default run");
    let report = read_report(&report);
    assert!(
        report.contains("env\tPARENT_SECRET=stray-parent-value\n"),
        "without --pure the parent environment passes through"
    );
}

#[test]
fn nested_agentenv_resolves_the_same_config_and_profile_under_pure() {
    // AC-001.8: AGENTENV_FILE and AGENTENV_PROFILE are in the base, so a
    // nested agentenv invocation inside the pure target selects the same
    // file and the same profile. The outer run pins its own profile with
    // --profile (which outranks the environment), so only the nested
    // invocation reads AGENTENV_PROFILE.
    let workspace = Workspace::new();
    let config = workspace.stage_config(RUNNER_CONFIG);
    let nested = assert_cmd::cargo::cargo_bin("agentenv");

    let run = run_ac(
        &config,
        &[("SOURCE_TOKEN", "token"), ("AGENTENV_PROFILE", "alternate")],
        &[
            "run",
            "--profile",
            "work",
            "--pure",
            "--with",
            "llm",
            "--",
            nested.to_str().expect("agentenv path is UTF-8"),
            "list",
        ],
    );

    assert_exit(&run, 0, "nested agentenv under --pure");
    assert!(
        run.stdout.contains("marker"),
        "the nested invocation resolves AGENTENV_PROFILE from the pure base\nstdout: {}\nstderr: {}",
        run.stdout,
        run.stderr
    );
    assert!(
        !run.stdout.contains("llm"),
        "the nested invocation lists the alternate profile, not the outer one\nstdout: {}",
        run.stdout
    );
}

#[test]
fn keep_carries_a_parent_value_and_duplicates_collapse() {
    // AC-002.1, AC-002.5, EDGE-002.
    let workspace = Workspace::new();
    let config = workspace.stage_config(RUNNER_CONFIG);
    let report = workspace.path("probe-report");
    let probe = probe();

    let run = run_ac(
        &config,
        &[
            ("SOURCE_TOKEN", "token"),
            ("AWS_REGION", "eu-west-1"),
            ("TEST_PROBE_OUT", report.to_str().expect("report is UTF-8")),
        ],
        &[
            "run",
            "--pure",
            "--keep",
            "TEST_PROBE_OUT",
            "--keep",
            "AWS_REGION",
            "--keep",
            "AWS_REGION",
            "--keep",
            "PATH",
            "--with",
            "llm",
            "--",
            &probe,
        ],
    );

    assert_exit(&run, 0, "pure run with duplicate keeps and a base keep");
    assert_eq!(run.stderr, "err", "no missing-keep line for present names");
    let report = read_report(&report);
    assert_eq!(
        report.matches("env\tAWS_REGION=eu-west-1\n").count(),
        1,
        "duplicate keeps carry the variable once"
    );
    assert_eq!(
        report.matches("env\tPATH=").count(),
        1,
        "keeping a base name is a no-op"
    );
}

#[test]
fn missing_keep_is_reported_once_and_the_run_continues() {
    // AC-002.2, AC-002.5 (duplicate missing names), AC-003.3, AC-004.3, and
    // EDGE-004: the report is inheritance-only, so a kept injection target
    // absent from the parent is reported and still injected.
    let workspace = Workspace::new();
    let config = workspace.stage_config(RUNNER_CONFIG);
    let report = workspace.path("probe-report");
    let probe = probe();

    let run = run_ac(
        &config,
        &[
            ("SOURCE_TOKEN", "token"),
            ("TEST_PROBE_EXIT", "7"),
            ("TEST_PROBE_OUT", report.to_str().expect("report is UTF-8")),
        ],
        &[
            "run",
            "--pure",
            "--keep",
            "TEST_PROBE_OUT",
            "--keep",
            "TEST_PROBE_EXIT",
            "--keep",
            "NOT_SET_ANYWHERE",
            "--keep",
            "NOT_SET_ANYWHERE",
            "--keep",
            "OPENAI_API_KEY",
            "--with",
            "llm",
            "--",
            &probe,
        ],
    );

    assert_exit(&run, 7, "the target's exit status is preserved");
    assert_eq!(
        run.stderr
            .lines()
            .filter(|line| line.contains("NOT_SET_ANYWHERE"))
            .count(),
        1,
        "duplicate missing keeps produce one report line\nstderr: {}",
        run.stderr
    );
    assert_eq!(
        run.stderr
            .lines()
            .filter(|line| line.contains("OPENAI_API_KEY"))
            .count(),
        1,
        "a kept injection target absent from the parent is reported (inheritance-only)\nstderr: {}",
        run.stderr
    );
    assert_mentions(&run, "not set", "the report states the condition");
    assert_mentions(&run, "--keep", "the report names the flag");
    let report = read_report(&report);
    assert!(
        !report.contains("NOT_SET_ANYWHERE"),
        "the missing name is absent from the child"
    );
    assert!(
        report.contains("env\tOPENAI_API_KEY=token\n"),
        "the injection still supplies the reported name"
    );
}

#[test]
fn missing_keep_line_survives_an_injection_conflict() {
    // AC-002.6: the report precedes conflict detection and the run still
    // exits 4 without resolving a provider.
    let workspace = Workspace::new();
    let config = workspace.stage_config(CONFLICT_CONFIG);

    let run = run_ac(
        &config,
        &[],
        &[
            "run",
            "--pure",
            "--keep",
            "NOT_SET_ANYWHERE",
            "--with",
            "first",
            "--with",
            "second",
            "--",
            "unused-target",
        ],
    );

    assert_exit(&run, 4, "the conflict still fails the run");
    assert_mentions(&run, "NOT_SET_ANYWHERE", "the missing-keep line appears");
    assert_mentions(
        &run,
        "injection conflict",
        "the conflict diagnostic appears",
    );
}

#[test]
fn pure_conflict_diagnostic_matches_the_default_one() {
    // AC-003.2: conflict semantics are unchanged by --pure.
    let workspace = Workspace::new();
    let config = workspace.stage_config(CONFLICT_CONFIG);
    let args_tail = ["--with", "first", "--with", "second", "--", "unused-target"];

    let mut pure_args = vec!["run", "--pure"];
    pure_args.extend_from_slice(&args_tail);
    let pure = run_ac(&config, &[], &pure_args);

    let mut default_args = vec!["run"];
    default_args.extend_from_slice(&args_tail);
    let default = run_ac(&config, &[], &default_args);

    assert_exit(&pure, 4, "pure conflict");
    assert_exit(&default, 4, "default conflict");
    assert_eq!(
        pure.stderr, default.stderr,
        "the conflict diagnostic is identical with and without --pure"
    );
}

#[test]
fn invalid_keep_names_are_usage_errors_before_launch() {
    // AC-002.3, AC-004.3: exit 1, the flag and grammar are named, and the
    // target never runs.
    let workspace = Workspace::new();
    let config = workspace.stage_config(RUNNER_CONFIG);
    let report = workspace.path("probe-report");
    let probe = probe();

    for bad in ["BAD-NAME", "9LEADING", ""] {
        let run = run_ac(
            &config,
            &[
                ("SOURCE_TOKEN", "token"),
                ("TEST_PROBE_OUT", report.to_str().expect("report is UTF-8")),
            ],
            &[
                "run", "--pure", "--keep", bad, "--with", "llm", "--", &probe,
            ],
        );
        assert_exit(&run, 1, "an invalid keep name is a usage error");
        assert_mentions(&run, "--keep", "the diagnostic names the flag");
        assert_mentions(
            &run,
            "[A-Za-z_][A-Za-z0-9_]*",
            "the diagnostic states the grammar",
        );
        assert!(
            run.stdout.is_empty(),
            "the target is not launched on a usage error (the probe would print 'out')\nstdout: {}",
            run.stdout
        );
    }
}

#[test]
fn keep_without_pure_is_a_usage_error() {
    // AC-002.4.
    let workspace = Workspace::new();
    let config = workspace.stage_config(RUNNER_CONFIG);
    let report = workspace.path("probe-report");
    let probe = probe();

    let run = run_ac(
        &config,
        &[
            ("SOURCE_TOKEN", "token"),
            ("TEST_PROBE_OUT", report.to_str().expect("report is UTF-8")),
        ],
        &["run", "--keep", "AWS_REGION", "--with", "llm", "--", &probe],
    );

    assert_exit(&run, 1, "--keep without --pure");
    assert_mentions(
        &run,
        "--keep requires --pure",
        "the diagnostic names the fix",
    );
    assert!(!report.exists(), "the target is not launched");
}

#[test]
fn invalid_keep_argument_value_is_never_echoed() {
    // AC-004.2: nothing at or after the first '=' reaches any channel.
    // run_ac asserts every sentinel is absent from both channels.
    let workspace = Workspace::new();
    let config = workspace.stage_config(RUNNER_CONFIG);
    let keep_argument = format!("API_KEY={SENTINEL_PLAIN}");

    let run = run_ac(
        &config,
        &[("SOURCE_TOKEN", "token")],
        &[
            "run",
            "--pure",
            "--keep",
            &keep_argument,
            "--with",
            "llm",
            "--",
            "unused-target",
        ],
    );

    assert_exit(&run, 1, "a keep argument with '=' is a usage error");
    assert_mentions(&run, "'API_KEY'", "the name portion may be echoed");
    assert_mentions(&run, "includes '='", "the diagnostic states the violation");
}

#[test]
fn new_diagnostic_paths_never_echo_parent_values() {
    // AC-004.1: sentinel-bearing parent variables stay out of agentenv's own
    // output on every new diagnostic path. run_ac asserts sentinel absence on
    // both channels of every invocation.
    let workspace = Workspace::new();
    let config = workspace.stage_config(RUNNER_CONFIG);
    let probe = probe();
    let sentinel_envs = [
        ("SOURCE_TOKEN", "token"),
        ("STRAY_ONE", SENTINEL_PLAIN),
        ("STRAY_TWO", SENTINEL_NESTED),
    ];

    // Invalid keep: no launch.
    let invalid = run_ac(
        &config,
        &sentinel_envs,
        &[
            "run", "--pure", "--keep", "BAD-NAME", "--with", "llm", "--", &probe,
        ],
    );
    assert_exit(&invalid, 1, "invalid keep with sentinels in the parent");

    // Keep without pure: no launch.
    let without_pure = run_ac(
        &config,
        &sentinel_envs,
        &["run", "--keep", "STRAY_ONE", "--with", "llm", "--", &probe],
    );
    assert_exit(&without_pure, 1, "keep without pure with sentinels");

    // Missing keep: the target launches silently (no TEST_PROBE_OUT).
    let missing = run_ac(
        &config,
        &sentinel_envs,
        &[
            "run",
            "--pure",
            "--keep",
            "NOT_SET_ANYWHERE",
            "--with",
            "llm",
            "--",
            &probe,
        ],
    );
    assert_exit(&missing, 0, "missing keep with sentinels");
}

#[cfg(unix)]
#[test]
fn unfindable_target_exits_127_and_absolute_targets_need_no_path() {
    // EDGE-001: with the parent PATH unset, a pure child has no PATH; an
    // absolute target still launches, and a target no permitted location can
    // supply exits 127.
    use helpers::command_with_project_discovery;

    let workspace = Workspace::new();
    let config = workspace.stage_config(RUNNER_CONFIG);
    let probe = probe();

    let mut absolute = command_with_project_discovery(&config);
    absolute.env_remove("PATH");
    absolute.env("AGENTENV_NO_PROJECT", "1");
    absolute.env("SOURCE_TOKEN", "token");
    absolute.args(["run", "--pure", "--with", "llm", "--", &probe]);
    let output = absolute.output().expect("agentenv runs without PATH");
    assert_eq!(
        output.status.code(),
        Some(0),
        "an absolute target launches without a child PATH\nstderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let mut unfindable = command_with_project_discovery(&config);
    unfindable.env_remove("PATH");
    unfindable.env("AGENTENV_NO_PROJECT", "1");
    unfindable.env("SOURCE_TOKEN", "token");
    unfindable.args([
        "run",
        "--pure",
        "--with",
        "llm",
        "--",
        "agentenv-test-absent-target-9x7q3",
    ]);
    let output = unfindable.output().expect("agentenv runs without PATH");
    assert_eq!(
        output.status.code(),
        Some(127),
        "a target found nowhere exits 127\nstderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[cfg(windows)]
#[test]
fn windows_carries_the_parent_path_spelling_and_value_once() {
    // AC-001.7 / EDGE-006: the parent spells the variable `Path`; the base
    // lists `PATH`. Case-insensitive equivalence carries it exactly once,
    // under the parent's own spelling with the parent's own value. The
    // harness would re-add `PATH`, so this test builds the command directly.
    use helpers::command_with_project_discovery;

    let workspace = Workspace::new();
    let config = workspace.stage_config(RUNNER_CONFIG);
    let report = workspace.path("probe-report");
    let probe = probe();
    let parent_path = std::env::var("PATH").expect("the test process has a PATH");

    let mut command = command_with_project_discovery(&config);
    command.env_remove("PATH");
    command.env("Path", &parent_path);
    command.env("AGENTENV_NO_PROJECT", "1");
    command.env("SOURCE_TOKEN", "token");
    command.env("TEST_PROBE_OUT", report.to_str().expect("report is UTF-8"));
    command.args([
        "run",
        "--pure",
        "--keep",
        "TEST_PROBE_OUT",
        "--keep",
        "PATH",
        "--with",
        "llm",
        "--",
        &probe,
    ]);

    let output = command.output().expect("agentenv runs on Windows");
    assert_eq!(
        output.status.code(),
        Some(0),
        "pure run on Windows\nstderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report = read_report(&report);
    assert!(
        report.contains(&format!("env\tPath={parent_path}\n")),
        "the parent spelling and value survive"
    );
    assert!(
        !report.contains("env\tPATH="),
        "the base-list spelling does not replace the parent's"
    );
}
