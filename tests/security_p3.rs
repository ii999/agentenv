//! Phase-3 security suite: the injection plan is built and conflict-checked
//! **before** any provider is touched, and one credential named several times
//! is resolved exactly once.
//!
//! Covers SPEC-016 (AC-016.2's canary half, AC-016.5, AC-016.8, and the
//! Unix-runnable rows of AC-016.9's matrix), SPEC-018 (AC-018.1 for the exit
//! codes `run` makes reachable in Phase 3) and SPEC-019 (every invocation goes
//! through `run_ac`, which greps the planted sentinels on both channels).
//!
//! Two fixture scripts carry the observations that the CLI's own output cannot:
//!
//! - `canary_provider.sh` creates a file when it executes, so a conflict row
//!   proves by that file's ABSENCE that no provider ran.
//! - `counting_provider.sh` appends one line per execution, so a dedup row
//!   asserts the resolution count exactly.
//!
//! The child environment is observed through the `test-probe` binary
//! (`tests/fixtures/bin/probe.rs`). `run` replaces agentenv with its
//! target, so the target itself reports what it received: with `TEST_PROBE_OUT`
//! pointing into the test's workspace, the probe writes one `argv\t<value>`
//! record per argument and one `env\t<NAME>=<VALUE>` record per variable. The
//! injected sentinels therefore live in the target's own file — the target's
//! channels are outside the SPEC-019 boundary — and never in agentenv's
//! output.
//!
//! AC-016.9's remaining row, the Windows case-variant pair, needs Windows
//! environment-name identity and is not part of this Unix-runnable matrix.
//!
//! Conflict diagnostics must name both sources. A credential source is named by
//! its credential name; an `inject`-table source is named by its identity —
//! entry plus inject key — rendered as the dotted `<entry>.inject.<KEY>` path
//! that `list <entry>` already uses for inject members.

mod helpers;

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use helpers::{assert_exit, run_ac, Run, SENTINEL_NESTED, SENTINEL_PLAIN};
use tempfile::TempDir;

const CANARY_PROVIDER: &str = "canary_provider.sh";
#[cfg(unix)]
const COUNTING_PROVIDER: &str = "counting_provider.sh";

/// Passed to the probe so the dedup rows also witness argv pass-through.
const PROBE_MARKER: &str = "probe-argument";

// ---------------------------------------------------------------------------
// AC-016.9 / AC-016.2: the conflict matrix
// ---------------------------------------------------------------------------

/// One conflict row: a config, the entries `--with` names, and the two sources
/// the diagnostic must name.
struct ConflictRow {
    name: &'static str,
    /// Builds the config from the staged canary script and the canary path the
    /// script would create.
    config: fn(&Path, &Path) -> String,
    entries: &'static [&'static str],
    sources: [&'static str; 2],
}

fn conflict_rows() -> Vec<ConflictRow> {
    vec![
        ConflictRow {
            name: "credential vs credential, across two entries",
            config: credential_vs_credential_across_entries,
            entries: &["alpha", "beta"],
            sources: ["canary_cred", "env_cred"],
        },
        ConflictRow {
            name: "credential vs credential, within one entry",
            config: credential_vs_credential_within_one_entry,
            entries: &["alpha"],
            sources: ["canary_cred", "env_cred"],
        },
        ConflictRow {
            name: "credential vs inject, across two entries",
            config: credential_vs_inject_across_entries,
            entries: &["alpha", "beta"],
            sources: ["canary_cred", "beta.inject.OPENAI_API_KEY"],
        },
        ConflictRow {
            name: "credential vs inject, within one entry",
            config: credential_vs_inject_within_one_entry,
            entries: &["alpha"],
            sources: ["canary_cred", "alpha.inject.OPENAI_API_KEY"],
        },
        ConflictRow {
            name: "inject vs inject, across two entries",
            config: inject_vs_inject_across_entries,
            entries: &["alpha", "beta"],
            sources: [
                "alpha.inject.OPENAI_BASE_URL",
                "beta.inject.OPENAI_BASE_URL",
            ],
        },
    ]
}

