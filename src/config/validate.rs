//! SPEC-002 core validation, the SPEC-013 `inject` rules, and the SPEC-020
//! sensitive-name traversal.
//!
//! One pass over the parsed root table collects **every** violation (no
//! short-circuiting); [`Config::load`](super::Config::load) turns a
//! non-empty result into a single exit-2 error. Diagnostics carry config
//! paths and never echo open-schema field values: the sensitive-name rule
//! reports the field path and the `credential://` remedy, never the value,
//! and closed credential-schema metadata is the only config content allowed
//! in messages (SPEC-019).

use toml::{Table, Value};

use super::model::{is_valid_credential_name, is_valid_env_name, CredentialRef, REFERENCE_PREFIX};
use crate::error::Violation;
use crate::path::Segments;

const ROOT_KEYS: [&str; 4] = ["version", "default_profile", "profiles", "credentials"];
const PROVIDERS: [&str; 3] = ["env", "keychain", "command"];
/// Exact sensitive field names (SPEC-020), matched ASCII case-insensitively.
const SENSITIVE_EXACT: [&str; 5] = ["token", "password", "secret", "api_key", "private_key"];
/// Sensitive field-name suffixes (SPEC-020), matched ASCII case-insensitively.
const SENSITIVE_SUFFIX: [&str; 5] = ["_token", "_password", "_secret", "_api_key", "_private_key"];

/// Runs all core rules over the parsed root table, returning every violation
/// in a deterministic traversal order.
pub(crate) fn validate(root: &Table) -> Vec<Violation> {
    let mut violations = Vec::new();
    validate_version(root, &mut violations);
    validate_default_profile(root, &mut violations);
    validate_root_keys(root, &mut violations);
    validate_profiles(root, &mut violations);
    validate_sensitive_names(root, &mut violations);
    validate_credentials(root, &mut violations);
    violations
}

fn validate_version(root: &Table, violations: &mut Vec<Violation>) {
    // SPEC-002 rules 1 and 11.
    match root.get("version") {
        Some(Value::Integer(1)) => {}
        Some(Value::Integer(version)) => violations.push(Violation {
            path: "version".to_owned(),
            message: format!(
                "unsupported version {version}; only version 1 is supported; set 'version = 1' \
                 in the config file"
            ),
        }),
        Some(_) => violations.push(Violation {
            path: "version".to_owned(),
            message: "version must be the integer 1; set 'version = 1' in the config file"
                .to_owned(),
        }),
        None => violations.push(Violation {
            path: "version".to_owned(),
            message: "version is required; set 'version = 1' at the top of the config file"
                .to_owned(),
        }),
    }
}

fn validate_default_profile(root: &Table, violations: &mut Vec<Violation>) {
    // SPEC-002 rules 2 and 11.
    let Some(value) = root.get("default_profile") else {
        return;
    };
    let Some(name) = value.as_str() else {
        violations.push(Violation {
            path: "default_profile".to_owned(),
            message: "default_profile must be a string naming a defined profile".to_owned(),
        });
        return;
    };
    let defined = root
        .get("profiles")
        .and_then(Value::as_table)
        .is_some_and(|profiles| profiles.contains_key(name));
    if !defined {
        violations.push(Violation {
            path: "default_profile".to_owned(),
            message: format!(
                "default_profile '{name}' names no defined profile; edit default_profile or add \
                 a [profiles.{name}] table"
            ),
        });
    }
}

fn validate_root_keys(root: &Table, violations: &mut Vec<Violation>) {
    // SPEC-002 rule 10: the root schema is closed.
    for key in root.keys() {
        if !ROOT_KEYS.contains(&key.as_str()) {
            violations.push(Violation {
                path: key.clone(),
                message: format!(
                    "unknown top-level key '{key}'; the config root allows only version, \
                     default_profile, profiles, and credentials"
                ),
            });
        }
    }
}

fn validate_profiles(root: &Table, violations: &mut Vec<Violation>) {
    // SPEC-002 rules 3, 9, 11, 5, 7 and the SPEC-013 inject rules.
    let Some(profiles) = root.get("profiles").and_then(Value::as_table) else {
        // Absent is valid; a mistyped container was reported by the
        // container check if the value is not a table.
        if root.get("profiles").is_some() {
            violations.push(Violation {
                path: "profiles".to_owned(),
                message: "profiles must be a table; write each profile as a [profiles.<name>] \
                          section"
                    .to_owned(),
            });
        }
        return;
    };
    let credentials = root.get("credentials").and_then(Value::as_table);
    for (name, value) in profiles {
        let Some(profile) = value.as_table() else {
            violations.push(Violation {
                path: format!("profiles.{name}"),
                message: format!("profile '{name}' must be a table; write it as [profiles.{name}]"),
            });
            continue;
        };
        require_description(
            profile,
            &format!("profiles.{name}"),
            &format!("profile '{name}'"),
            violations,
        );
        for (entry_name, entry_value) in profile {
            if entry_name == "description" {
                continue;
            }
            let entry_path = format!("profiles.{name}.{entry_name}");
            let Some(entry) = entry_value.as_table() else {
                // SPEC-002 rule 9: profile-level keys must be entries.
                violations.push(Violation {
                    path: entry_path,
                    message: format!(
                        "profile-level key '{entry_name}' must be a table (an entry); move it \
                         into an entry such as [profiles.{name}.{entry_name}]"
                    ),
                });
                continue;
            };
            require_description(
                entry,
                &entry_path,
                &format!("entry '{entry_name}'"),
                violations,
            );
            validate_inject_table(name, entry_name, entry, violations);
            scan_entry_references(&entry_path, entry, credentials, violations);
        }
    }
}

