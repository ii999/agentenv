//! Integration coverage for the Phase-1 read-only query surface.

mod helpers;

use std::fs;
use std::path::Path;

use helpers::{assert_exit, assert_mentions, run_ac, Fixture};

#[test]
fn list_show_get_find_and_credential_list_follow_the_read_only_contract() {
    let config = Fixture::new("example.toml");

    let list = run_ac(config.path(), &[], &["list"]);
    assert_exit(&list, 0, "list example config");
    assert_mentions(&list, "Profile: work", "list selects the default profile");
    assert_mentions(
        &list,
        "llm.inject: table",
        "top-level list does not recurse into inject",
    );

    let entry = run_ac(config.path(), &[], &["list", "llm"]);
    assert_exit(&entry, 0, "list one entry");
    assert_mentions(
        &entry,
        "llm.inject.OPENAI_BASE_URL",
        "entry list recursively names paths",
    );

    let show = run_ac(config.path(), &[], &["show", "llm"]);
    assert_exit(&show, 0, "show one entry");
    assert_mentions(
        &show,
        "OPENAI_BASE_URL ← endpoint",
        "show renders inject mappings once",
    );
    assert_mentions(
        &show,
        "company_llm (not set)",
        "show has shallow status only",
    );

    let get = run_ac(config.path(), &[], &["get", "llm.endpoint"]);
    assert_exit(&get, 0, "get a scalar");
    assert_eq!(get.stdout, "https://llm.example.com/v1\n");

    let complex_get = run_ac(config.path(), &[], &["get", "ci.tags"]);
    assert_exit(&complex_get, 1, "get an array without JSON");
    assert_mentions(&complex_get, "--json", "complex get names the JSON remedy");

    let find = run_ac(config.path(), &[], &["find", "llm"]);
    assert_exit(&find, 0, "find matches case-insensitively");
    assert_mentions(&find, "llm.endpoint", "find matches string values");
    assert_mentions(
        &find,
        "llm.credential",
        "find matches credential references",
    );

    let credentials = run_ac(config.path(), &[], &["credential", "list"]);
    assert_exit(&credentials, 0, "credential list");
    assert_mentions(
        &credentials,
        "provider: env",
        "credential list shows provider metadata",
    );
}

#[test]
fn json_shapes_are_frozen_by_snapshots() {
    let config = Fixture::new("example.toml");
    for (arguments, snapshot) in [
        (&["list", "--json"][..], "list.json"),
        (&["list", "--profiles", "--json"][..], "profiles.json"),
        (&["show", "llm", "--json"][..], "entry.json"),
        (&["get", "ci.tags", "--json"][..], "raw-get.json"),
        (&["find", "llm", "--json"][..], "find.json"),
        (&["credential", "list", "--json"][..], "credentials.json"),
    ] {
        let run = run_ac(config.path(), &[], arguments);
        assert_exit(&run, 0, snapshot);
        assert_json_snapshot(&run.stdout, snapshot);
    }

    let list = run_ac(config.path(), &[], &["list", "llm", "--json"]);
    let show = run_ac(config.path(), &[], &["show", "llm", "--json"]);
    assert_eq!(
        list.stdout, show.stdout,
        "list <entry> and show are JSON aliases"
    );
}

#[test]
fn profile_recovery_and_usage_errors_keep_their_exit_codes() {
    let config = Fixture::new("example.toml");
    let unknown = run_ac(config.path(), &[], &["list", "nosuch"]);
    assert_exit(&unknown, 3, "unknown entry");
    assert_mentions(&unknown, "llm", "unknown entry lists available names");

    let multi_segment = run_ac(config.path(), &[], &["show", "llm.endpoint"]);
    assert_exit(&multi_segment, 1, "a show argument must be one segment");

    let empty_find = run_ac(config.path(), &[], &["find", ""]);
    assert_exit(&empty_find, 1, "empty find needle");
    assert_mentions(
        &empty_find,
        "agentenv list",
        "empty find offers a next action",
    );

    let no_matches = run_ac(config.path(), &[], &["find", "zzz-no-match"]);
    assert_exit(&no_matches, 0, "find has a successful empty result");
    assert_eq!(no_matches.stdout, "");
    assert_mentions(&no_matches, "No matches", "empty text find is explicit");
}

fn assert_json_snapshot(output: &str, name: &str) {
    let expected = fs::read_to_string(snapshot_path(name))
        .unwrap_or_else(|error| panic!("failed to read snapshot {name}: {error}"));
    assert!(
        output.ends_with('\n'),
        "JSON output {name} must end with one newline"
    );
    assert!(
        expected.ends_with('\n'),
        "snapshot {name} must end with one newline"
    );
    assert_eq!(output, expected, "JSON snapshot {name} changed");
}

