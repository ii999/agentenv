use std::path::Path;

use agentenv::project::model::{parse, ProjectFileMeta, MAX_PROJECT_FILE_BYTES};

const FIXTURE_DIRECTORY: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/project/");

fn parse_fixture(name: &str) -> Result<ProjectFileMeta, Vec<agentenv::error::Violation>> {
    let fixture = format!("{FIXTURE_DIRECTORY}{name}");
    parse(
        std::fs::read(&fixture)
            .unwrap_or_else(|error| panic!("fixture {name} should be readable: {error}"))
            .as_slice(),
        Path::new(&fixture),
    )
}

fn assert_invalid(name: &str, path: &str, omitted: Option<&str>) {
    let violations = parse_fixture(name).expect_err("fixture should be invalid");
    assert!(
        violations.iter().any(|violation| violation.path == path),
        "{name} should name {path}; got {violations:?}"
    );
    if let Some(omitted) = omitted {
        assert!(
            violations
                .iter()
                .all(|violation| !violation.message.contains(omitted)),
            "{name} must not echo its sentinel"
        );
    }
}

#[test]
fn accepts_a_well_formed_project_file_in_declaration_order() {
    let meta = parse_fixture("valid.toml").expect("valid fixture should parse");
    assert_eq!(meta.pin.as_ref().map(|pin| pin.name.as_str()), Some("work"));
    assert_eq!(meta.requires.len(), 2);
    assert_eq!(meta.requires[0].entry, "llm");
    assert_eq!(meta.requires[0].fields, ["endpoint", "auth.token"]);
    assert_eq!(meta.requires[1].entry, "database");
    assert!(meta.requires[1].fields.is_empty());
}

#[test]
fn rejects_every_closed_schema_violation_class() {
    assert_invalid(
        "unknown_top_level.toml",
        "inject",
        Some("project-secret-sentinel"),
    );
    assert_invalid("missing_version.toml", "version", None);
    assert_invalid("wrong_version.toml", "version", None);
    assert_invalid("bad_profile.toml", "profile", None);
    assert_invalid("profile_not_string.toml", "profile", None);
    assert_invalid("bad_requirement.toml", "requires.llm.reason", None);
    assert_invalid("empty_reason.toml", "requires.llm.reason", None);
    assert_invalid("empty_fields.toml", "requires.llm.fields", None);
    assert_invalid("bad_fields.toml", "requires.llm.fields[1]", None);
    assert_invalid("duplicate_fields.toml", "requires.llm.fields[1]", None);
    assert_invalid("bad_entry_key.toml", "requires.\"\"", None);
    assert_invalid(
        "credential_reference.toml",
        "profile",
        Some("credential://do-not-echo"),
    );
    assert_invalid(
        "credential_reason.toml",
        "requires.llm.reason",
        Some("credential://do-not-echo"),
    );
    assert_invalid(
        "credential_field.toml",
        "requires.llm.fields[0]",
        Some("credential://do-not-echo"),
    );
}

#[test]
fn reports_toml_parse_position_without_source_text() {
    let fixture = format!("{FIXTURE_DIRECTORY}parse_error.toml");
    let violations = parse_fixture("parse_error.toml").expect_err("fixture should be invalid");
    assert_eq!(violations.len(), 1);
    assert_eq!(violations[0].path, fixture);
    assert!(violations[0].message.contains("line"));
    assert!(!violations[0].message.contains("project-parse-sentinel"));
}

#[test]
fn rejects_files_larger_than_64_kib_before_parsing() {
    let file = Path::new("oversized.agentenv.toml");
    let bytes = vec![b'x'; MAX_PROJECT_FILE_BYTES + 1];
    let violations = parse(&bytes, file).expect_err("oversized input should be invalid");
    assert_eq!(violations.len(), 1);
    assert_eq!(violations[0].path, file.display().to_string());
    assert!(violations[0].message.contains("64 KiB"));
}
