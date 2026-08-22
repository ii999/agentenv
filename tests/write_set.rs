//! Integration coverage for `agentenv set` (change 002: SPEC-002, plus the
//! SPEC-001 pipeline behaviors observable through it and the SPEC-006
//! sentinel checks, which `helpers::run_ac` applies to every invocation).

mod helpers;

#[cfg(unix)]
use std::fs;

use helpers::{
    assert_exit, assert_mentions, assert_omits, read_config as read, run_ac,
    staged_config as staged, SENTINEL_PARSE, SENTINEL_PLAIN,
};
use tempfile::TempDir;

const BASE_CONFIG: &str = r#"# top comment
version = 1
default_profile = "work"

[profiles.work]
description = "Work environment."

[profiles.work.llm]
description = "LLM entry."
endpoint = "https://old.example.com/v1"  # prod
model = "m1"

[credentials.company_llm]
description = "Company credential."
provider = "env"
name = "COMPANY_TOKEN"
inject_as = "OPENAI_API_KEY"
"#;

#[test]
fn set_replaces_a_scalar_and_names_the_path_only() {
    // AC-002.1 + AC-001.1 + AC-001.6.
    let (_dir, config) = staged(BASE_CONFIG);
    let run = run_ac(
        &config,
        &[],
        &["set", "llm.endpoint", "https://new.example.com/v1"],
    );
    assert_exit(&run, 0, "set replaces a scalar");
    assert_mentions(
        &run,
        "profiles.work.llm.endpoint",
        "the message names the path",
    );
    assert_omits(&run, "new.example.com", "the message omits the value");
    let after = read(&config);
    assert!(
        after.contains("# top comment\n"),
        "comments survive: {after}"
    );
    assert!(
        after.contains("endpoint = \"https://new.example.com/v1\"  # prod\n"),
        "the trailing comment survives: {after}"
    );
    assert!(
        after.contains("model = \"m1\"\n"),
        "untouched lines survive"
    );
}

#[test]
fn set_creates_an_entry_with_description() {
    // AC-002.2.
    let (_dir, config) = staged(BASE_CONFIG);
    let run = run_ac(
        &config,
        &[],
        &[
            "set",
            "kubernetes.context",
            "staging",
            "--description",
            "Kubernetes staging environment.",
        ],
    );
    assert_exit(&run, 0, "entry creation with --description");
    let after = read(&config);
    assert!(
        after.contains("[profiles.work.kubernetes]"),
        "the entry is a standard table: {after}"
    );
    let show = run_ac(&config, &[], &["get", "kubernetes.context"]);
    assert_exit(&show, 0, "the created value reads back");
    assert_mentions(&show, "staging", "the value round-trips");
}

#[test]
fn set_without_description_on_a_new_entry_is_refused() {
    // AC-002.3 + AC-001.2.
    let (_dir, config) = staged(BASE_CONFIG);
    let before = read(&config);
    let run = run_ac(&config, &[], &["set", "newentry.field", "x"]);
    assert_exit(&run, 2, "a new entry without a description is refused");
    assert_mentions(
        &run,
        "newentry.description",
        "the violation names the missing description path",
    );
    assert_eq!(before, read(&config), "the file is byte-identical");
}

#[test]
fn unknown_profile_is_exit_3_from_every_source() {
    // AC-002.4.
    let (_dir, config) = staged(BASE_CONFIG);
    for (envs, args) in [
        (
            &[][..],
            vec!["set", "--profile", "nosuch", "llm.model", "x"],
        ),
        (
            &[("AGENTENV_PROFILE", "nosuch")][..],
            vec!["set", "llm.model", "x"],
        ),
    ] {
        let run = run_ac(&config, envs, &args);
        assert_exit(&run, 3, "unknown profile is refused");
        assert_mentions(&run, "work", "the defined profiles are listed");
    }
    let (_dir2, config2) = staged(
        "version = 1\ndefault_profile = \"ghost\"\n\n[profiles.ghost]\ndescription = \"d\"\n",
    );
    // default_profile must name a defined profile, so an unknown default is
    // already a validation refusal (exit 2) before profile selection.
    let run = run_ac(&config2, &[], &["set", "--profile", "absent", "x.y", "z"]);
    assert_exit(&run, 3, "unknown --profile with a valid default");
}