#[test]
fn ac_016_9_every_conflict_row_fails_before_any_provider_or_target_runs() {
    let probe = probe_binary();

    for row in conflict_rows() {
        let workspace = Workspace::new();
        let script = workspace.stage_script(CANARY_PROVIDER);
        let canary = workspace.path("canary-ran");
        let config = workspace.stage_config(&(row.config)(&script, &canary));
        let report = workspace.path("probe-report");

        let run = run_plan(
            &config,
            &plan_args(row.entries, &probe, &[PROBE_MARKER]),
            &report,
        );

        assert_exit(&run, 4, row.name);
        for source in row.sources {
            assert_stderr_mentions(&run, source, row.name);
        }
        assert!(
            !canary.exists(),
            "{}: a conflict must be detected before any provider is resolved, \
             but the canary provider ran\nstdout: {}\nstderr: {}",
            row.name,
            run.stdout,
            run.stderr
        );
        assert!(
            !report.exists(),
            "{}: a conflict must be detected before the target is launched, \
             but the probe ran\nstdout: {}\nstderr: {}",
            row.name,
            run.stdout,
            run.stderr
        );
    }
}

// ---------------------------------------------------------------------------
// AC-016.5 / AC-016.8 / AC-016.9: the dedup rows
// ---------------------------------------------------------------------------

/// One dedup row: identities that collapse, so the plan succeeds and the shared
/// credential is resolved exactly once. The rows execute the sh counting
/// provider, so the dedup machinery is unix-only.
#[cfg(unix)]
struct DedupRow {
    name: &'static str,
    /// Builds the config from the staged counting script and the counter file
    /// it appends to.
    config: fn(&Path, &Path) -> String,
    entries: &'static [&'static str],
    /// Names the probe must see carrying the counting provider's value.
    credential_targets: &'static [&'static str],
    /// Names the probe must see carrying a plain `inject`-table value.
    plain_targets: &'static [(&'static str, &'static str)],
}

#[cfg(unix)]
fn dedup_rows() -> Vec<DedupRow> {
    vec![
        DedupRow {
            name: "identical effective pairs across two entries (AC-016.5)",
            config: identical_pairs_across_entries,
            entries: &["alpha", "beta"],
            credential_targets: &["OPENAI_API_KEY"],
            plain_targets: &[],
        },
        DedupRow {
            name: "one credential under two target names (AC-016.8)",
            config: two_targets_one_credential,
            entries: &["alpha", "beta"],
            credential_targets: &["OPENAI_API_KEY", "LLM_API_KEY"],
            plain_targets: &[],
        },
        DedupRow {
            name: "identical effective pairs within one entry",
            config: identical_pairs_within_one_entry,
            entries: &["alpha"],
            credential_targets: &["OPENAI_API_KEY"],
            plain_targets: &[],
        },
        DedupRow {
            name: "one entry named twice by --with",
            config: credential_and_inject_in_one_entry,
            entries: &["alpha", "alpha"],
            credential_targets: &["OPENAI_API_KEY"],
            plain_targets: &[("OPENAI_BASE_URL", "https://alpha.example.com/v1")],
        },
    ]
}

#[cfg(unix)]
#[test]
fn ac_016_5_and_ac_016_8_dedup_rows_resolve_each_credential_exactly_once() {
    let probe = probe_binary();

    for row in dedup_rows() {
        let workspace = Workspace::new();
        let script = workspace.stage_script(COUNTING_PROVIDER);
        let counter = workspace.path("provider-invocations");
        let config = workspace.stage_config(&(row.config)(&script, &counter));
        let destination = workspace.path("probe-report");

        let run = run_plan(
            &config,
            &plan_args(row.entries, &probe, &[PROBE_MARKER]),
            &destination,
        );

        assert_exit(&run, 0, row.name);
        assert_eq!(
            invocation_count(&counter),
            1,
            "{}: a credential named several times is resolved once\nstdout: {}\nstderr: {}",
            row.name,
            run.stdout,
            run.stderr
        );

        let report = ProbeReport::read(&destination);
        assert!(
            report.argv.iter().any(|argument| argument == PROBE_MARKER),
            "{}: the target must receive its own arguments, got {} of them",
            row.name,
            report.argv.len()
        );
        for target in row.credential_targets {
            // Compared without printing either side: the expected value is a
            // planted sentinel and a failure message is test output.
            assert!(
                report.value_of(target) == Some(SENTINEL_PLAIN),
                "{}: the probe must see {target} carrying the resolved credential value",
                row.name
            );
        }
        for (target, expected) in row.plain_targets {
            assert_eq!(
                report.value_of(target),
                Some(*expected),
                "{}: the probe must see the injected plain value under {target}",
                row.name
            );
        }
    }
}

// ---------------------------------------------------------------------------
// AC-018.1: every exit code `run` makes reachable in Phase 3
// ---------------------------------------------------------------------------

