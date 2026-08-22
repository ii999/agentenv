//! Read-only query views.
//!
//! The types in this module deliberately retain configuration metadata only.
//! Credential references are represented by their stored URI and a shallow
//! summary; none of the view variants can contain a resolved secret.

use serde_json::{Map, Number, Value as JsonValue};
use toml::{Table, Value};

use crate::config::validate::recognized_reference;
use crate::config::{Config, CredentialDef, Profile};
use crate::credential::{shallow_status, Status};
use crate::error::AppError;
use crate::path::{resolve, Segments};

#[derive(Debug, Clone)]
pub struct CredentialSummary {
    pub name: String,
    pub provider: String,
    pub status: Status,
}

#[derive(Debug, Clone)]
pub struct Field {
    pub path: Option<String>,
    pub key: Option<String>,
    pub value: FieldValue,
}

#[derive(Debug, Clone)]
pub enum FieldValue {
    Scalar {
        type_name: &'static str,
        value: JsonValue,
    },
    CredentialRef {
        reference: String,
        credential: CredentialSummary,
    },
    Array(JsonValue),
    Table(Vec<Field>),
}

#[derive(Debug, Clone)]
pub struct EntryView {
    pub profile: String,
    pub name: String,
    pub description: String,
    pub fields: Vec<Field>,
}

#[derive(Debug, Clone)]
pub struct Listing {
    pub profile: String,
    pub profile_description: String,
    pub entries: Vec<EntryView>,
}

#[derive(Debug, Clone)]
pub struct ProfileListing {
    pub name: String,
    pub description: String,
    pub default: bool,
}

#[derive(Debug, Clone)]
pub enum MatchKind {
    Entry { description: String },
    Field(FieldValue),
}

#[derive(Debug, Clone)]
pub struct Match {
    pub profile: String,
    pub path: Option<String>,
    pub key: Option<String>,
    pub kind: MatchKind,
}

pub fn list(config: &Config, profile: &Profile, env: &impl Fn(&str) -> Option<String>) -> Listing {
    let entries = profile
        .entries
        .iter()
        .filter_map(|(name, value)| {
            let table = value.as_table()?;
            Some(entry_view(config, profile, name, table, env))
        })
        .collect();
    Listing {
        profile: profile.name.clone(),
        profile_description: profile.description.clone(),
        entries,
    }
}

pub fn entry(
    config: &Config,
    profile: &Profile,
    name: &str,
    env: &impl Fn(&str) -> Option<String>,
) -> Result<EntryView, AppError> {
    let table = entry_table(profile, name)?;
    Ok(entry_view(config, profile, name, table, env))
}

pub(crate) fn entry_table<'a>(profile: &'a Profile, name: &str) -> Result<&'a Table, AppError> {
    let Some(value) = profile.entries.get(name) else {
        let names = profile
            .entries
            .keys()
            .cloned()
            .collect::<Vec<_>>()
            .join(", ");
        let available = if names.is_empty() {
            "(none defined)"
        } else {
            &names
        };
        return Err(AppError::NotFound(format!(
            "entry '{name}' is not defined in profile '{}'; available entries: {available}; run 'agentenv list' to see the entries",
            profile.name
        )));
    };
    let table = value
        .as_table()
        .expect("validated profile entries are always tables");
    Ok(table)
}

pub fn get<'a>(profile: &'a Profile, path: &Segments) -> Result<&'a Value, AppError> {
    resolve(profile, path)
}

pub fn profiles(config: &Config) -> Vec<ProfileListing> {
    config
        .profiles
        .iter()
        .map(|profile| ProfileListing {
            name: profile.name.clone(),
            description: profile.description.clone(),
            default: config.default_profile.as_deref() == Some(profile.name.as_str()),
        })
        .collect()
}

pub fn credentials(config: &Config, env: &impl Fn(&str) -> Option<String>) -> Vec<CredentialView> {
    config
        .credentials
        .iter()
        .map(|credential| CredentialView {
            summary: credential_summary(credential, env),
            description: credential.description.clone(),
            inject_as: credential.inject_as.clone(),
        })
        .collect()
}

#[derive(Debug, Clone)]
pub struct CredentialView {
    pub summary: CredentialSummary,
    pub description: String,
    pub inject_as: String,
}