#[test]
fn create_profile_is_an_explicit_opt_in() {
    // AC-002.5.
    let (_dir, config) = staged(BASE_CONFIG);
    let run = run_ac(
        &config,
        &[],
        &[
            "set",
            "llm.endpoint",
            "https://p.example.com/v1",
            "--profile",
            "personal",
            "--create-profile",
            "Personal projects.",
            "--description",
            "Personal LLM.",
        ],
    );
    assert_exit(&run, 0, "profile bootstrap in one atomic write");
    let list = run_ac(&config, &[], &["list", "--profiles"]);
    assert_mentions(&list, "personal", "the new profile is listed");

    // Without an explicit --profile the flag is a usage error.
    let no_flag = run_ac(
        &config,
        &[("AGENTENV_PROFILE", "work")],
        &["set", "llm.model", "x", "--create-profile", "text"],
    );
    assert_exit(&no_flag, 1, "--create-profile requires --profile");

    // Against an existing profile the flag is a usage error.
    let existing = run_ac(
        &config,
        &[],
        &[
            "set",
            "llm.model",
            "x",
            "--profile",
            "work",
            "--create-profile",
            "text",
        ],
    );
    assert_exit(&existing, 1, "--create-profile refuses an existing profile");
    assert_mentions(&existing, "already exists", "the refusal explains itself");
}

#[test]
fn path_wins_over_create_profile_description() {
    // AC-002.5 precedence: the target path addresses the new profile's
    // description.
    let (_dir, config) = staged(BASE_CONFIG);
    let run = run_ac(
        &config,
        &[],
        &[
            "set",
            "description",
            "From the path.",
            "--profile",
            "fresh",
            "--create-profile",
            "From the flag.",
        ],
    );
    assert_exit(&run, 0, "profile description bootstrap");
    let after = read(&config);
    assert!(
        after.contains("From the path."),
        "the path value wins: {after}"
    );
    assert!(
        !after.contains("From the flag."),
        "the flag text is replaced"
    );
}

#[test]
fn traversal_through_a_scalar_is_exit_3() {
    // AC-002.6.
    let (_dir, config) = staged(BASE_CONFIG);
    let before = read(&config);
    let run = run_ac(&config, &[], &["set", "llm.model.deep", "x"]);
    assert_exit(&run, 3, "a scalar in the path is a conflict");
    assert_mentions(&run, "llm.model", "the conflicting path is named");
    assert_eq!(before, read(&config), "nothing is written");
}

#[test]
fn sensitive_guardrail_refuses_plaintext_and_names_the_remedy() {
    // AC-002.7, with the planted sentinel as the plaintext secret.
    let (_dir, config) = staged(BASE_CONFIG);
    let before = read(&config);
    let run = run_ac(&config, &[], &["set", "llm.api_key", SENTINEL_PLAIN]);
    assert_exit(&run, 2, "a plaintext sensitive field is refused");
    assert_mentions(&run, "credential add", "the remedy names credential add");
    assert_mentions(&run, "credential set", "the remedy names credential set");
    assert_mentions(&run, "api_key", "the field is named");
    assert_eq!(before, read(&config), "nothing is written");

    // A credential reference passes the guardrail.
    let reference = run_ac(
        &config,
        &[],
        &["set", "llm.api_key", "credential://company_llm"],
    );
    assert_exit(&reference, 0, "a credential reference is accepted");
}

#[test]
fn guardrail_exclusions_match_the_validator() {
    // AC-002.8: (a) inject-table target names, (b) non-string values,
    // (c) names without the underscore separator.
    let (_dir, config) = staged(BASE_CONFIG);
    let inject = run_ac(&config, &[], &["set", "llm.inject.GITHUB_TOKEN", "model"]);
    assert_exit(
        &inject,
        0,
        "an inject mapping with a sensitive-looking target",
    );

    let integer = run_ac(&config, &[], &["set", "llm.token", "5", "--type", "int"]);
    assert_exit(&integer, 0, "a non-string value in a sensitive-named field");

    let mytoken = run_ac(&config, &[], &["set", "llm.mytoken", "plain"]);
    assert_exit(&mytoken, 0, "'mytoken' has no underscore separator");
}

