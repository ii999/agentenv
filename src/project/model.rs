//! Closed-schema parsing for checked-in `.agentenv.toml` project files.
//!
//! Project files are untrusted input. Every diagnostic therefore identifies a
//! TOML path or file path without reproducing values or source text.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use toml::{Table, Value};

use crate::error::Violation;
use crate::path::Segments;

/// Largest accepted project-file size: 64 KiB.
pub const MAX_PROJECT_FILE_BYTES: usize = 65_536;

/// A profile pin and the project file that declared it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectPin {
    pub name: String,
    pub file: PathBuf,
}

/// A structural requirement declared by a project file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Requirement {
    pub entry: String,
    pub reason: String,
    pub fields: Vec<String>,
}

/// The validated content that may affect project behavior after trust approval.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ProjectFileMeta {
    pub pin: Option<ProjectPin>,
    pub requires: Vec<Requirement>,
}

/// Parses and validates one `.agentenv.toml` byte snapshot.
///
/// The size limit is checked before decoding or parsing so an oversized file
/// always has one stable violation. TOML parser errors include their location
/// but omit the source line, preserving the no-echo invariant.
pub fn parse(bytes: &[u8], file: &Path) -> Result<ProjectFileMeta, Vec<Violation>> {
    if bytes.len() > MAX_PROJECT_FILE_BYTES {
        return Err(vec![Violation {
            path: file.display().to_string(),
            message: "project file exceeds the 64 KiB limit; reduce its size before running \
                      'agentenv project allow'"
                .to_owned(),
        }]);
    }

    let text = std::str::from_utf8(bytes).map_err(|_| {
        vec![Violation {
            path: file.display().to_string(),
            message: "project file is not valid UTF-8 TOML; fix the file before running \
                      'agentenv project allow'"
                .to_owned(),
        }]
    })?;
    let root = text.parse::<Table>().map_err(|error: toml::de::Error| {
        vec![Violation {
            path: file.display().to_string(),
            message: toml_error_message(text, &error),
        }]
    })?;

    let mut violations = Vec::new();
    validate_version(&root, &mut violations);
    let pin = validate_profile(&root, file, &mut violations);
    validate_root_keys(&root, &mut violations);
    let requires = validate_requires(&root, &mut violations);

    if violations.is_empty() {
        Ok(ProjectFileMeta { pin, requires })
    } else {
        Err(violations)
    }
}

fn toml_error_message(text: &str, error: &toml::de::Error) -> String {
    match error.span() {
        Some(span) => {
            let (line, column) = line_column(text, span.start);
            format!(
                "invalid TOML at line {line}, column {column}: {}",
                error.message()
            )
        }
        None => format!("invalid TOML: {}", error.message()),
    }
}

fn line_column(text: &str, offset: usize) -> (usize, usize) {
    let mut line = 1;
    let mut line_start = 0;
    for (index, character) in text.char_indices() {
        if index >= offset {
            break;
        }
        if character == '\n' {
            line += 1;
            line_start = index + 1;
        }
    }
    let column = text[line_start..]
        .char_indices()
        .take_while(|(index, _)| line_start + index < offset)
        .count()
        + 1;
    (line, column)
}

fn validate_version(root: &Table, violations: &mut Vec<Violation>) {
    match root.get("version") {
        Some(Value::Integer(1)) => {}
        Some(_) => violations.push(violation(
            "version",
            "version must be the integer 1; set 'version = 1'",
        )),
        None => violations.push(violation(
            "version",
            "version is required; set 'version = 1' at the top of the project file",
        )),
    }
}

fn validate_profile(
    root: &Table,
    file: &Path,
    violations: &mut Vec<Violation>,
) -> Option<ProjectPin> {
    let value = root.get("profile")?;
    let Some(name) = value.as_str() else {
        violations.push(violation(
            "profile",
            "profile must be a non-empty string naming a profile",
        ));
        return None;
    };
    if name.is_empty() {
        violations.push(violation(
            "profile",
            "profile must be a non-empty string naming a profile",
        ));
        return None;
    }
    if is_credential_reference(name) {
        violations.push(violation(
            "profile",
            "profile must not be a credential reference; name a profile instead",
        ));
        return None;
    }
    Some(ProjectPin {
        name: name.to_owned(),
        file: file.to_owned(),
    })
}

fn validate_root_keys(root: &Table, violations: &mut Vec<Violation>) {
    for key in root.keys() {
        if !matches!(key.as_str(), "version" | "profile" | "requires") {
            violations.push(violation(
                key,
                "unknown top-level key; project files allow only version, profile, and requires",
            ));
        }
    }
}

