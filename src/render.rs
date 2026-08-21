//! Rendering for the read-only query surface.

use serde_json::{json, Map, Value as JsonValue};
use toml::Value;

use crate::query::{
    json_value, scalar_text, CredentialView, EntryView, Field, FieldValue, Listing, Match,
    MatchKind, ProfileListing,
};

pub fn list_text(listing: &Listing) -> String {
    let mut output = format!(
        "Profile: {} — {}\n",
        listing.profile, listing.profile_description
    );
    if listing.entries.is_empty() {
        output.push_str("No entries are defined.\n");
        return output;
    }
    for entry in &listing.entries {
        output.push_str(&format!("\n{} — {}\n", entry.name, entry.description));
        for field in &entry.fields {
            output.push_str(&format!(
                "  {}: {}\n",
                field_label(field),
                type_label(&field.value)
            ));
        }
    }
    output
}

pub fn entry_text(entry: &EntryView, show_values: bool) -> String {
    let mut output = format!("{} — {}\n", entry.name, entry.description);
    let inject_path = format!("{}.inject", entry.name);
    for field in &entry.fields {
        append_entry_text(&mut output, field, show_values, &inject_path);
    }
    output
}

pub fn profiles_text(profiles: &[ProfileListing]) -> String {
    if profiles.is_empty() {
        return "No profiles are defined.\n".to_owned();
    }
    let mut output = String::new();
    for profile in profiles {
        let default = if profile.default { " (default)" } else { "" };
        output.push_str(&format!(
            "{}{} — {}\n",
            profile.name, default, profile.description
        ));
    }
    output
}

pub fn credentials_text(credentials: &[CredentialView]) -> String {
    if credentials.is_empty() {
        return "No credentials are defined.\n".to_owned();
    }
    let mut output = String::new();
    for credential in credentials {
        output.push_str(&format!(
            "{} — {} (provider: {}, inject as: {}, status: {})\n",
            credential.summary.name,
            credential.description,
            credential.summary.provider,
            credential.inject_as,
            credential.summary.status
        ));
    }
    output
}

pub fn get_text(value: &Value) -> Option<String> {
    match value {
        Value::String(_)
        | Value::Integer(_)
        | Value::Float(_)
        | Value::Boolean(_)
        | Value::Datetime(_) => Some(format!("{}\n", scalar_text(value))),
        Value::Array(_) | Value::Table(_) => None,
    }
}

pub fn find_text(matches: &[Match]) -> String {
    let mut output = String::new();
    for item in matches {
        let path = item
            .path
            .as_deref()
            .unwrap_or_else(|| item.key.as_deref().unwrap_or("<unaddressable>"));
        match &item.kind {
            MatchKind::Entry { description } => {
                output.push_str(&format!("{}: {} — {}\n", item.profile, path, description));
            }
            MatchKind::Field(value) => {
                output.push_str(&format!(
                    "{}: {}: {}",
                    item.profile,
                    path,
                    type_label(value)
                ));
                if let Some(display) = find_display_value(value) {
                    output.push_str(&format!(" — {display}"));
                }
                output.push('\n');
            }
        }
    }
    output
}

pub fn list_json(version: i64, listing: &Listing) -> JsonValue {
    json!({
        "version": version,
        "profile": listing.profile,
        "profile_description": listing.profile_description,
        "entries": listing.entries.iter().map(entry_body_json).collect::<Vec<_>>(),
    })
}

pub fn entry_json(version: i64, entry: &EntryView) -> JsonValue {
    json!({
        "version": version,
        "profile": entry.profile,
        "name": entry.name,
        "description": entry.description,
        "fields": entry.fields.iter().map(field_json).collect::<Vec<_>>(),
    })
}

fn entry_body_json(entry: &EntryView) -> JsonValue {
    json!({
        "name": entry.name,
        "description": entry.description,
        "fields": entry.fields.iter().map(field_json).collect::<Vec<_>>(),
    })
}

pub fn profiles_json(version: i64, profiles: &[ProfileListing]) -> JsonValue {
    json!({
        "version": version,
        "profiles": profiles.iter().map(|profile| json!({
            "name": profile.name,
            "description": profile.description,
            "default": profile.default,
        })).collect::<Vec<_>>(),
    })
}

pub fn credentials_json(version: i64, credentials: &[CredentialView]) -> JsonValue {
    json!({
        "version": version,
        "credentials": credentials.iter().map(|credential| json!({
            "name": credential.summary.name,
            "provider": credential.summary.provider,
            "status": credential.summary.status.json_token(),
            "description": credential.description,
            "inject_as": credential.inject_as,
        })).collect::<Vec<_>>(),
    })
}

pub fn find_json(version: i64, matches: &[Match]) -> JsonValue {
    json!({
        "version": version,
        "matches": matches.iter().map(match_json).collect::<Vec<_>>(),
    })
}

pub fn raw_get_json(value: &Value) -> JsonValue {
    json_value(value)
}

fn append_entry_text(output: &mut String, field: &Field, show_values: bool, inject_path: &str) {
    let path = field_label(field);
    match &field.value {
        FieldValue::Table(children) => {
            if show_values && field.path.as_deref() == Some(inject_path) {
                for child in children {
                    if let Some(source) = show_display_value(&child.value) {
                        let target = child
                            .path
                            .as_deref()
                            .and_then(|value| value.rsplit('.').next())
                            .or(child.key.as_deref())
                            .unwrap_or("<unaddressable>");
                        output.push_str(&format!("  {target} ← {source}\n"));
                    }
                }
                return;
            }
            for child in children {
                append_entry_text(output, child, show_values, inject_path);
            }
        }
        value => {
            if show_values {
                let text = show_display_value(value).unwrap_or_default();
                output.push_str(&format!("  {path}: {} — {text}\n", type_label(value)));
            } else {
                output.push_str(&format!("  {path}: {}\n", type_label(value)));
            }
        }
    }
}