/// One exit-code row. Each owns its workspace so its config and its temp paths
/// outlive the table.
struct ExitRow {
    name: &'static str,
    _workspace: Workspace,
    config: PathBuf,
    args: Vec<String>,
    report: PathBuf,
    code: i32,
    token: String,
}

#[test]
fn ac_018_1_run_reports_every_phase_three_exit_code_with_a_naming_message() {
    for row in exit_rows() {
        let run = run_plan(&row.config, &row.args, &row.report);

        assert_exit(&run, row.code, row.name);
        assert_stderr_mentions(&run, &row.token, row.name);
    }
}

fn exit_rows() -> Vec<ExitRow> {
    let probe = probe_binary();
    let mut rows = Vec::new();

    // Exit 1: `run` without `--with` is a usage error showing the expected form.
    let workspace = Workspace::new();
    let config = workspace.stage_config(&runner_config());
    let report = workspace.path("probe-report");
    rows.push(ExitRow {
        name: "exit 1: run without --with",
        _workspace: workspace,
        config,
        args: vec![
            "run".to_owned(),
            "--".to_owned(),
            path_str(&probe).to_owned(),
        ],
        report,
        code: 1,
        token: "--with".to_owned(),
    });

    // Exit 1: `--with` without a command after `--`.
    let workspace = Workspace::new();
    let config = workspace.stage_config(&runner_config());
    let report = workspace.path("probe-report");
    rows.push(ExitRow {
        name: "exit 1: run without a command",
        _workspace: workspace,
        config,
        args: vec!["run".to_owned(), "--with".to_owned(), "plain".to_owned()],
        report,
        code: 1,
        token: "run --with".to_owned(),
    });

    // Exit 2: a core validation violation refuses `run` like every command.
    let workspace = Workspace::new();
    let config = workspace.stage_config(&sensitive_field_config());
    let report = workspace.path("probe-report");
    rows.push(ExitRow {
        name: "exit 2: a suspected plaintext secret in the config",
        _workspace: workspace,
        config,
        args: plan_args(&["llm"], &probe, &[]),
        report,
        code: 2,
        token: "api_key".to_owned(),
    });

    // Exit 3: an entry that the active profile does not define.
    let workspace = Workspace::new();
    let config = workspace.stage_config(&runner_config());
    let report = workspace.path("probe-report");
    rows.push(ExitRow {
        name: "exit 3: an unknown entry",
        _workspace: workspace,
        config,
        args: plan_args(&["nosuch"], &probe, &[]),
        report,
        code: 3,
        token: "nosuch".to_owned(),
    });

    // Exit 4: an injection conflict.
    let workspace = Workspace::new();
    let script = workspace.stage_script(CANARY_PROVIDER);
    let canary = workspace.path("canary-ran");
    let config = workspace.stage_config(&credential_vs_credential_across_entries(&script, &canary));
    let report = workspace.path("probe-report");
    rows.push(ExitRow {
        name: "exit 4: an injection conflict",
        _workspace: workspace,
        config,
        args: plan_args(&["alpha", "beta"], &probe, &[]),
        report,
        code: 4,
        token: "canary_cred".to_owned(),
    });

    // Exit 4: a provider that cannot resolve, reached through `run`.
    let workspace = Workspace::new();
    let config = workspace.stage_config(&runner_config());
    let report = workspace.path("probe-report");
    rows.push(ExitRow {
        name: "exit 4: a credential that cannot be resolved",
        _workspace: workspace,
        config,
        args: plan_args(&["missing_env"], &probe, &[]),
        report,
        code: 4,
        token: "MISSING_RUN_TOKEN".to_owned(),
    });

    // Exit 127: a plan that succeeds, then a target that cannot be executed.
    let workspace = Workspace::new();
    let config = workspace.stage_config(&runner_config());
    let report = workspace.path("probe-report");
    let missing = workspace.path("no-such-target");
    rows.push(ExitRow {
        name: "exit 127: a target that cannot be executed",
        _workspace: workspace,
        config,
        args: plan_args(&["plain"], &missing, &[]),
        report,
        code: 127,
        token: path_str(&missing).to_owned(),
    });

    rows
}

// ---------------------------------------------------------------------------
// Config builders
// ---------------------------------------------------------------------------

const PROFILE_HEADER: &str = "version = 1\n\
     default_profile = \"work\"\n\
     \n\
     [profiles.work]\n\
     description = \"Profile exercising the injection plan.\"\n";