fn require_description(table: &Table, path: &str, owner: &str, violations: &mut Vec<Violation>) {
    // SPEC-002 rule 3.
    match table.get("description") {
        Some(Value::String(description)) if !description.is_empty() => {}
        Some(Value::String(_)) => violations.push(Violation {
            path: format!("{path}.description"),
            message: format!("{owner} requires a non-empty description"),
        }),
        Some(_) => violations.push(Violation {
            path: format!("{path}.description"),
            message: format!("{owner} description must be a non-empty string"),
        }),
        None => violations.push(Violation {
            path: format!("{path}.description"),
            message: format!(
                "{owner} requires a non-empty string description; add a description field"
            ),
        }),
    }
}

fn validate_inject_table(
    profile: &str,
    entry: &str,
    entry_table: &Table,
    violations: &mut Vec<Violation>,
) {
    // SPEC-002 rule 7 and SPEC-013.
    let Some(inject) = entry_table.get("inject") else {
        return;
    };
    let Some(inject_table) = inject.as_table() else {
        violations.push(Violation {
            path: format!("profiles.{profile}.{entry}.inject"),
            message: "inject must be a table mapping environment variable names to field paths \
                      in this entry"
                .to_owned(),
        });
        return;
    };
    for (key, value) in inject_table {
        let violation_path = format!("profiles.{profile}.{entry}.inject.{key}");
        if !is_valid_env_name(key) {
            violations.push(Violation {
                path: violation_path.clone(),
                message: format!(
                    "'{key}' is not a valid environment variable name (expected \
                     [A-Za-z_][A-Za-z0-9_]*)"
                ),
            });
        }
        let Some(source) = value.as_str() else {
            violations.push(Violation {
                path: violation_path,
                message: format!(
                    "inject value for '{key}' must be a string field path, not a {}",
                    value.type_str()
                ),
            });
            continue;
        };
        let segments = match Segments::parse(source) {
            Ok(segments) => segments,
            Err(error) => {
                violations.push(Violation {
                    path: violation_path,
                    message: format!("inject value for '{key}' is not a valid field path: {error}"),
                });
                continue;
            }
        };
        if segments
            .as_slice()
            .first()
            .is_some_and(|first| first == "inject")
        {
            violations.push(Violation {
                path: violation_path,
                message: format!(
                    "inject value for '{key}' points at '{source}', which starts with the \
                     reserved inject table; the inject table cannot be a source of itself"
                ),
            });
            continue;
        }
        match resolve_in_entry(entry_table, &segments) {
            None => violations.push(Violation {
                path: violation_path,
                message: format!(
                    "inject value for '{key}' does not resolve within entry '{entry}': \
                     '{source}'; run 'agent-context list {entry}' to see its fields"
                ),
            }),
            Some(Value::String(source_value)) => {
                if source_value.starts_with(REFERENCE_PREFIX) {
                    violations.push(Violation {
                        path: violation_path,
                        message: format!(
                            "inject value for '{key}' points at '{source}', a credential \
                             reference; credentials inject via inject_as or '?as=' instead"
                        ),
                    });
                } else if source_value.contains('\0') {
                    violations.push(Violation {
                        path: violation_path,
                        message: format!(
                            "inject value for '{key}' points at '{source}', a string \
                             containing a NUL byte, which cannot be injected into a process \
                             environment"
                        ),
                    });
                }
            }
            Some(Value::Integer(_) | Value::Float(_) | Value::Boolean(_)) => {}
            Some(other) => violations.push(Violation {
                path: violation_path,
                message: format!(
                    "inject value for '{key}' points at '{source}', whose type is {}; only \
                     string, integer, float, and boolean fields are injectable",
                    other.type_str()
                ),
            }),
        }
    }
}

fn resolve_in_entry<'a>(entry_table: &'a Table, segments: &Segments) -> Option<&'a Value> {
    let parts = segments.as_slice();
    let mut current = entry_table.get(parts.first()?)?;
    for segment in &parts[1..] {
        current = current.as_table()?.get(segment)?;
    }
    Some(current)
}

fn scan_entry_references(
    entry_path: &str,
    entry_table: &Table,
    credentials: Option<&Table>,
    violations: &mut Vec<Violation>,
) {
    // SPEC-002 rule 5 over the Reference scanning scope (SPEC-AS-015): the
    // entry-level inject table and description keys are never scanned.
    for (key, value) in entry_table {
        if key == "description" || key == "inject" {
            continue;
        }
        scan_value_references(
            &format!("{entry_path}.{key}"),
            value,
            credentials,
            violations,
        );
    }
}

fn scan_value_references(
    path: &str,
    value: &Value,
    credentials: Option<&Table>,
    violations: &mut Vec<Violation>,
) {
    match value {
        Value::String(text) => {
            if !text.starts_with(REFERENCE_PREFIX) {
                return;
            }
            match CredentialRef::parse(text) {
                Err(message) => violations.push(Violation {
                    path: path.to_owned(),
                    message,
                }),
                Ok(reference) => {
                    let defined =
                        credentials.is_some_and(|table| table.contains_key(&reference.name));
                    if !defined {
                        violations.push(Violation {
                            path: path.to_owned(),
                            message: format!(
                                "credential '{}' is not defined; add a [credentials.{}] table \
                                 or fix the reference",
                                reference.name, reference.name
                            ),
                        });
                    }
                }
            }
        }
        Value::Table(table) => {
            for (key, inner) in table {
                if key == "description" {
                    continue;
                }
                scan_value_references(&format!("{path}.{key}"), inner, credentials, violations);
            }
        }
        // Arrays stop reference recognition entirely; non-string scalars
        // cannot be references.
        _ => {}
    }
}

fn validate_sensitive_names(root: &Table, violations: &mut Vec<Violation>) {
    // SPEC-020 / SPEC-002 rule 8: a traversal deliberately broader than the
    // reference scan - it covers every table field at any depth under every
    // profile, including tables nested inside arrays, and excludes only the
    // reserved entry-level inject table.
    let Some(profiles) = root.get("profiles").and_then(Value::as_table) else {
        return;
    };
    for (name, value) in profiles {
        let Some(profile) = value.as_table() else {
            continue;
        };
        walk_sensitive_table(
            &format!("profiles.{name}"),
            profile,
            TableLevel::Profile,
            violations,
        );
    }
}