fn validate_requires(root: &Table, violations: &mut Vec<Violation>) -> Vec<Requirement> {
    let Some(value) = root.get("requires") else {
        return Vec::new();
    };
    let Some(requires) = value.as_table() else {
        violations.push(violation(
            "requires",
            "requires must be a table of requirement declarations",
        ));
        return Vec::new();
    };

    requires
        .iter()
        .filter_map(|(entry, value)| validate_requirement(entry, value, violations))
        .collect()
}

fn validate_requirement(
    entry: &str,
    value: &Value,
    violations: &mut Vec<Violation>,
) -> Option<Requirement> {
    let entry_path = child_path("requires", entry);
    let entry_is_valid = Segments::parse(entry).is_ok_and(|segments| segments.len() == 1);
    if !entry_is_valid {
        violations.push(violation(
            &entry_path,
            "requirement entry keys must be one addressable path segment",
        ));
    }

    let Some(requirement) = value.as_table() else {
        violations.push(violation(
            &entry_path,
            "requirement declarations must be tables",
        ));
        return None;
    };

    let reason = validate_reason(requirement, &entry_path, violations);
    let fields = validate_fields(requirement, &entry_path, violations);
    validate_requirement_keys(requirement, &entry_path, violations);

    match (entry_is_valid, reason, fields) {
        (true, Some(reason), Some(fields)) => Some(Requirement {
            entry: entry.to_owned(),
            reason,
            fields,
        }),
        _ => None,
    }
}

fn validate_reason(
    requirement: &Table,
    entry_path: &str,
    violations: &mut Vec<Violation>,
) -> Option<String> {
    let path = child_path(entry_path, "reason");
    let Some(value) = requirement.get("reason") else {
        violations.push(violation(
            &path,
            "reason is required and must be a non-empty string",
        ));
        return None;
    };
    let Some(reason) = value.as_str() else {
        violations.push(violation(&path, "reason must be a non-empty string"));
        return None;
    };
    if reason.is_empty() {
        violations.push(violation(&path, "reason must be a non-empty string"));
        return None;
    }
    if is_credential_reference(reason) {
        violations.push(violation(
            &path,
            "reason must not be a credential reference; describe the requirement instead",
        ));
        return None;
    }
    Some(reason.to_owned())
}

fn validate_fields(
    requirement: &Table,
    entry_path: &str,
    violations: &mut Vec<Violation>,
) -> Option<Vec<String>> {
    let Some(value) = requirement.get("fields") else {
        return Some(Vec::new());
    };
    let fields_path = child_path(entry_path, "fields");
    let Some(fields) = value.as_array() else {
        violations.push(violation(
            &fields_path,
            "fields must be a non-empty array of field-path strings",
        ));
        return None;
    };
    if fields.is_empty() {
        violations.push(violation(
            &fields_path,
            "fields must be non-empty when it is declared",
        ));
        return None;
    }

    let mut valid = true;
    let mut normalized = HashSet::new();
    let mut result = Vec::with_capacity(fields.len());
    for (index, value) in fields.iter().enumerate() {
        let item_path = format!("{fields_path}[{index}]");
        let Some(field) = value.as_str() else {
            violations.push(violation(&item_path, "field paths must be strings"));
            valid = false;
            continue;
        };
        if is_credential_reference(field) {
            violations.push(violation(
                &item_path,
                "field paths must not be credential references",
            ));
            valid = false;
            continue;
        }
        let Ok(segments) = Segments::parse(field) else {
            violations.push(violation(
                &item_path,
                "field paths must use the accepted path-segment grammar",
            ));
            valid = false;
            continue;
        };
        if !normalized.insert(segments.render()) {
            violations.push(violation(
                &item_path,
                "field paths must not contain duplicates",
            ));
            valid = false;
            continue;
        }
        result.push(field.to_owned());
    }

    valid.then_some(result)
}

fn validate_requirement_keys(
    requirement: &Table,
    entry_path: &str,
    violations: &mut Vec<Violation>,
) {
    for key in requirement.keys() {
        if !matches!(key.as_str(), "reason" | "fields") {
            violations.push(violation(
                child_path(entry_path, key),
                "unknown requirement key; declarations allow only reason and fields",
            ));
        }
    }
}

fn is_credential_reference(value: &str) -> bool {
    value.starts_with("credential://")
}

fn child_path(parent: &str, child: &str) -> String {
    format!("{parent}.{}", toml_path_segment(child))
}

fn toml_path_segment(segment: &str) -> String {
    if segment.is_empty()
        || segment
            .chars()
            .any(|character| character == '.' || character == '"' || character.is_whitespace())
    {
        format!("\"{segment}\"")
    } else {
        segment.to_owned()
    }
}

fn violation(path: impl Into<String>, message: impl Into<String>) -> Violation {
    Violation {
        path: path.into(),
        message: message.into(),
    }
}