#[test]
fn json_values_round_trip_through_get() {
    // AC-002.9 + EDGE-004.
    let (_dir, config) = staged(BASE_CONFIG);
    let array = run_ac(
        &config,
        &[],
        &["set", "llm.tags", r#"["a","b"]"#, "--type", "json"],
    );
    assert_exit(&array, 0, "a JSON array writes");
    let get_array = run_ac(&config, &[], &["get", "llm.tags", "--json"]);
    assert_exit(&get_array, 0, "the array reads back");
    assert_mentions(&get_array, r#"["a","b"]"#, "the JSON value is equivalent");

    let object = run_ac(
        &config,
        &[],
        &[
            "set",
            "llm.limits",
            r#"{"rpm":60,"burst":true}"#,
            "--type",
            "json",
        ],
    );
    assert_exit(&object, 0, "a JSON object writes as an inline table");
    let get_object = run_ac(&config, &[], &["get", "llm.limits", "--json"]);
    assert_mentions(&get_object, "\"rpm\":60", "the object reads back");

    let null = run_ac(&config, &[], &["set", "llm.bad", "null", "--type", "json"]);
    assert_exit(&null, 1, "JSON null is a usage error");
    let nested_null = run_ac(
        &config,
        &[],
        &["set", "llm.bad", r#"{"x":null}"#, "--type", "json"],
    );
    assert_exit(&nested_null, 1, "nested JSON null is a usage error");
}

#[test]
fn nested_tables_need_no_description() {
    // AC-002.10.
    let (_dir, config) = staged(BASE_CONFIG);
    let run = run_ac(
        &config,
        &[],
        &["set", "llm.limits.rpm", "60", "--type", "int"],
    );
    assert_exit(&run, 0, "a nested table under an existing entry");
    let get = run_ac(&config, &[], &["get", "llm.limits.rpm"]);
    assert_mentions(&get, "60", "the nested value reads back");
}

#[test]
fn credential_references_are_cross_checked_in_validator_scope() {
    // AC-002.11 + AC-002.13.
    let (_dir, config) = staged(BASE_CONFIG);
    let undefined = run_ac(
        &config,
        &[],
        &["set", "llm.credential", "credential://absent"],
    );
    assert_exit(&undefined, 2, "an undefined credential name is refused");
    assert_mentions(&undefined, "absent", "the undefined name is diagnosed");
    assert_mentions(&undefined, "credential add", "the remedy is named");

    let malformed = run_ac(
        &config,
        &[],
        &["set", "llm.credential", "credential://company_llm?as=1bad"],
    );
    assert_exit(&malformed, 2, "a malformed ?as= override is refused");

    // Outside the reference scan: a description value and an array element.
    let description = run_ac(
        &config,
        &[],
        &["set", "llm.description", "credential://absent"],
    );
    assert_exit(&description, 0, "description values are not scanned");
    let array = run_ac(
        &config,
        &[],
        &[
            "set",
            "llm.list",
            r#"["credential://absent"]"#,
            "--type",
            "json",
        ],
    );
    assert_exit(&array, 0, "array elements are not scanned");
}

#[test]
fn description_flag_overwrites_and_path_wins() {
    // AC-002.12.
    let (_dir, config) = staged(BASE_CONFIG);
    let overwrite = run_ac(
        &config,
        &[],
        &[
            "set",
            "llm.model",
            "m2",
            "--description",
            "Updated entry text.",
        ],
    );
    assert_exit(&overwrite, 0, "--description overwrites");
    assert!(read(&config).contains("Updated entry text."));

    let collision = run_ac(
        &config,
        &[],
        &[
            "set",
            "llm.description",
            "Path text.",
            "--description",
            "Flag text.",
        ],
    );
    assert_exit(&collision, 0, "the collision resolves");
    let after = read(&config);
    assert!(after.contains("Path text."), "the path value wins: {after}");
    assert!(!after.contains("Flag text."), "the flag text is replaced");
}

#[test]
fn write_commands_reject_the_global_json_flag() {
    // AC-001.7.
    let (_dir, config) = staged(BASE_CONFIG);
    let run = run_ac(&config, &[], &["set", "llm.model", "x", "--json"]);
    assert_exit(&run, 1, "set rejects --json");
    assert_mentions(&run, "--json", "the refusal names the flag");
}

#[test]
fn missing_config_file_names_the_init_remedy() {
    // EDGE-001.
    let dir = TempDir::new().expect("a temp dir");
    let absent = dir.path().join("absent.toml");
    let run = run_ac(&absent, &[], &["set", "llm.model", "x"]);
    assert_exit(&run, 2, "a missing config file is exit 2");
    assert_mentions(&run, "agentenv init", "the remedy names init");
}

#[test]
fn grammar_and_type_errors_are_usage_errors_without_echo() {
    // EDGE-002, EDGE-003, EDGE-009, and the sentinel discipline (the
    // harness re-checks every invocation).
    let (_dir, config) = staged(BASE_CONFIG);
    let empty_path = run_ac(&config, &[], &["set", "", "x"]);
    assert_exit(&empty_path, 1, "an empty path is a grammar error");

    let empty_description = run_ac(&config, &[], &["set", "llm.description", ""]);
    assert_exit(
        &empty_description,
        2,
        "an empty description fails validation",
    );

    let bad_int = run_ac(
        &config,
        &[],
        &["set", "llm.count", SENTINEL_PARSE, "--type", "int"],
    );
    assert_exit(&bad_int, 1, "a non-integer --type int value");

    let bad_json = run_ac(
        &config,
        &[],
        &["set", "llm.count", SENTINEL_PARSE, "--type", "json"],
    );
    assert_exit(&bad_json, 1, "malformed JSON");

    // AC-006.1: the sentinel under every remaining --type; the harness
    // checks each invocation for leaks.
    let bad_float = run_ac(
        &config,
        &[],
        &["set", "llm.rate", SENTINEL_PARSE, "--type", "float"],
    );
    assert_exit(&bad_float, 1, "a non-float --type float value");
    let overflow_float = run_ac(
        &config,
        &[],
        &["set", "llm.rate", "1e400", "--type", "float"],
    );
    assert_exit(&overflow_float, 1, "a finite literal overflowing to inf");
    let explicit_inf = run_ac(&config, &[], &["set", "llm.rate", "inf", "--type", "float"]);
    assert_exit(&explicit_inf, 0, "the explicit inf keyword still writes");
    let bad_bool = run_ac(
        &config,
        &[],
        &["set", "llm.flag", SENTINEL_PARSE, "--type", "bool"],
    );
    assert_exit(&bad_bool, 1, "a non-bool --type bool value");
    let string_ok = run_ac(&config, &[], &["set", "llm.note", SENTINEL_PARSE]);
    assert_exit(&string_ok, 0, "a sentinel string value writes silently");

    // A sentinel planted in the file must not surface in later diagnostics.
    let sentinel_config = format!(
        "version = 1\ndefault_profile = \"work\"\n\n[profiles.work]\ndescription = \"d\"\n\n\
         [profiles.work.llm]\ndescription = \"d\"\nnote = \"{SENTINEL_PLAIN}\"\n"
    );
    let (_dir2, config2) = staged(&sentinel_config);
    let refusal = run_ac(&config2, &[], &["set", "other.field", "x"]);
    assert_exit(&refusal, 2, "the refusal diagnostics stay value-free");
}

#[test]
fn set_into_reserved_inject_with_non_string_is_refused() {
    // EDGE-008.
    let (_dir, config) = staged(BASE_CONFIG);
    let before = read(&config);
    let run = run_ac(
        &config,
        &[],
        &["set", "llm.inject.OPENAI_MODEL", "5", "--type", "int"],
    );
    assert_exit(&run, 2, "a non-string inject value fails validation");
    assert_eq!(before, read(&config), "nothing is written");
}

#[cfg(unix)]
#[test]
fn unwritable_config_directory_is_exit_2() {
    // EDGE-011.
    use std::os::unix::fs::PermissionsExt;
    let (dir, config) = staged(BASE_CONFIG);
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o500))
        .expect("the directory is read-only");
    if fs::write(dir.path().join("probe"), b"x").is_ok() {
        // Running as root: the permission bits do not restrict us, so the
        // scenario cannot be staged. Skip rather than assert a non-failure.
        fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700))
            .expect("the directory is restored");
        eprintln!("skipping: directory permissions do not bind this user");
        return;
    }
    let run = run_ac(&config, &[], &["set", "llm.model", "x"]);
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700))
        .expect("the directory is restored");
    assert_exit(&run, 2, "an unwritable directory is exit 2");
    assert_mentions(&run, "config.toml", "the diagnostic names the config path");
}

