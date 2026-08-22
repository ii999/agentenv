//! Integration coverage for `agentenv unset` (change 002: SPEC-003).

mod helpers;

use std::path::PathBuf;
use std::sync::LazyLock;

use helpers::{
    assert_exit, assert_mentions, read_config as read, run_ac, staged_config, SENTINEL_PLAIN,
};
use tempfile::TempDir;

/// The base fixture plants a sentinel as an open-schema value, so the
/// harness's automatic leak check bites on every refusal diagnostic
/// (AC-006.2).
static BASE_CONFIG: LazyLock<String> = LazyLock::new(|| {
    format!(
        r#"version = 1
default_profile = "work"

[profiles.work]
description = "Work environment."

[profiles.work.llm]
description = "LLM entry."
endpoint = "https://llm.example.com/v1"
model = "m1"
note = "{SENTINEL_PLAIN}"

[profiles.work.llm.inject]
OPENAI_MODEL = "model"

[profiles.work.ci]
description = "CI labels."
tags = ["linux"]
"#
    )
});

fn staged(content: &str) -> (TempDir, PathBuf) {
    staged_config(content)
}

#[test]
fn unset_removes_a_field_and_names_the_path() {
    // AC-003.1.
    let (_dir, config) = staged(&BASE_CONFIG);
    let run = run_ac(&config, &[], &["unset", "llm.endpoint"]);
    assert_exit(&run, 0, "a field is removed");
    assert_mentions(
        &run,
        "profiles.work.llm.endpoint",
        "the message names the path",
    );
    let after = read(&config);
    assert!(
        !after.contains("endpoint = \"https://llm.example.com/v1\""),
        "the field is gone: {after}"
    );
    assert!(after.contains("model = \"m1\""), "sibling fields survive");
    let validate = run_ac(&config, &[], &["validate"]);
    assert_exit(&validate, 0, "the result still validates");
}

#[test]
fn unset_removes_a_whole_entry_including_inject() {
    // AC-003.2.
    let (_dir, config) = staged(&BASE_CONFIG);
    let run = run_ac(&config, &[], &["unset", "llm"]);
    assert_exit(&run, 0, "an entry table is removed");
    let after = read(&config);
    assert!(!after.contains("[profiles.work.llm]"), "{after}");
    assert!(!after.contains("[profiles.work.llm.inject]"), "{after}");
    assert!(
        after.contains("[profiles.work.ci]"),
        "other entries survive"
    );
    let validate = run_ac(&config, &[], &["validate"]);
    assert_exit(&validate, 0, "the result still validates");
}

#[test]
fn unset_missing_path_and_unknown_profile_are_exit_3() {
    // AC-003.3 (inherits SPEC-002 failure modes).
    let (_dir, config) = staged(&BASE_CONFIG);
    let before = read(&config);

    let missing = run_ac(&config, &[], &["unset", "llm.absent"]);
    assert_exit(&missing, 3, "a missing field is exit 3");
    assert_eq!(before, read(&config), "the file is unchanged");

    let traversal = run_ac(&config, &[], &["unset", "llm.model.deep"]);
    assert_exit(&traversal, 3, "traversal through a scalar is exit 3");

    let profile_flag = run_ac(&config, &[], &["unset", "--profile", "nosuch", "llm.model"]);
    assert_exit(&profile_flag, 3, "an unknown --profile is exit 3");

    let profile_env = run_ac(
        &config,
        &[("AGENTENV_PROFILE", "nosuch")],
        &["unset", "llm.model"],
    );
    assert_exit(&profile_env, 3, "an unknown env profile is exit 3");
    assert_eq!(before, read(&config), "nothing was written");
}

#[test]
fn unset_refuses_a_removal_that_breaks_validation() {
    // AC-003.4 + EDGE-007.
    let (_dir, config) = staged(&BASE_CONFIG);
    let before = read(&config);

    let entry_description = run_ac(&config, &[], &["unset", "llm.description"]);
    assert_exit(&entry_description, 2, "removing an entry description");
    assert_mentions(
        &entry_description,
        "llm.description",
        "the violation names the description path",
    );
    assert_eq!(before, read(&config), "the file is unchanged");

    let profile_description = run_ac(&config, &[], &["unset", "description"]);
    assert_exit(&profile_description, 2, "removing the profile description");
    assert_eq!(before, read(&config), "the file is unchanged");

    // Removing a field an inject mapping points at leaves the mapping
    // dangling; the pre-write validation refuses the removal.
    let inject_source = run_ac(&config, &[], &["unset", "llm.model"]);
    assert_exit(&inject_source, 2, "removing an inject mapping's source");
    assert_mentions(
        &inject_source,
        "OPENAI_MODEL",
        "the violation names the dangling mapping",
    );
    assert_eq!(before, read(&config), "the file is unchanged");
}

#[test]
fn unset_rejects_the_global_json_flag() {
    // AC-001.7 applies to every write command.
    let (_dir, config) = staged(&BASE_CONFIG);
    let run = run_ac(&config, &[], &["unset", "llm.model", "--json"]);
    assert_exit(&run, 1, "unset rejects --json");
}