fn snapshot_path(name: &str) -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("snapshots")
        .join(name)
}

#[test]
fn open_schema_types_and_quoted_paths_are_rendered_and_addressable() {
    let config = Fixture::new("query_types.toml");

    for (command, expected) in [
        (
            &["list", "types"][..],
            "types.nested.deeper.credential: credential reference",
        ),
        (
            &["show", "types"][..],
            "types.nested.deeper.credential: credential reference",
        ),
    ] {
        let run = run_ac(config.path(), &[], command);
        assert_exit(&run, 0, "all TOML types render successfully");
        for label in ["string", "integer", "float", "boolean", "datetime", "array"] {
            assert_mentions(&run, label, "type label is present");
        }
        assert_mentions(
            &run,
            expected,
            "nested credential is classified as a reference",
        );
    }

    let quoted = run_ac(config.path(), &[], &["get", "server.\"my.field\""]);
    assert_exit(&quoted, 0, "quoted field path resolves");
    assert_eq!(quoted.stdout, "addressable through a quoted path\n");

    let show_json = run_ac(config.path(), &[], &["show", "server", "--json"]);
    assert_exit(&show_json, 0, "quoted field appears in JSON");
    assert_mentions(
        &show_json,
        "server.\\\"my.field\\\"",
        "quoted field retains its get-acceptable rendered path",
    );
    assert!(!show_json.stdout.contains("\"addressable\""));
}

#[test]
fn scalar_get_uses_exact_toml_lexical_text() {
    let config = Fixture::new("query_types.toml");
    for (path, expected) in [
        ("types.enabled", "true\n"),
        ("types.integer", "42\n"),
        ("types.finite", "1.5\n"),
        ("types.infinite", "inf\n"),
        ("types.offset", "1979-05-27T07:32:00-08:00\n"),
        ("types.local_date", "1979-05-27\n"),
    ] {
        let run = run_ac(config.path(), &[], &["get", path]);
        assert_exit(&run, 0, path);
        assert_eq!(run.stdout, expected, "{path} must be byte-exact");
    }
}

#[test]
fn all_profiles_find_does_not_require_a_default_profile() {
    let config = Fixture::new("no_default_profiles.toml");
    let run = run_ac(config.path(), &[], &["find", "LLM", "--all-profiles"]);
    assert_exit(&run, 0, "all-profiles find without a default profile");
    assert_mentions(&run, "work: llm", "work profile match is labeled");
    assert_mentions(&run, "personal: llm", "personal profile match is labeled");
}

#[test]
fn find_hides_complex_field_values_and_keeps_credential_needles_visible() {
    let config = Fixture::new("query_types.toml");

    let table = run_ac(config.path(), &[], &["find", "inject"]);
    assert_exit(&table, 0, "table name match");
    assert_mentions(
        &table,
        "nested_inject.settings.inject: table",
        "table label is shown",
    );
    assert_eq!(
        table
            .stdout
            .lines()
            .find(|line| line.contains("nested_inject.settings.inject")),
        Some("work: nested_inject.settings.inject: table"),
        "a table name-match carries no value"
    );

    let reference = run_ac(config.path(), &[], &["find", "custom_token"]);
    assert_exit(&reference, 0, "credential reference value match");
    assert_mentions(
        &reference,
        "credential://company_llm?as=CUSTOM_TOKEN",
        "full reference text remains visible",
    );
    assert_mentions(&reference, "not set", "reference carries shallow status");

    let show = run_ac(config.path(), &[], &["show", "nested_inject"]);
    assert_exit(&show, 0, "nested inject show");
    assert_mentions(
        &show,
        "nested_inject.settings.inject.source: string — ordinary",
        "nested inject is ordinary data",
    );
    assert!(!show.stdout.contains('←'));
}

#[test]
fn list_and_json_preserve_stored_order_byte_for_byte() {
    let config = Fixture::new("query_types.toml");
    let text = run_ac(config.path(), &[], &["list"]);
    assert_exit(&text, 0, "ordered text list");
    let zebra = text.stdout.find("zebra —").expect("zebra listed");
    let alpha = text.stdout.find("alpha —").expect("alpha listed");
    assert!(zebra < alpha, "entries retain stored order");

    let profiles = run_ac(config.path(), &[], &["list", "--profiles"]);
    assert_exit(&profiles, 0, "ordered profile list");
    let work = profiles.stdout.find("work").expect("work listed");
    let empty = profiles.stdout.find("empty").expect("empty listed");
    assert!(work < empty, "profiles retain stored order");

    let list_json = run_ac(config.path(), &[], &["list", "--json"]);
    assert_exit(&list_json, 0, "ordered JSON list");
    let zebra_json = list_json.stdout.find("\"zebra\"").expect("zebra listed");
    let alpha_json = list_json.stdout.find("\"alpha\"").expect("alpha listed");
    assert!(zebra_json < alpha_json, "JSON entries retain stored order");

    let json = run_ac(config.path(), &[], &["get", "zebra", "--json"]);
    assert_exit(&json, 0, "raw JSON order");
    let description = json.stdout.find("description").expect("description exists");
    let third = json.stdout.find("third").expect("third exists");
    let first = json.stdout.find("first").expect("first exists");
    assert!(
        description < third && third < first,
        "raw JSON preserves TOML member order"
    );
}