#[test]
fn set_refuses_to_replace_a_table_leaf() {
    // A leaf that names a structural table is a conflict, not a silent
    // wholesale replacement.
    let table_config = format!("{BASE_CONFIG}\n[profiles.work.llm.limits]\nrpm = 60\n");
    let (_dir, config) = staged(&table_config);
    let before = read(&config);

    let nested = run_ac(&config, &[], &["set", "llm.limits", "somestring"]);
    assert_exit(&nested, 3, "a nested table leaf is refused");
    assert_mentions(&nested, "agentenv unset", "the refusal points at unset");
    assert_eq!(before, read(&config), "nothing is written");

    let entry = run_ac(&config, &[], &["set", "llm", "somestring"]);
    assert_exit(&entry, 3, "an entry table leaf is refused");
    assert_eq!(before, read(&config), "nothing is written");

    // The same protection covers a structural table held inline.
    let inline_config = format!(
        "{BASE_CONFIG}\n[profiles.work.db]\ndescription = \"DB.\"\nlimits = {{ rpm = 60 }}\n"
    );
    let (_dir2, config2) = staged(&inline_config);
    let before2 = read(&config2);
    let inline = run_ac(&config2, &[], &["set", "db.limits", "somestring"]);
    assert_exit(&inline, 3, "an inline table leaf is refused");
    assert_eq!(before2, read(&config2), "nothing is written");
}