pub fn find<'a>(
    config: &Config,
    profiles: impl IntoIterator<Item = &'a Profile>,
    needle: &str,
    env: &impl Fn(&str) -> Option<String>,
) -> Result<Vec<Match>, AppError> {
    if needle.is_empty() {
        return Err(AppError::Usage(
            "find requires a non-empty search string; provide a non-empty search string, or run 'agentenv list'"
                .to_owned(),
        ));
    }
    let needle = needle.to_ascii_lowercase();
    let mut matches = Vec::new();
    for profile in profiles {
        let mut search = MatchSearch {
            config,
            profile: &profile.name,
            needle: &needle,
            env,
            matches: &mut matches,
        };
        for (entry_name, entry_value) in &profile.entries {
            let Some(entry_table) = entry_value.as_table() else {
                continue;
            };
            let description = entry_table
                .get("description")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let entry_path = addressable_path(&[], entry_name);
            if contains(&needle, entry_name) || contains(&needle, description) {
                search.matches.push(Match {
                    profile: profile.name.clone(),
                    path: entry_path.clone(),
                    key: (!is_addressable_key(entry_name)).then(|| entry_name.clone()),
                    kind: MatchKind::Entry {
                        description: description.to_owned(),
                    },
                });
            }
            search.collect(
                entry_name,
                entry_table,
                true,
                true,
                is_addressable_key(entry_name),
            );
        }
    }
    Ok(matches)
}

struct MatchSearch<'a, E> {
    config: &'a Config,
    profile: &'a str,
    needle: &'a str,
    env: &'a E,
    matches: &'a mut Vec<Match>,
}

impl<E: Fn(&str) -> Option<String>> MatchSearch<'_, E> {
    fn collect(
        &mut self,
        prefix: &str,
        table: &Table,
        scan_references: bool,
        entry_level: bool,
        prefix_addressable: bool,
    ) {
        for (key, value) in table {
            if entry_level && key == "description" {
                continue;
            }
            let path = append_path(prefix, key);
            // The reserved entry-level `inject` table can match by its own
            // name, but its members are machinery and never enter the match
            // domain (SPEC-009); `description` keys are never scanned as
            // references at any depth (SPEC-AS-030).
            let in_reserved_inject = entry_level && key == "inject";
            let child_scan_refs = scan_references && !in_reserved_inject && key != "description";
            let field = field_from_value(
                self.config,
                prefix,
                prefix_addressable,
                key,
                value,
                self.env,
                child_scan_refs,
            );
            let matches_name = contains(self.needle, key);
            let matches_value = matches!(
                value,
                Value::String(text) if contains(self.needle, text)
            );
            if matches_name || matches_value {
                self.matches.push(Match {
                    profile: self.profile.to_owned(),
                    path: field.path.clone(),
                    key: field.key.clone(),
                    kind: MatchKind::Field(field.value.clone()),
                });
            }
            if let Value::Table(inner) = value {
                if !in_reserved_inject {
                    self.collect(&path, inner, child_scan_refs, false, field.path.is_some());
                }
            }
        }
    }
}

fn contains(needle: &str, haystack: &str) -> bool {
    haystack.to_ascii_lowercase().contains(needle)
}

fn entry_view(
    config: &Config,
    profile: &Profile,
    name: &str,
    table: &Table,
    env: &impl Fn(&str) -> Option<String>,
) -> EntryView {
    let fields = table
        .iter()
        .filter(|(key, _)| key.as_str() != "description")
        .map(|(key, value)| {
            field_from_value(
                config,
                name,
                is_addressable_key(name),
                key,
                value,
                env,
                key.as_str() != "inject",
            )
        })
        .collect();
    EntryView {
        profile: profile.name.clone(),
        name: name.to_owned(),
        description: table
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        fields,
    }
}