#[derive(Clone, Copy)]
enum TableLevel {
    /// The profile table: its table children are entries.
    Profile,
    /// An entry table: its `inject` key (when a table) is the reserved one.
    Entry,
    /// Any deeper table: ordinary open-schema data.
    Deep,
}

impl TableLevel {
    fn child(self) -> Self {
        match self {
            Self::Profile => Self::Entry,
            Self::Entry | Self::Deep => Self::Deep,
        }
    }
}

fn walk_sensitive_table(
    path: &str,
    table: &Table,
    level: TableLevel,
    violations: &mut Vec<Violation>,
) {
    for (key, value) in table {
        if matches!(level, TableLevel::Entry) && key == "inject" && value.as_table().is_some() {
            continue;
        }
        let field_path = format!("{path}.{key}");
        match value {
            Value::String(text) => {
                if is_sensitive_name(key) && !text.starts_with(REFERENCE_PREFIX) {
                    violations.push(Violation {
                        path: field_path,
                        message: format!(
                            "field '{key}' appears to hold a plaintext secret; store the value \
                             in a credential and reference it with '{REFERENCE_PREFIX}<name>' \
                             instead"
                        ),
                    });
                }
            }
            Value::Table(inner) => {
                walk_sensitive_table(&field_path, inner, level.child(), violations);
            }
            Value::Array(array) => {
                walk_sensitive_array(&field_path, array, violations);
            }
            _ => {}
        }
    }
}

fn walk_sensitive_array(path: &str, array: &[Value], violations: &mut Vec<Violation>) {
    for (index, element) in array.iter().enumerate() {
        let element_path = format!("{path}[{index}]");
        match element {
            Value::Table(inner) => {
                walk_sensitive_table(&element_path, inner, TableLevel::Deep, violations);
            }
            Value::Array(inner) => walk_sensitive_array(&element_path, inner, violations),
            _ => {}
        }
    }
}

fn is_sensitive_name(name: &str) -> bool {
    let lowered = name.to_ascii_lowercase();
    SENSITIVE_EXACT.contains(&lowered.as_str())
        || SENSITIVE_SUFFIX
            .iter()
            .any(|suffix| lowered.ends_with(suffix))
}

fn validate_credentials(root: &Table, violations: &mut Vec<Violation>) {
    // SPEC-002 rule 4 and rule 11.
    match root.get("credentials") {
        None => {}
        Some(value) if value.as_table().is_none() => violations.push(Violation {
            path: "credentials".to_owned(),
            message: "credentials must be a table; write each credential as a \
                      [credentials.<name>] section"
                .to_owned(),
        }),
        Some(value) => {
            let credentials = value.as_table().expect("checked above");
            for (name, definition_value) in credentials {
                let Some(definition) = definition_value.as_table() else {
                    violations.push(Violation {
                        path: format!("credentials.{name}"),
                        message: format!(
                            "credential '{name}' must be a table; write it as \
                             [credentials.{name}]"
                        ),
                    });
                    continue;
                };
                validate_credential(name, definition, violations);
            }
        }
    }
}