#[test]
fn inline_tables_are_addressable_like_standard_tables() {
    // The read path resolves inline tables, so the write path must too.
    let inline_config = "version = 1\ndefault_profile = \"work\"\n\n[profiles]\nwork = { \
                         description = \"Work.\", llm = { description = \"LLM.\", model = \
                         \"m1\" } }\n";
    let (_dir, config) = staged(inline_config);

    let set = run_ac(&config, &[], &["set", "llm.model", "m2"]);
    assert_exit(&set, 0, "set addresses an inline table");
    let get = run_ac(&config, &[], &["get", "llm.model"]);
    assert_mentions(&get, "m2", "the value round-trips");

    let unset = run_ac(&config, &[], &["unset", "llm.model"]);
    assert_exit(&unset, 0, "unset addresses an inline table");
    let gone = run_ac(&config, &[], &["get", "llm.model"]);
    assert_exit(&gone, 3, "the field is gone");
}

#[test]
fn json_integers_beyond_i64_are_refused() {
    // as_f64 would silently round these; the pipeline refuses instead, in
    // every band: (i64::MAX, u64::MAX], above u64::MAX, and below i64::MIN.
    let (_dir, config) = staged(BASE_CONFIG);
    let before = read(&config);
    for literal in [
        "18446744073709551615",
        "99999999999999999999",
        "[-99999999999999999999]",
    ] {
        let run = run_ac(&config, &[], &["set", "llm.n", literal, "--type", "json"]);
        assert_exit(&run, 1, "an out-of-range JSON integer is refused");
    }
    assert_eq!(before, read(&config), "nothing is written");
}

#[test]
fn description_with_a_single_segment_path_is_a_usage_error() {
    let (_dir, config) = staged(BASE_CONFIG);
    let run = run_ac(
        &config,
        &[],
        &["set", "somekey", "x", "--description", "text"],
    );
    assert_exit(&run, 1, "--description needs an <entry>.<field> path");
    assert_mentions(&run, "--description", "the refusal names the flag");
}