fn field_from_value(
    config: &Config,
    parent_path: &str,
    parent_addressable: bool,
    key: &str,
    value: &Value,
    env: &impl Fn(&str) -> Option<String>,
    scan_reference: bool,
) -> Field {
    let path_text = append_path(parent_path, key);
    let addressable = parent_addressable && is_addressable_key(key);
    let path = addressable.then(|| path_text.clone());
    let field_value = match value {
        Value::String(text) if scan_reference => match recognized_reference(value) {
            Some(Ok(reference)) => {
                let credential = config
                    .credential(&reference.name)
                    .expect("validated credential reference target");
                FieldValue::CredentialRef {
                    reference: text.clone(),
                    credential: credential_summary(credential, env),
                }
            }
            _ => FieldValue::Scalar {
                type_name: toml_type(value),
                value: json_value(value),
            },
        },
        Value::Table(table) => FieldValue::Table(
            table
                .iter()
                .map(|(child_key, child_value)| {
                    field_from_value(
                        config,
                        &path_text,
                        addressable,
                        child_key,
                        child_value,
                        env,
                        // `description` keys are never scanned as references
                        // at any depth (SPEC-AS-030), matching validate's
                        // scanning scope so the credential lookup below can
                        // rely on load-time resolution.
                        scan_reference && child_key != "description",
                    )
                })
                .collect(),
        ),
        Value::Array(_) => FieldValue::Array(json_value(value)),
        _ => FieldValue::Scalar {
            type_name: toml_type(value),
            value: json_value(value),
        },
    };
    Field {
        path,
        key: (!addressable).then(|| key.to_owned()),
        value: field_value,
    }
}

pub fn credential_summary(
    credential: &CredentialDef,
    env: &impl Fn(&str) -> Option<String>,
) -> CredentialSummary {
    CredentialSummary {
        name: credential.name.clone(),
        provider: credential.provider.kind().to_owned(),
        status: shallow_status(credential, env),
    }
}

pub(crate) fn scalar_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Integer(number) => number.to_string(),
        Value::Float(number) if number.is_finite() => finite_float_text(*number),
        Value::Float(number) if number.is_sign_negative() && number.is_infinite() => {
            "-inf".to_owned()
        }
        Value::Float(number) if number.is_infinite() => "inf".to_owned(),
        Value::Float(_) => "nan".to_owned(),
        Value::Boolean(value) => value.to_string(),
        Value::Datetime(value) => value.to_string(),
        _ => unreachable!("scalar_text is called only for scalar TOML values"),
    }
}

fn finite_float_text(number: f64) -> String {
    let text = number.to_string();
    if text.contains('.') || text.contains('e') || text.contains('E') {
        text
    } else {
        format!("{text}.0")
    }
}

pub fn json_value(value: &Value) -> JsonValue {
    match value {
        Value::String(value) => JsonValue::String(value.clone()),
        Value::Integer(value) => JsonValue::Number(Number::from(*value)),
        Value::Float(value) if value.is_finite() => Number::from_f64(*value)
            .map(JsonValue::Number)
            .expect("finite floats have a JSON number representation"),
        Value::Float(_) | Value::Datetime(_) => JsonValue::String(scalar_text(value)),
        Value::Boolean(value) => JsonValue::Bool(*value),
        Value::Array(values) => JsonValue::Array(values.iter().map(json_value).collect()),
        Value::Table(table) => JsonValue::Object(
            table
                .iter()
                .map(|(key, value)| (key.clone(), json_value(value)))
                .collect::<Map<_, _>>(),
        ),
    }
}

pub fn toml_type(value: &Value) -> &'static str {
    match value {
        Value::String(_) => "string",
        Value::Integer(_) => "integer",
        Value::Float(_) => "float",
        Value::Boolean(_) => "boolean",
        Value::Datetime(_) => "datetime",
        Value::Array(_) => "array",
        Value::Table(_) => "table",
    }
}

fn append_path(parent: &str, key: &str) -> String {
    if parent.is_empty() {
        render_key(key)
    } else {
        format!("{parent}.{}", render_key(key))
    }
}

fn addressable_path(parts: &[&str], key: &str) -> Option<String> {
    let mut parts = parts.to_vec();
    parts.push(key);
    if parts.iter().all(|part| is_addressable_key(part)) {
        Some(
            parts
                .into_iter()
                .map(render_key)
                .collect::<Vec<_>>()
                .join("."),
        )
    } else {
        None
    }
}

fn is_addressable_key(key: &str) -> bool {
    !key.is_empty() && !key.contains('"')
}

fn render_key(key: &str) -> String {
    if key
        .chars()
        .any(|character| character == '.' || character.is_whitespace())
    {
        format!("\"{key}\"")
    } else {
        key.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::scalar_text;

    #[test]
    fn scalar_text_keeps_the_decimal_point_of_whole_floats() {
        let value: toml::Value = "1.0".parse().expect("a TOML float parses");
        assert_eq!(scalar_text(&value), "1.0");
    }
}