fn validate_credential(name: &str, definition: &Table, violations: &mut Vec<Violation>) {
    let path = format!("credentials.{name}");
    if !is_valid_credential_name(name) {
        violations.push(Violation {
            path: path.clone(),
            message: format!(
                "credential name '{name}' must match [A-Za-z0-9_-]+; rename the \
                 [credentials.{name}] table"
            ),
        });
    }
    require_description(
        definition,
        &path,
        &format!("credential '{name}'"),
        violations,
    );
    match definition.get("inject_as") {
        Some(Value::String(inject_as)) if !inject_as.is_empty() => {
            // SPEC-002 rule 6.
            if !is_valid_env_name(inject_as) {
                violations.push(Violation {
                    path: format!("{path}.inject_as"),
                    message: format!(
                        "inject_as '{inject_as}' is not a valid environment variable name \
                         (expected [A-Za-z_][A-Za-z0-9_]*)"
                    ),
                });
            }
        }
        Some(Value::String(_)) => violations.push(Violation {
            path: format!("{path}.inject_as"),
            message: format!(
                "credential '{name}' requires a non-empty string inject_as naming the target \
                 environment variable"
            ),
        }),
        Some(_) => violations.push(Violation {
            path: format!("{path}.inject_as"),
            message: "inject_as must be a non-empty string naming the target environment \
                      variable"
                .to_owned(),
        }),
        None => violations.push(Violation {
            path: format!("{path}.inject_as"),
            message: format!(
                "credential '{name}' requires a non-empty string inject_as naming the target \
                 environment variable; add an inject_as field"
            ),
        }),
    }
    let provider = match definition.get("provider") {
        Some(Value::String(provider)) if PROVIDERS.contains(&provider.as_str()) => {
            Some(provider.as_str())
        }
        Some(Value::String(provider)) => {
            violations.push(Violation {
                path: format!("{path}.provider"),
                message: format!(
                    "provider '{provider}' is not supported; expected one of: env, keychain, \
                     command"
                ),
            });
            None
        }
        Some(_) => {
            violations.push(Violation {
                path: format!("{path}.provider"),
                message: "provider must be one of: env, keychain, command".to_owned(),
            });
            None
        }
        None => {
            violations.push(Violation {
                path: format!("{path}.provider"),
                message: format!(
                    "credential '{name}' requires a provider; expected one of: env, keychain, \
                     command"
                ),
            });
            None
        }
    };
    match provider {
        Some("env") => match definition.get("name") {
            Some(Value::String(variable)) if !variable.is_empty() => {
                if !is_valid_env_name(variable) {
                    violations.push(Violation {
                        path: format!("{path}.name"),
                        message: format!(
                            "'{variable}' is not a valid environment variable name (expected \
                             [A-Za-z_][A-Za-z0-9_]*)"
                        ),
                    });
                }
            }
            _ => violations.push(Violation {
                path: format!("{path}.name"),
                message: format!(
                    "env credential '{name}' requires a non-empty string 'name' naming the \
                     environment variable"
                ),
            }),
        },
        Some("keychain") => {
            for field in ["service", "account"] {
                match definition.get(field) {
                    Some(Value::String(value)) if !value.is_empty() => {}
                    _ => violations.push(Violation {
                        path: format!("{path}.{field}"),
                        message: format!(
                            "keychain credential '{name}' requires a non-empty string \
                             '{field}'"
                        ),
                    }),
                }
            }
        }
        Some("command") => match definition.get("argv") {
            Some(Value::Array(argv)) if !argv.is_empty() => {
                let all_strings = argv.iter().all(|item| item.as_str().is_some());
                let first_non_empty = argv
                    .first()
                    .and_then(Value::as_str)
                    .is_some_and(|first| !first.is_empty());
                if !all_strings {
                    violations.push(Violation {
                        path: format!("{path}.argv"),
                        message: format!(
                            "command credential '{name}' requires argv to be an array of \
                             strings"
                        ),
                    });
                } else if !first_non_empty {
                    violations.push(Violation {
                        path: format!("{path}.argv"),
                        message: format!(
                            "command credential '{name}' requires a non-empty argv[0] naming \
                             the command to run"
                        ),
                    });
                }
            }
            _ => violations.push(Violation {
                path: format!("{path}.argv"),
                message: format!(
                    "command credential '{name}' requires argv, a non-empty array of strings"
                ),
            }),
        },
        _ => {}
    }
    // Closed schema (SPEC-AS-021). Skipped when the provider is missing or
    // unrecognized: the allowed field set is undeterminable then, and the
    // provider violation already names the problem.
    if let Some(provider) = provider {
        let allowed_fields: &[&str] = match provider {
            "env" => &["description", "inject_as", "provider", "name"],
            "keychain" => &["description", "inject_as", "provider", "service", "account"],
            _ => &["description", "inject_as", "provider", "argv"],
        };
        for key in definition.keys() {
            if !allowed_fields.contains(&key.as_str()) {
                violations.push(Violation {
                    path: format!("{path}.{key}"),
                    message: format!(
                        "unknown field '{key}' in credential '{name}'; the credential schema \
                         is closed (allowed: {})",
                        allowed_fields.join(", ")
                    ),
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::validate;
    use toml::Table;

    /// Runs validation over `config` and asserts that every `expected`
    /// (path, message fragment) pair matches exactly one violation; no other
    /// violations may exist.
    fn assert_violations(config: &str, expected: &[(&str, &str)]) {
        let root: Table = config
            .parse()
            .unwrap_or_else(|error| panic!("the fixture should parse: {error}"));
        let violations = validate(&root);
        let summaries: Vec<String> = violations.iter().map(|v| format!("{v}")).collect();
        assert_eq!(
            violations.len(),
            expected.len(),
            "expected exactly {} violations, got {summaries:?}",
            expected.len()
        );
        for (path, fragment) in expected {
            assert!(
                summaries
                    .iter()
                    .any(|summary| summary.starts_with(&format!("{path}: "))
                        && summary.contains(fragment)),
                "expected a violation at {path} mentioning {fragment:?}, got {summaries:?}"
            );
        }
    }

    fn assert_valid(config: &str) {
        let root: Table = config
            .parse()
            .unwrap_or_else(|error| panic!("the fixture should parse: {error}"));
        let violations = validate(&root);
        assert!(
            violations.is_empty(),
            "expected no violations, got {:?}",
            violations.iter().map(|v| v.to_string()).collect::<Vec<_>>()
        );
    }

    const VALID_CONFIG: &str = r#"
version = 1
default_profile = "work"

[profiles.work]
description = "Day-to-day development."

[profiles.work.llm]
description = "Default LLM."
endpoint = "https://llm.example.com/v1"
credential = "credential://company_llm"

[profiles.work.llm.inject]
OPENAI_BASE_URL = "endpoint"

[credentials.company_llm]
description = "Company LLM credential."
provider = "env"
name = "COMPANY_LLM_TOKEN"
inject_as = "OPENAI_API_KEY"
"#;

    #[test]
    fn the_design_example_is_valid() {
        assert_valid(VALID_CONFIG);
    }

    #[test]
    fn version_rule() {
        assert_valid("version = 1\n");
        // EDGE-003: an empty file has no version.
        assert_violations("", &[("version", "required")]);
        // AC-002.2.
        assert_violations(
            "version = 2\n",
            &[("version", "only version 1 is supported")],
        );
        assert_violations("version = \"1\"\n", &[("version", "must be the integer 1")]);
    }

    #[test]
    fn default_profile_rule() {
        assert_violations(
            "version = 1\ndefault_profile = \"work\"\n",
            &[("default_profile", "names no defined profile")],
        );
        assert_violations(
            "version = 1\ndefault_profile = 1\n",
            &[("default_profile", "must be a string")],
        );
        // A defined name is fine.
        assert_valid(
            "version = 1\ndefault_profile = \"work\"\n\n[profiles.work]\ndescription = \"d\"\n",
        );
    }

    #[test]
    fn closed_root_schema() {
        // EDGE-019.
        assert_violations(
            "version = 1\ndefualt_profile = \"work\"\n",
            &[("defualt_profile", "unknown top-level key")],
        );
        assert_violations(
            "version = 1\n[credential]\nx = 1\n",
            &[("credential", "unknown top-level key")],
        );
    }

    #[test]
    fn container_type_rules() {
        // AC-002.8 first half: a mistyped container is a core violation.
        assert_violations(
            "version = 1\nprofiles = []\n",
            &[("profiles", "must be a table")],
        );
        assert_violations(
            "version = 1\ncredentials = \"x\"\n",
            &[("credentials", "must be a table")],
        );
        assert_violations(
            "version = 1\n[profiles]\nwork = \"x\"\n",
            &[("profiles.work", "must be a table")],
        );
        assert_violations(
            "version = 1\n[credentials]\nc = 1\n",
            &[("credentials.c", "must be a table")],
        );
    }

    #[test]
    fn mistyped_container_aggregates_with_other_violations() {
        // AC-002.8: profiles = [] plus a credential missing inject_as.
        assert_violations(
            "version = 1\nprofiles = []\n\n[credentials.c]\ndescription = \"d\"\nprovider = \"env\"\nname = \"C\"\n",
            &[
                ("profiles", "must be a table"),
                ("credentials.c.inject_as", "inject_as"),
            ],
        );
    }

    #[test]
    fn description_rules() {
        // AC-002.1: an entry without a description.
        assert_violations(
            "version = 1\n\n[profiles.work]\ndescription = \"d\"\n\n[profiles.work.llm]\nendpoint = \"x\"\n",
            &[("profiles.work.llm.description", "requires a non-empty string description")],
        );
        assert_violations(
            "version = 1\n\n[profiles.work]\n\n[profiles.work.llm]\ndescription = \"d\"\n",
            &[(
                "profiles.work.description",
                "requires a non-empty string description",
            )],
        );
        assert_violations(
            "version = 1\n\n[profiles.work]\ndescription = \"\"\n",
            &[("profiles.work.description", "non-empty")],
        );
        assert_violations(
            "version = 1\n\n[profiles.work]\ndescription = 1\n",
            &[("profiles.work.description", "non-empty string")],
        );
    }

    #[test]
    fn profile_level_scalars_are_violations() {
        // AC-002.7.
        assert_violations(
            "version = 1\n\n[profiles.work]\ndescription = \"d\"\nregion = \"eu\"\n",
            &[("profiles.work.region", "must be a table (an entry)")],
        );
        assert_violations(
            "version = 1\n\n[profiles.work]\ndescription = \"d\"\ntags = [\"a\"]\n",
            &[("profiles.work.tags", "must be a table (an entry)")],
        );
    }

    #[test]
    fn three_independent_violations_are_all_reported() {
        // AC-002.4.
        assert_violations(
            "version = 1\ndefault_profile = \"work\"\n\n[profiles.work]\ndescription = \"d\"\n\n[profiles.work.llm]\nendpoint = \"x\"\ninject = \"x\"\n\n[credentials.c]\ndescription = \"d\"\nprovider = \"vault\"\nname = \"X\"\ninject_as = \"X\"\n",
            &[
                ("profiles.work.llm.description", "description"),
                ("profiles.work.llm.inject", "inject must be a table"),
                ("credentials.c.provider", "not supported"),
            ],
        );
    }

    #[test]
    fn credential_name_pattern() {
        // AC-002.6 first case.
        assert_violations(
            "version = 1\n\n[credentials.\"my cred\"]\ndescription = \"d\"\nprovider = \"env\"\nname = \"X\"\ninject_as = \"X\"\n",
            &[("credentials.my cred", "must match [A-Za-z0-9_-]+")],
        );
    }

    #[test]
    fn credential_closed_schema() {
        // AC-002.6 second case.
        assert_violations(
            "version = 1\n\n[credentials.c]\ndescription = \"d\"\nprovider = \"env\"\nname = \"X\"\ninject_as = \"X\"\nextra = 1\n",
            &[("credentials.c.extra", "closed")],
        );
    }

    #[test]
    fn credential_env_name_must_be_a_valid_env_name() {
        // AC-002.6 third case.
        assert_violations(
            "version = 1\n\n[credentials.c]\ndescription = \"d\"\nprovider = \"env\"\nname = \"1BAD\"\ninject_as = \"X\"\n",
            &[("credentials.c.name", "not a valid environment variable name")],
        );
    }

    #[test]
    fn credential_required_fields() {
        assert_violations(
            "version = 1\n\n[credentials.c]\nprovider = \"env\"\nname = \"X\"\ninject_as = \"X\"\n",
            &[("credentials.c.description", "description")],
        );
        assert_violations(
            "version = 1\n\n[credentials.c]\ndescription = \"d\"\nprovider = \"env\"\nname = \"X\"\n",
            &[("credentials.c.inject_as", "inject_as")],
        );
        assert_violations(
            "version = 1\n\n[credentials.c]\ndescription = \"d\"\nprovider = \"env\"\nname = \"X\"\ninject_as = \"X\"\n",
            &[],
        );
        assert_violations(
            "version = 1\n\n[credentials.c]\ndescription = \"d\"\nprovider = \"env\"\ninject_as = \"X\"\n",
            &[("credentials.c.name", "non-empty string 'name'")],
        );
        // SPEC-002 rule 6: inject_as must be a valid env name.
        assert_violations(
            "version = 1\n\n[credentials.c]\ndescription = \"d\"\nprovider = \"env\"\nname = \"X\"\ninject_as = \"9X\"\n",
            &[("credentials.c.inject_as", "not a valid environment variable name")],
        );
    }

    #[test]
    fn credential_provider_rules() {
        assert_violations(
            "version = 1\n\n[credentials.c]\ndescription = \"d\"\nname = \"X\"\ninject_as = \"X\"\n",
            &[("credentials.c.provider", "requires a provider")],
        );
        assert_violations(
            "version = 1\n\n[credentials.c]\ndescription = \"d\"\nprovider = \"keychain\"\nservice = \"s\"\ninject_as = \"X\"\n",
            &[("credentials.c.account", "non-empty string 'account'")],
        );
        assert_violations(
            "version = 1\n\n[credentials.c]\ndescription = \"d\"\nprovider = \"command\"\ninject_as = \"X\"\n",
            &[("credentials.c.argv", "non-empty array")],
        );
        assert_violations(
            "version = 1\n\n[credentials.c]\ndescription = \"d\"\nprovider = \"command\"\nargv = []\ninject_as = \"X\"\n",
            &[("credentials.c.argv", "non-empty array")],
        );
        assert_violations(
            "version = 1\n\n[credentials.c]\ndescription = \"d\"\nprovider = \"command\"\nargv = [\"\"]\ninject_as = \"X\"\n",
            &[("credentials.c.argv", "argv[0]")],
        );
        assert_violations(
            "version = 1\n\n[credentials.c]\ndescription = \"d\"\nprovider = \"command\"\nargv = [\"op\", 1]\ninject_as = \"X\"\n",
            &[("credentials.c.argv", "array of strings")],
        );
        assert_valid(
            "version = 1\n\n[credentials.c]\ndescription = \"d\"\nprovider = \"command\"\nargv = [\"op\", \"\"]\ninject_as = \"X\"\n",
        );
    }

    #[test]
    fn keychain_credential_is_valid_with_both_fields() {
        assert_valid(
            "version = 1\n\n[credentials.c]\ndescription = \"d\"\nprovider = \"keychain\"\nservice = \"agent-context\"\naccount = \"personal\"\ninject_as = \"X\"\n",
        );
    }

    #[test]
    fn reference_rules() {
        // AC-002.3: an undefined reference name.
        assert_violations(
            "version = 1\n\n[profiles.work]\ndescription = \"d\"\n\n[profiles.work.llm]\ndescription = \"d\"\ncredential = \"credential://missing\"\n",
            &[("profiles.work.llm.credential", "credential 'missing' is not defined")],
        );
        // A defined reference resolves cleanly.
        assert_valid(
            "version = 1\n\n[profiles.work]\ndescription = \"d\"\n\n[profiles.work.llm]\ndescription = \"d\"\ncredential = \"credential://c\"\n\n[credentials.c]\ndescription = \"d\"\nprovider = \"env\"\nname = \"X\"\ninject_as = \"X\"\n",
        );
    }

    #[test]
    fn malformed_references_are_load_time_violations() {
        // AC-012.4 and EDGE-007.
        for reference in [
            "credential://company_llm?As=X",
            "credential://",
            "credential://name?x=1",
            "credential://name?as=1BAD",
            "credential://name trailing",
        ] {
            let config = format!(
                "version = 1\n\n[profiles.work]\ndescription = \"d\"\n\n[profiles.work.llm]\ndescription = \"d\"\ncredential = \"{reference}\"\n"
            );
            assert_violations(
                &config,
                &[(
                    "profiles.work.llm.credential",
                    "invalid credential reference",
                )],
            );
        }
    }

    #[test]
    fn references_are_scanned_at_any_depth() {
        assert_valid(
            "version = 1\n\n[profiles.work]\ndescription = \"d\"\n\n[profiles.work.llm]\ndescription = \"d\"\n\n[profiles.work.llm.deep.table]\ncredential = \"credential://c\"\n\n[credentials.c]\ndescription = \"d\"\nprovider = \"env\"\nname = \"X\"\ninject_as = \"X\"\n",
        );
        assert_violations(
            "version = 1\n\n[profiles.work]\ndescription = \"d\"\n\n[profiles.work.llm]\ndescription = \"d\"\n\n[profiles.work.llm.deep]\ncredential = \"credential://nope\"\n",
            &[("profiles.work.llm.deep.credential", "not defined")],
        );
    }

    #[test]
    fn references_in_arrays_are_ordinary_data() {
        // AC-012.5: array elements are never scanned.
        assert_valid(
            "version = 1\n\n[profiles.work]\ndescription = \"d\"\n\n[profiles.work.ci]\ndescription = \"d\"\ntags = [\"credential://company_llm\"]\n",
        );
    }

    #[test]
    fn references_in_descriptions_and_inject_are_not_scanned() {
        // The entry-level inject table and description keys are outside the
        // scanning scope (SPEC-AS-015).
        assert_violations(
            "version = 1\n\n[profiles.work]\ndescription = \"d\"\n\n[profiles.work.llm]\ndescription = \"see credential://c\"\n\n[profiles.work.llm.inject]\nTOKEN = \"credential://c\"\n",
            &[("profiles.work.llm.inject.TOKEN", "does not resolve")],
        );
    }

    #[test]
    fn sensitive_name_matrix() {
        // AC-020.1: a plaintext secret under a sensitive name.
        assert_violations(
            "version = 1\n\n[profiles.work]\ndescription = \"d\"\n\n[profiles.work.llm]\ndescription = \"d\"\nendpoint = \"x\"\napi_key = \"sk-live-123\"\n",
            &[("profiles.work.llm.api_key", "credential://")],
        );
        // AC-020.2: a credential reference is fine.
        assert_valid(
            "version = 1\n\n[profiles.work]\ndescription = \"d\"\n\n[profiles.work.github]\ndescription = \"d\"\ngithub_token = \"credential://gh\"\n\n[credentials.gh]\ndescription = \"d\"\nprovider = \"env\"\nname = \"X\"\ninject_as = \"X\"\n",
        );
        // AC-020.3: token_endpoint and use_token are not sensitive names.
        assert_valid(
            "version = 1\n\n[profiles.work]\ndescription = \"d\"\n\n[profiles.work.llm]\ndescription = \"d\"\ntoken_endpoint = \"https://x\"\nuse_token = true\n",
        );
        // AC-020.4: one level down.
        assert_violations(
            "version = 1\n\n[profiles.work]\ndescription = \"d\"\n\n[profiles.work.llm]\ndescription = \"d\"\n\n[profiles.work.llm.extra]\napi_key = \"sk-live-123\"\n",
            &[("profiles.work.llm.extra.api_key", "credential://")],
        );
        // AC-020.5: a table nested inside an array.
        assert_violations(
            "version = 1\n\n[profiles.work]\ndescription = \"d\"\n\n[profiles.work.db]\ndescription = \"d\"\nhost = \"db\"\nrecords = [{ api_key = \"sk-live-123\" }]\n",
            &[("profiles.work.db.records[0].api_key", "credential://")],
        );
        // AC-020.6: ASCII case-insensitive match.
        assert_violations(
            "version = 1\n\n[profiles.work]\ndescription = \"d\"\n\n[profiles.work.llm]\ndescription = \"d\"\nTOKEN = \"sk-live-123\"\n",
            &[("profiles.work.llm.TOKEN", "credential://")],
        );
        assert_violations(
            "version = 1\n\n[profiles.work]\ndescription = \"d\"\n\n[profiles.work.llm]\ndescription = \"d\"\nApi_Key = \"sk-live-123\"\n",
            &[("profiles.work.llm.Api_Key", "credential://")],
        );
        // Suffix names count; non-strings do not.
        assert_violations(
            "version = 1\n\n[profiles.work]\ndescription = \"d\"\n\n[profiles.work.llm]\ndescription = \"d\"\ndb_password = \"hunter\"\n",
            &[("profiles.work.llm.db_password", "credential://")],
        );
        assert_valid(
            "version = 1\n\n[profiles.work]\ndescription = \"d\"\n\n[profiles.work.llm]\ndescription = \"d\"\ntoken = 5\napi_key = [\"sk\"]\n",
        );
    }

    #[test]
    fn sensitive_names_exclude_the_entry_level_inject_table() {
        // SPEC-020: inject keys are env names, machinery not values.
        assert_valid(
            "version = 1\n\n[profiles.work]\ndescription = \"d\"\n\n[profiles.work.llm]\ndescription = \"d\"\nfield = \"v\"\n\n[profiles.work.llm.inject]\nGITHUB_TOKEN = \"field\"\n",
        );
        // A deeper table named inject is ordinary data and scanned.
        assert_violations(
            "version = 1\n\n[profiles.work]\ndescription = \"d\"\n\n[profiles.work.llm]\ndescription = \"d\"\n\n[profiles.work.llm.docs]\n\n[profiles.work.llm.docs.inject]\napi_key = \"sk\"\n",
            &[("profiles.work.llm.docs.inject.api_key", "credential://")],
        );
    }

    #[test]
    fn inject_rules() {
        let base = "version = 1\n\n[profiles.work]\ndescription = \"d\"\n\n[profiles.work.llm]\ndescription = \"d\"\nendpoint = \"x\"\nretries = 3\nwhen = 2026-08-22\ntags = [\"a\"]\nnulled = \"a\\u0000b\"\n";

        // AC-013.1: a string source is valid.
        let valid = format!("{base}\n[profiles.work.llm.inject]\nOPENAI_BASE_URL = \"endpoint\"\n");
        assert_valid(&valid);

        // AC-013.5: quoted multi-segment paths work.
        let quoted = format!(
            "{base}\n[profiles.work.llm.inject]\nA = 'nested.\"my.key\"'\n\n[profiles.work.llm.nested]\n\"my.key\" = \"v\"\n"
        );
        assert_valid(&quoted);

        // Integer and boolean sources are injectable.
        let numeric = format!("{base}\n[profiles.work.llm.inject]\nRETRIES = \"retries\"\n");
        assert_valid(&numeric);

        // AC-013.2: array and datetime sources are rejected, naming each
        // inject key and path.
        let non_injectable =
            format!("{base}\n[profiles.work.llm.inject]\nTAGS = \"tags\"\nWHEN = \"when\"\n");
        assert_violations(
            &non_injectable,
            &[
                ("profiles.work.llm.inject.TAGS", "whose type is array"),
                ("profiles.work.llm.inject.WHEN", "whose type is datetime"),
            ],
        );

        // AC-013.3: a credential-reference source directs to inject_as.
        let reference_source = format!(
            "{base}\ncredential = \"credential://c\"\n\n[credentials.c]\ndescription = \"d\"\nprovider = \"env\"\nname = \"X\"\ninject_as = \"X\"\n\n[profiles.work.llm.inject]\nKEY = \"credential\"\n"
        );
        assert_violations(
            &reference_source,
            &[("profiles.work.llm.inject.KEY", "inject_as")],
        );

        // AC-013.4: self-referential path and NUL-bearing source.
        let self_referential = format!(
            "{base}\n[profiles.work.llm.inject]\nA = \"inject.B\"\nB = \"endpoint\"\nNULLED = \"nulled\"\n"
        );
        assert_violations(
            &self_referential,
            &[
                ("profiles.work.llm.inject.A", "cannot be a source of itself"),
                ("profiles.work.llm.inject.NULLED", "NUL"),
            ],
        );

        // Unresolved paths and invalid keys/values.
        let unresolved = format!("{base}\n[profiles.work.llm.inject]\nA = \"nope\"\n");
        assert_violations(
            &unresolved,
            &[("profiles.work.llm.inject.A", "does not resolve")],
        );
        let bad_key = format!("{base}\n[profiles.work.llm.inject]\n\"1BAD\" = \"endpoint\"\n");
        assert_violations(
            &bad_key,
            &[(
                "profiles.work.llm.inject.1BAD",
                "not a valid environment variable name",
            )],
        );
        let non_string = format!("{base}\n[profiles.work.llm.inject]\nA = 1\n");
        assert_violations(
            &non_string,
            &[("profiles.work.llm.inject.A", "must be a string field path")],
        );
        let bad_grammar = format!("{base}\n[profiles.work.llm.inject]\nA = \"a..b\"\n");
        assert_violations(
            &bad_grammar,
            &[("profiles.work.llm.inject.A", "not a valid field path")],
        );

        // Rule 7: an empty inject table is valid; a non-table inject is not.
        let empty_inject = format!("{base}\n[profiles.work.llm.inject]\n");
        assert_valid(&empty_inject);
        let non_table = format!("{base}\ninject = \"x\"\n");
        assert_violations(
            &non_table,
            &[("profiles.work.llm.inject", "inject must be a table")],
        );
    }

    #[test]
    fn absent_profiles_and_credentials_are_valid() {
        // EDGE-016.
        assert_valid("version = 1\n");
        assert_valid("version = 1\n\n[profiles.work]\ndescription = \"d\"\n");
    }
}

#[cfg(test)]
mod fixture_tests {
    //! Validation checked against the T002 fixture files (read-only
    //! contract) that the Phase-1 security suite exercises, so the logic
    //! here and the integration expectations cannot drift apart.

    use std::fs;
    use std::path::Path;

    use super::validate;
    use toml::Table;

    fn fixture(name: &str) -> String {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("fixtures")
            .join(name);
        fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("failed to read fixture {name}: {error}"))
    }

    fn violations_of(name: &str) -> Vec<String> {
        let root: Table = fixture(name)
            .parse()
            .unwrap_or_else(|error| panic!("fixture {name} should parse: {error}"));
        validate(&root)
            .iter()
            .map(|violation| violation.to_string())
            .collect()
    }

    #[test]
    fn the_design_example_fixture_is_valid() {
        assert!(
            violations_of("example.toml").is_empty(),
            "example.toml must validate cleanly"
        );
    }

    #[test]
    fn sensitive_ok_fixture_is_valid() {
        // AC-020.2 / AC-020.3: references, token_endpoint, and use_token
        // are all fine.
        assert!(
            violations_of("sensitive_ok.toml").is_empty(),
            "sensitive_ok.toml must validate cleanly"
        );
    }

    #[test]
    fn sensitive_plain_fixture_names_the_field() {
        // AC-020.1 / AC-002.5.
        let violations = violations_of("sensitive_plain.toml");
        assert_eq!(violations.len(), 1, "{violations:?}");
        assert!(
            violations[0].starts_with("profiles.work.llm.api_key: "),
            "{violations:?}"
        );
        assert!(violations[0].contains("credential://"), "{violations:?}");
        // The planted sentinel value must not appear.
        assert!(!violations[0].contains("sk-sentinel"), "{violations:?}");
    }

    #[test]
    fn sensitive_nested_fixture_names_the_full_path() {
        // AC-020.4.
        let violations = violations_of("sensitive_nested.toml");
        assert_eq!(violations.len(), 1, "{violations:?}");
        assert!(
            violations[0].starts_with("profiles.work.llm.extra.api_key: "),
            "{violations:?}"
        );
    }

    #[test]
    fn sensitive_array_fixture_uses_index_notation() {
        // AC-020.5.
        let violations = violations_of("sensitive_array.toml");
        assert_eq!(violations.len(), 1, "{violations:?}");
        assert!(
            violations[0].starts_with("profiles.work.db.records[0].api_key: "),
            "{violations:?}"
        );
    }

    #[test]
    fn sensitive_upper_fixture_matches_case_insensitively() {
        // AC-020.6.
        let violations = violations_of("sensitive_upper.toml");
        assert_eq!(violations.len(), 1, "{violations:?}");
        assert!(
            violations[0].starts_with("profiles.work.llm.TOKEN: "),
            "{violations:?}"
        );
    }

    #[test]
    fn parse_error_fixture_reports_position() {
        // AC-019.3 at the validation layer: the fixture fails to parse, so
        // the loader's parse diagnostics own it; here we only confirm the
        // fixture is indeed unparseable.
        let result = fixture("parse_error_sentinel.toml").parse::<Table>();
        assert!(result.is_err(), "the sentinel fixture must not parse");
    }
}

#[cfg(test)]
mod interpretation_tests {
    //! Checks for the scanning-scope interpretation recorded in the task
    //! report: only the entry-level `inject` table is excluded from
    //! reference scanning; deeper tables named `inject` are ordinary data.

    use super::validate;
    use toml::Table;

    fn violations_of(config: &str) -> Vec<String> {
        let root: Table = config
            .parse()
            .unwrap_or_else(|error| panic!("the fixture should parse: {error}"));
        validate(&root)
            .iter()
            .map(|violation| violation.to_string())
            .collect()
    }

    #[test]
    fn a_nested_inject_table_is_scanned_for_references() {
        let config = "version = 1\n\n[profiles.work]\ndescription = \"d\"\n\n[profiles.work.llm]\ndescription = \"d\"\n\n[profiles.work.llm.docs]\ndescription = \"d\"\n\n[profiles.work.llm.docs.inject]\nEXAMPLE = \"credential://nope\"\n";
        let violations = violations_of(config);
        assert_eq!(violations.len(), 1, "{violations:?}");
        assert!(
            violations[0].starts_with("profiles.work.llm.docs.inject.EXAMPLE: "),
            "{violations:?}"
        );
    }

    #[test]
    fn the_entry_level_inject_table_is_not_scanned_for_references() {
        // An inject value that looks like a reference is a path, not a
        // reference; it fails SPEC-013 resolution instead.
        let config = "version = 1\n\n[profiles.work]\ndescription = \"d\"\n\n[profiles.work.llm]\ndescription = \"d\"\nfield = \"v\"\n\n[profiles.work.llm.inject]\nTOKEN = \"field\"\n";
        let violations = violations_of(config);
        assert!(violations.is_empty(), "{violations:?}");
    }

    #[test]
    fn a_description_key_at_any_depth_is_not_scanned() {
        let config = "version = 1\n\n[profiles.work]\ndescription = \"d\"\n\n[profiles.work.llm]\ndescription = \"d\"\n\n[profiles.work.llm.docs]\ndescription = \"see credential://nope\"\n";
        let violations = violations_of(config);
        assert!(violations.is_empty(), "{violations:?}");
    }
}