fn credential_vs_credential_across_entries(script: &Path, canary: &Path) -> String {
    format!(
        "{PROFILE_HEADER}\n\
         [profiles.work.alpha]\n\
         description = \"Entry naming the canary credential.\"\n\
         credential = \"credential://canary_cred\"\n\
         \n\
         [profiles.work.beta]\n\
         description = \"Entry naming a second credential for the same variable.\"\n\
         credential = \"credential://env_cred\"\n\
         {canary}{env}",
        canary = canary_credential(script, canary, "OPENAI_API_KEY"),
        env = env_credential("OPENAI_API_KEY"),
    )
}

fn credential_vs_credential_within_one_entry(script: &Path, canary: &Path) -> String {
    format!(
        "{PROFILE_HEADER}\n\
         [profiles.work.alpha]\n\
         description = \"Entry naming two credentials for one variable.\"\n\
         primary = \"credential://canary_cred\"\n\
         secondary = \"credential://env_cred\"\n\
         {canary}{env}",
        canary = canary_credential(script, canary, "OPENAI_API_KEY"),
        env = env_credential("OPENAI_API_KEY"),
    )
}

fn credential_vs_inject_across_entries(script: &Path, canary: &Path) -> String {
    format!(
        "{PROFILE_HEADER}\n\
         [profiles.work.alpha]\n\
         description = \"Entry naming the canary credential.\"\n\
         credential = \"credential://canary_cred\"\n\
         \n\
         [profiles.work.beta]\n\
         description = \"Entry injecting a plain value under the same variable.\"\n\
         endpoint = \"https://beta.example.com/v1\"\n\
         \n\
         [profiles.work.beta.inject]\n\
         OPENAI_API_KEY = \"endpoint\"\n\
         {canary}",
        canary = canary_credential(script, canary, "OPENAI_API_KEY"),
    )
}

fn credential_vs_inject_within_one_entry(script: &Path, canary: &Path) -> String {
    format!(
        "{PROFILE_HEADER}\n\
         [profiles.work.alpha]\n\
         description = \"Entry whose credential and inject table target one variable.\"\n\
         credential = \"credential://canary_cred\"\n\
         endpoint = \"https://alpha.example.com/v1\"\n\
         \n\
         [profiles.work.alpha.inject]\n\
         OPENAI_API_KEY = \"endpoint\"\n\
         {canary}",
        canary = canary_credential(script, canary, "OPENAI_API_KEY"),
    )
}

/// The canary credential targets a variable of its own here, so the conflict
/// under test is purely between the two `inject` tables while a provider is
/// still in the plan to prove nothing resolved.
fn inject_vs_inject_across_entries(script: &Path, canary: &Path) -> String {
    format!(
        "{PROFILE_HEADER}\n\
         [profiles.work.alpha]\n\
         description = \"Entry injecting its endpoint.\"\n\
         endpoint = \"https://alpha.example.com/v1\"\n\
         credential = \"credential://canary_cred\"\n\
         \n\
         [profiles.work.alpha.inject]\n\
         OPENAI_BASE_URL = \"endpoint\"\n\
         \n\
         [profiles.work.beta]\n\
         description = \"Entry injecting a different endpoint under the same variable.\"\n\
         endpoint = \"https://beta.example.com/v1\"\n\
         \n\
         [profiles.work.beta.inject]\n\
         OPENAI_BASE_URL = \"endpoint\"\n\
         {canary}",
        canary = canary_credential(script, canary, "CANARY_TOKEN"),
    )
}

#[cfg(unix)]
fn identical_pairs_across_entries(script: &Path, counter: &Path) -> String {
    format!(
        "{PROFILE_HEADER}\n\
         [profiles.work.alpha]\n\
         description = \"Entry using the credential's default target.\"\n\
         credential = \"credential://counting_cred\"\n\
         \n\
         [profiles.work.beta]\n\
         description = \"Entry naming the same target explicitly.\"\n\
         credential = \"credential://counting_cred?as=OPENAI_API_KEY\"\n\
         {counting}",
        counting = counting_credential(script, counter),
    )
}

#[cfg(unix)]
fn two_targets_one_credential(script: &Path, counter: &Path) -> String {
    format!(
        "{PROFILE_HEADER}\n\
         [profiles.work.alpha]\n\
         description = \"Entry using the credential's default target.\"\n\
         credential = \"credential://counting_cred\"\n\
         \n\
         [profiles.work.beta]\n\
         description = \"Entry redirecting the same credential to a second target.\"\n\
         credential = \"credential://counting_cred?as=LLM_API_KEY\"\n\
         {counting}",
        counting = counting_credential(script, counter),
    )
}