#[test]
fn empty_profile_and_empty_entry_listings_are_explicit() {
    let config = Fixture::new("query_types.toml");
    let run = run_ac(config.path(), &[], &["--profile", "empty", "list"]);
    assert_exit(&run, 0, "empty profile list");
    assert_mentions(&run, "No entries are defined.", "empty profile is explicit");

    let unknown = run_ac(
        config.path(),
        &[],
        &["--profile", "empty", "list", "missing"],
    );
    assert_exit(&unknown, 3, "unknown entry in empty profile");
    assert_mentions(&unknown, "(none defined)", "empty entry set is explicit");
}

#[cfg(unix)]
#[test]
fn validate_checks_permissions_even_when_structure_is_invalid() {
    use std::os::unix::fs::PermissionsExt;

    let invalid = Fixture::new("sensitive_plain.toml");
    fs::set_permissions(invalid.path(), fs::Permissions::from_mode(0o644))
        .expect("set invalid fixture mode");
    let aggregate = run_ac(invalid.path(), &[], &["validate"]);
    assert_exit(
        &aggregate,
        2,
        "validation aggregates structure and permissions",
    );
    assert_mentions(&aggregate, "api_key", "structural violation is present");
    assert_mentions(&aggregate, "0600", "permission violation is present");

    for (mode, expected) in [(0o644, 2), (0o700, 2), (0o600, 0), (0o400, 0)] {
        let config = Fixture::new("example.toml");
        fs::set_permissions(config.path(), fs::Permissions::from_mode(mode))
            .unwrap_or_else(|error| panic!("set mode {mode:04o}: {error}"));
        let run = run_ac(config.path(), &[], &["validate"]);
        assert_exit(&run, expected, &format!("validate mode {mode:04o}"));
        if expected != 0 {
            assert_mentions(&run, "0600", "permission remedy is named");
        }
    }
}

#[test]
fn phase_one_failure_codes_remain_distinct() {
    let valid = Fixture::new("example.toml");
    let invalid = Fixture::new("sensitive_plain.toml");
    for (config, arguments, code, token) in [
        (&valid, &["find", ""][..], 1, "non-empty"),
        (&invalid, &["validate"][..], 2, "api_key"),
        (&valid, &["list", "missing"][..], 3, "available entries"),
    ] {
        let run = run_ac(config.path(), &[], arguments);
        assert_exit(&run, code, "phase-one failure code");
        assert_mentions(&run, token, "failure has a useful message token");
    }
}

#[test]
fn a_nested_description_holding_a_reference_shaped_string_is_ordinary_data() {
    // Regression (T004 re-review N1): `description` keys are never scanned as
    // references at any depth, so a reference-shaped string in one must not
    // panic query commands or resolve against the credentials table.
    let config = Fixture::new("nested_description.toml");

    let validate = run_ac(config.path(), &[], &["validate"]);
    assert_exit(&validate, 0, "nested description is valid config");

    for command in [vec!["list"], vec!["show", "e"], vec!["find", "sub"]] {
        let run = run_ac(config.path(), &[], &command);
        assert_exit(&run, 0, "query commands treat nested description as data");
    }

    let find = run_ac(config.path(), &[], &["find", "not_defined_anywhere"]);
    assert_exit(&find, 0, "find matches the raw string value");
    assert_mentions(
        &find,
        "e.sub.description",
        "the nested description matches as an ordinary string field",
    );
}

#[test]
fn find_never_matches_reserved_inject_members() {
    // Regression (T004 re-review N2): SPEC-009 excludes the entry-level
    // inject table's members from the match domain entirely.
    let config = Fixture::new("example.toml");

    let find = run_ac(config.path(), &[], &["find", "OPENAI"]);
    assert_exit(&find, 0, "find with an inject-member needle");
    assert_eq!(
        find.stdout, "",
        "no inject member may surface as a match: {}",
        find.stdout
    );
    assert_mentions(&find, "No matches", "zero-match message on stderr");
}