fn type_label(value: &FieldValue) -> &'static str {
    match value {
        FieldValue::Scalar { type_name, .. } => type_name,
        FieldValue::CredentialRef { .. } => "credential reference",
        FieldValue::Array(_) => "array",
        FieldValue::Table(_) => "table",
    }
}

fn show_display_value(value: &FieldValue) -> Option<String> {
    match value {
        FieldValue::Scalar { value, .. } | FieldValue::Array(value) => Some(compact_json(value)),
        FieldValue::CredentialRef {
            reference: _,
            credential,
        } => Some(format!("{} ({})", credential.name, credential.status)),
        FieldValue::Table(_) => None,
    }
}

fn find_display_value(value: &FieldValue) -> Option<String> {
    match value {
        FieldValue::Scalar { value, .. } => Some(compact_json(value)),
        FieldValue::CredentialRef {
            reference,
            credential,
        } => Some(format!("{} ({})", reference, credential.status)),
        FieldValue::Array(_) | FieldValue::Table(_) => None,
    }
}

fn compact_json(value: &JsonValue) -> String {
    match value {
        JsonValue::String(value) => value.clone(),
        _ => serde_json::to_string(value).expect("JSON values are serializable"),
    }
}

fn field_label(field: &Field) -> String {
    field
        .path
        .clone()
        .or_else(|| field.key.clone())
        .unwrap_or_else(|| "<unaddressable>".to_owned())
}

fn field_json(field: &Field) -> JsonValue {
    let mut object = Map::new();
    match &field.path {
        Some(path) => {
            object.insert("path".to_owned(), JsonValue::String(path.clone()));
        }
        None => {
            object.insert("path".to_owned(), JsonValue::Null);
            object.insert(
                "key".to_owned(),
                JsonValue::String(field.key.clone().unwrap_or_default()),
            );
            object.insert("addressable".to_owned(), JsonValue::Bool(false));
        }
    }
    match &field.value {
        FieldValue::Scalar { type_name, value } => {
            object.insert(
                "type".to_owned(),
                JsonValue::String((*type_name).to_owned()),
            );
            object.insert("value".to_owned(), value.clone());
        }
        FieldValue::CredentialRef {
            reference,
            credential,
        } => {
            object.insert(
                "type".to_owned(),
                JsonValue::String("credential_ref".to_owned()),
            );
            object.insert("reference".to_owned(), JsonValue::String(reference.clone()));
            object.insert("credential".to_owned(), credential_json(credential));
        }
        FieldValue::Array(value) => {
            object.insert("type".to_owned(), JsonValue::String("array".to_owned()));
            object.insert("value".to_owned(), value.clone());
        }
        FieldValue::Table(fields) => {
            object.insert("type".to_owned(), JsonValue::String("table".to_owned()));
            object.insert(
                "fields".to_owned(),
                JsonValue::Array(fields.iter().map(field_json).collect()),
            );
        }
    }
    JsonValue::Object(object)
}

fn match_json(item: &Match) -> JsonValue {
    let mut object = Map::new();
    object.insert(
        "profile".to_owned(),
        JsonValue::String(item.profile.clone()),
    );
    match &item.path {
        Some(path) => {
            object.insert("path".to_owned(), JsonValue::String(path.clone()));
        }
        None => {
            object.insert("path".to_owned(), JsonValue::Null);
            object.insert(
                "key".to_owned(),
                JsonValue::String(item.key.clone().unwrap_or_default()),
            );
            object.insert("addressable".to_owned(), JsonValue::Bool(false));
        }
    }
    match &item.kind {
        MatchKind::Entry { description } => {
            object.insert("kind".to_owned(), JsonValue::String("entry".to_owned()));
            object.insert(
                "description".to_owned(),
                JsonValue::String(description.clone()),
            );
        }
        MatchKind::Field(value) => {
            object.insert("kind".to_owned(), JsonValue::String("field".to_owned()));
            match value {
                FieldValue::Scalar { type_name, value } => {
                    object.insert(
                        "type".to_owned(),
                        JsonValue::String((*type_name).to_owned()),
                    );
                    object.insert("value".to_owned(), value.clone());
                }
                FieldValue::CredentialRef {
                    reference,
                    credential,
                } => {
                    object.insert(
                        "type".to_owned(),
                        JsonValue::String("credential_ref".to_owned()),
                    );
                    object.insert("reference".to_owned(), JsonValue::String(reference.clone()));
                    object.insert("credential".to_owned(), credential_json(credential));
                }
                FieldValue::Array(_) => {
                    object.insert("type".to_owned(), JsonValue::String("array".to_owned()));
                }
                FieldValue::Table(_) => {
                    object.insert("type".to_owned(), JsonValue::String("table".to_owned()));
                }
            }
        }
    }
    JsonValue::Object(object)
}

fn credential_json(credential: &crate::query::CredentialSummary) -> JsonValue {
    json!({
        "name": credential.name,
        "provider": credential.provider,
        "status": credential.status.json_token(),
    })
}