#[cfg(unix)]
fn identical_pairs_within_one_entry(script: &Path, counter: &Path) -> String {
    format!(
        "{PROFILE_HEADER}\n\
         [profiles.work.alpha]\n\
         description = \"Entry naming one credential from two fields.\"\n\
         primary = \"credential://counting_cred\"\n\
         secondary = \"credential://counting_cred?as=OPENAI_API_KEY\"\n\
         {counting}",
        counting = counting_credential(script, counter),
    )
}

#[cfg(unix)]
fn credential_and_inject_in_one_entry(script: &Path, counter: &Path) -> String {
    format!(
        "{PROFILE_HEADER}\n\
         [profiles.work.alpha]\n\
         description = \"Entry carrying both a credential and an inject table.\"\n\
         endpoint = \"https://alpha.example.com/v1\"\n\
         credential = \"credential://counting_cred\"\n\
         \n\
         [profiles.work.alpha.inject]\n\
         OPENAI_BASE_URL = \"endpoint\"\n\
         {counting}",
        counting = counting_credential(script, counter),
    )
}

/// The config behind the exit-code rows: one entry that injects without any
/// provider, and one whose environment credential is never set.
fn runner_config() -> String {
    format!(
        "{PROFILE_HEADER}\n\
         [profiles.work.plain]\n\
         description = \"Entry injecting one plain value and no credential.\"\n\
         endpoint = \"https://alpha.example.com/v1\"\n\
         \n\
         [profiles.work.plain.inject]\n\
         OPENAI_BASE_URL = \"endpoint\"\n\
         \n\
         [profiles.work.missing_env]\n\
         description = \"Entry whose credential cannot resolve.\"\n\
         credential = \"credential://missing_env_cred\"\n\
         \n\
         [credentials.missing_env_cred]\n\
         description = \"Environment credential whose variable is never set.\"\n\
         provider = \"env\"\n\
         name = \"MISSING_RUN_TOKEN\"\n\
         inject_as = \"MISSING_RUN_TOKEN\"\n"
    )
}

/// A config carrying a suspected plaintext secret, so `run` must refuse it on
/// the same core-validation path every other command uses.
fn sensitive_field_config() -> String {
    format!(
        "{PROFILE_HEADER}\n\
         [profiles.work.llm]\n\
         description = \"Entry holding a plaintext secret.\"\n\
         api_key = {value}\n",
        value = toml_string(SENTINEL_PLAIN),
    )
}

fn canary_credential(script: &Path, canary: &Path, inject_as: &str) -> String {
    format!(
        "\n[credentials.canary_cred]\n\
         description = \"Command credential whose script leaves a canary file.\"\n\
         provider = \"command\"\n\
         argv = [{script}, {canary}, {value}]\n\
         inject_as = \"{inject_as}\"\n",
        script = toml_string(path_str(script)),
        canary = toml_string(path_str(canary)),
        value = toml_string(SENTINEL_NESTED),
    )
}

#[cfg(unix)]
fn counting_credential(script: &Path, counter: &Path) -> String {
    format!(
        "\n[credentials.counting_cred]\n\
         description = \"Command credential counting its own invocations.\"\n\
         provider = \"command\"\n\
         argv = [{script}, {counter}, {value}]\n\
         inject_as = \"OPENAI_API_KEY\"\n",
        script = toml_string(path_str(script)),
        counter = toml_string(path_str(counter)),
        value = toml_string(SENTINEL_PLAIN),
    )
}

fn env_credential(inject_as: &str) -> String {
    format!(
        "\n[credentials.env_cred]\n\
         description = \"Environment credential targeting the contested variable.\"\n\
         provider = \"env\"\n\
         name = \"ENV_CRED_VALUE\"\n\
         inject_as = \"{inject_as}\"\n"
    )
}

fn toml_string(value: &str) -> String {
    serde_json::to_string(value).expect("test strings serialize")
}

// ---------------------------------------------------------------------------
// Invocation
// ---------------------------------------------------------------------------

fn probe_binary() -> PathBuf {
    assert_cmd::cargo::cargo_bin("test-probe")
}

/// `run --with <entry>... -- <target> [args...]`.
fn plan_args(entries: &[&str], target: &Path, target_args: &[&str]) -> Vec<String> {
    let mut args = vec!["run".to_owned()];
    for entry in entries {
        args.push("--with".to_owned());
        args.push((*entry).to_owned());
    }
    args.push("--".to_owned());
    args.push(path_str(target).to_owned());
    args.extend(target_args.iter().map(|argument| (*argument).to_owned()));
    args
}

