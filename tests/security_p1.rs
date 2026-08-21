//! Phase-1 security suite: the guardrails that keep a suspected plaintext
//! secret out of every command's output, and out of the config in the first
//! place. Covers SPEC-002 (AC-002.5), SPEC-019 (AC-019.1, AC-019.3) and
//! SPEC-020 (AC-020.1 to AC-020.6).
//!
//! AC-019.1 needs no test of its own: `run_ac` checks every invocation this
//! file makes for planted secrets before returning it.

mod helpers;

use std::fs;
use std::path::Path;

use helpers::{assert_exit, assert_mentions, assert_omits, run_ac, Fixture};

#[test]
fn ac_002_5_get_refuses_a_sensitive_field_instead_of_printing_it() {
    let config = Fixture::new("sensitive_plain.toml");
    let run = run_ac(config.path(), &[], &["get", "llm.api_key"]);

    assert_exit(
        &run,
        2,
        "get on a field holding a suspected plaintext secret",
    );
    assert_mentions(
        &run,
        &config.path().display().to_string(),
        "the refusal must name the config file to edit",
    );
    assert_mentions(
        &run,
        "credential://",
        "the refusal must point at the credential reference remedy",
    );
}

#[test]
fn ac_020_1_validate_rejects_a_plaintext_secret_naming_its_path() {
    let config = Fixture::new("sensitive_plain.toml");
    let run = run_ac(config.path(), &[], &["validate"]);

    assert_exit(&run, 2, "validate on a plaintext secret");
    assert_mentions(
        &run,
        "profiles.work.llm.api_key",
        "the violation must name the field that carries the secret",
    );
}

#[test]
fn ac_020_2_and_ac_020_3_validate_accepts_references_and_non_secret_names() {
    let config = Fixture::new("sensitive_ok.toml");
    let run = run_ac(config.path(), &[], &["validate"]);

    assert_exit(
        &run,
        0,
        "validate on a config whose sensitive-looking fields are a credential reference, \
         a non-secret name, and a boolean",
    );
}

#[test]
fn ac_020_4_a_nested_sensitive_field_is_rejected_by_every_command() {
    let config = Fixture::new("sensitive_nested.toml");

    for args in [["validate"].as_slice(), ["list"].as_slice()] {
        let run = run_ac(config.path(), &[], args);

        assert_exit(&run, 2, "a secret nested below entry level");
        assert_mentions(
            &run,
            "profiles.work.llm.extra.api_key",
            "the violation must name the full path of the nested field",
        );
    }
}

#[test]
fn ac_020_5_a_sensitive_field_inside_an_array_is_rejected() {
    let config = Fixture::new("sensitive_array.toml");
    let run = run_ac(config.path(), &[], &["validate"]);

    assert_exit(&run, 2, "a secret in a table nested inside an array");
    assert_mentions(
        &run,
        "records[0].api_key",
        "the violation must name the array element that carries the secret",
    );
}

#[test]
fn ac_020_6_sensitive_names_match_case_insensitively() {
    let config = Fixture::new("sensitive_upper.toml");
    let run = run_ac(config.path(), &[], &["validate"]);

    assert_exit(&run, 2, "an uppercase TOKEN field holding a string");
}

#[test]
fn ac_019_3_a_parse_error_reports_its_position_without_echoing_the_line() {
    let config = Fixture::new("parse_error_sentinel.toml");
    let run = run_ac(config.path(), &[], &["list"]);

    assert_exit(&run, 2, "list on a config with a malformed line");
    assert_omits(
        &run,
        "api_key =",
        "a parse diagnostic must not reproduce the offending source line",
    );

    let position = line_number_of(config.path(), "api_key");
    assert_mentions(
        &run,
        &position.to_string(),
        "a parse diagnostic must carry the line position of the error",
    );
    assert!(
        run.combined().to_ascii_lowercase().contains("line"),
        "a parse diagnostic must state that the position is a line\nstdout: {}\nstderr: {}",
        run.stdout,
        run.stderr
    );
}

#[test]
fn the_design_example_config_loads_cleanly() {
    let config = Fixture::new("example.toml");
    let run = run_ac(config.path(), &[], &["validate"]);

    assert_exit(&run, 0, "validate on the documented example config");
}

/// One-based number of the first line containing `needle`.
fn line_number_of(config: &Path, needle: &str) -> usize {
    let text = fs::read_to_string(config).expect("the staged fixture is readable");
    let index = text
        .lines()
        .position(|line| line.contains(needle))
        .unwrap_or_else(|| panic!("no line of {} contains {needle:?}", config.display()));

    index + 1
}