/// Invokes agentenv through the shared harness, so the per-invocation
/// sentinel check of AC-019.1 covers this suite too, with `TEST_PROBE_OUT`
/// pointing at where the target should report.
fn run_plan(config: &Path, args: &[String], report: &Path) -> Run {
    let borrowed: Vec<&str> = args.iter().map(String::as_str).collect();

    run_ac(config, &[("TEST_PROBE_OUT", path_str(report))], &borrowed)
}

// ---------------------------------------------------------------------------
// Observations
// ---------------------------------------------------------------------------

/// What the target reported about its own launch. Kept as ordered pairs rather
/// than a map: the suite asserts on named variables only, and config order is
/// meaningful everywhere else in this crate.
#[cfg(unix)]
struct ProbeReport {
    argv: Vec<String>,
    environment: Vec<(String, String)>,
}

#[cfg(unix)]
impl ProbeReport {
    fn read(path: &Path) -> Self {
        let text = fs::read_to_string(path)
            .unwrap_or_else(|error| panic!("the target wrote no report to {path:?}: {error}"));
        let mut argv = Vec::new();
        let mut environment = Vec::new();

        for line in text.lines() {
            if let Some(argument) = line.strip_prefix("argv\t") {
                argv.push(argument.to_owned());
            } else if let Some(variable) = line.strip_prefix("env\t") {
                // Split only; neither half is echoed on failure, since a value
                // here is an injected secret.
                let (name, value) = variable
                    .split_once('=')
                    .unwrap_or_else(|| panic!("a probe env record in {path:?} carries no '='"));
                environment.push((name.to_owned(), value.to_owned()));
            } else {
                panic!("an unrecognized record appears in the probe report {path:?}");
            }
        }

        Self { argv, environment }
    }

    fn value_of(&self, name: &str) -> Option<&str> {
        self.environment
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }
}

/// How many times a counting provider ran. An absent counter means it never
/// ran, which is a legitimate observation rather than a read failure.
#[cfg(unix)]
fn invocation_count(counter: &Path) -> usize {
    match fs::read_to_string(counter) {
        Ok(text) => text.lines().filter(|line| !line.is_empty()).count(),
        Err(error) if error.kind() == io::ErrorKind::NotFound => 0,
        Err(error) => panic!("failed to read the provider counter {counter:?}: {error}"),
    }
}

/// Conflict and failure diagnostics belong on stderr, so these rows assert the
/// channel as well as the text.
fn assert_stderr_mentions(run: &Run, needle: &str, context: &str) {
    assert!(
        run.stderr.contains(needle),
        "{context}: expected stderr to name {needle:?}\nstdout: {}\nstderr: {}",
        run.stdout,
        run.stderr
    );
}

// ---------------------------------------------------------------------------
// Workspace
// ---------------------------------------------------------------------------

/// A temp directory holding one row's config, fixture script, and observation
/// files. Held for as long as the row is invoked against.
struct Workspace {
    directory: TempDir,
}

impl Workspace {
    fn new() -> Self {
        Self {
            directory: TempDir::new().expect("a temp directory is available"),
        }
    }

    /// A path inside the workspace. Nothing is created: the conflict rows read
    /// these paths' absence as evidence.
    fn path(&self, name: &str) -> PathBuf {
        self.directory.path().join(name)
    }

    /// Stages the config at mode 0600, which the Unix permission gate requires
    /// of any config that should load cleanly.
    fn stage_config(&self, contents: &str) -> PathBuf {
        let path = self.path("config.toml");
        fs::write(&path, contents).expect("the injection config is written");
        set_mode(&path, 0o600);
        path
    }

    /// Copies a fixture script into the workspace and makes it executable, so
    /// the suite does not depend on the checked-in mode surviving a checkout.
    fn stage_script(&self, name: &str) -> PathBuf {
        let source = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("fixtures")
            .join(name);
        let path = self.path(name);
        fs::copy(&source, &path)
            .unwrap_or_else(|error| panic!("failed to stage fixture script {name}: {error}"));
        set_mode(&path, 0o755);
        path
    }
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;

    fs::set_permissions(path, fs::Permissions::from_mode(mode))
        .unwrap_or_else(|error| panic!("failed to set the mode of {}: {error}", path.display()));
}

#[cfg(not(unix))]
fn set_mode(_path: &Path, _mode: u32) {}

fn path_str(path: &Path) -> &str {
    path.to_str().expect("test paths are UTF-8")
}
