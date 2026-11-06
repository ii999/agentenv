//! SPEC-002 core validation, the SPEC-013 `inject` rules, and the SPEC-020
//! sensitive-name traversal, fused with decoding into the typed model.
//!
//! [`decode`] makes one pass over the parsed root table: every rule runs (no
//! short-circuiting) and records its violations in a deterministic traversal
//! order, and each section decoder returns the typed value it checked. A
//! section decoder returns `None` only after recording at least one
//! violation for that section, and [`Config::load`](super::Config::load)
//! receives a [`Config`] only when no violation was recorded at all; it turns
//! a non-empty result into a single exit-2 error. Diagnostics carry config
//! paths and never echo open-schema field values: the sensitive-name rule
//! reports the field path and the `credential://` remedy, never the value,
//! and closed credential-schema metadata is the only config content allowed
//! in messages (SPEC-019).

use std::collections::BTreeMap;
use std::path::PathBuf;

use toml::{Table, Value};

use super::model::{
    check_env_provider, check_inject_as, is_valid_credential_name, is_valid_env_name, Config,
    CredentialDef, CredentialRef, CredentialUsage, Profile, Provider, SshAuth, SshConnection,
    SshTarget, SudoTarget, SudoTransport, UsageListCheck, UsageRuleViolation, REFERENCE_PREFIX,
};
use crate::error::Violation;
use crate::path::Segments;

const ROOT_KEYS: [&str; 4] = ["version", "default_profile", "profiles", "credentials"];
/// Exact sensitive field names (SPEC-020), matched ASCII case-insensitively.
const SENSITIVE_EXACT: [&str; 5] = ["token", "password", "secret", "api_key", "private_key"];
/// Sensitive field-name suffixes (SPEC-020), matched ASCII case-insensitively.
const SENSITIVE_SUFFIX: [&str; 5] = ["_token", "_password", "_secret", "_api_key", "_private_key"];
/// The fixed phrase of a sensitive-name violation. The write path matches on
/// it to append the credential-workflow remedy without re-deriving the rule.
pub(crate) const PLAINTEXT_SECRET_PHRASE: &str = "appears to hold a plaintext secret";

/// Runs all core rules over the parsed root table and decodes it into the
/// typed model. Returns every violation, in a deterministic traversal order,
/// when any rule is broken.
pub(crate) fn decode(root: &Table) -> Result<Config, Vec<Violation>> {
    let mut violations = Vec::new();
    let version = validate_version(root, &mut violations);
    let default_profile = validate_default_profile(root, &mut violations);
    validate_root_keys(root, &mut violations);
    let profiles = validate_profiles(root, &mut violations);
    validate_sensitive_names(root, &mut violations);
    let credentials = validate_credentials(root, &mut violations);
    match (version, default_profile, profiles, credentials) {
        (Some(version), Some(default_profile), Some(profiles), Some(credentials))
            if violations.is_empty() =>
        {
            Ok(Config {
                version,
                default_profile,
                profiles,
                credentials,
            })
        }
        _ => {
            debug_assert!(
                !violations.is_empty(),
                "a config section was rejected without recording a violation"
            );
            Err(violations)
        }
    }
}

/// The violations of a parsed root table, empty when it is valid.
#[cfg(test)]
fn validate(root: &Table) -> Vec<Violation> {
    decode(root).err().unwrap_or_default()
}

fn validate_version(root: &Table, violations: &mut Vec<Violation>) -> Option<i64> {
    // SPEC-002 rules 1 and 11.
    let message = match root.get("version") {
        Some(Value::Integer(1)) => return Some(1),
        Some(Value::Integer(version)) => format!(
            "unsupported version {version}; only version 1 is supported; set 'version = 1' \
             in the config file"
        ),
        Some(_) => "version must be the integer 1; set 'version = 1' in the config file".to_owned(),
        None => "version is required; set 'version = 1' at the top of the config file".to_owned(),
    };
    violations.push(Violation {
        path: "version".to_owned(),
        message,
    });
    None
}

/// Decodes `default_profile`: `Some(None)` when it is absent, `None` when it
/// is rejected.
fn validate_default_profile(
    root: &Table,
    violations: &mut Vec<Violation>,
) -> Option<Option<String>> {
    // SPEC-002 rules 2 and 11.
    let Some(value) = root.get("default_profile") else {
        return Some(None);
    };
    let Some(name) = value.as_str() else {
        violations.push(Violation {
            path: "default_profile".to_owned(),
            message: "default_profile must be a string naming a defined profile".to_owned(),
        });
        return None;
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
        return None;
    }
    Some(Some(name.to_owned()))
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

fn validate_profiles(root: &Table, violations: &mut Vec<Violation>) -> Option<Vec<Profile>> {
    // SPEC-002 rules 3, 9, 11, 5, 7 and the SPEC-013 inject rules.
    let Some(profiles) = root.get("profiles").and_then(Value::as_table) else {
        // Absent is valid; a present value that is not a table is not.
        if root.get("profiles").is_some() {
            violations.push(Violation {
                path: "profiles".to_owned(),
                message: "profiles must be a table; write each profile as a [profiles.<name>] \
                          section"
                    .to_owned(),
            });
            return None;
        }
        return Some(Vec::new());
    };
    let credentials = root.get("credentials").and_then(Value::as_table);
    // Every profile is checked before the results are combined, so one
    // rejected profile never hides a later profile's violations.
    let decoded: Vec<Option<Profile>> = profiles
        .iter()
        .map(|(name, value)| validate_profile(name, value, credentials, violations))
        .collect();
    decoded.into_iter().collect()
}

fn validate_profile(
    name: &str,
    value: &Value,
    credentials: Option<&Table>,
    violations: &mut Vec<Violation>,
) -> Option<Profile> {
    let Some(profile) = value.as_table() else {
        violations.push(Violation {
            path: format!("profiles.{name}"),
            message: format!("profile '{name}' must be a table; write it as [profiles.{name}]"),
        });
        return None;
    };
    let description = require_description(
        profile,
        &format!("profiles.{name}"),
        &format!("profile '{name}'"),
        violations,
    );
    let mut sudo_targets = BTreeMap::new();
    let mut sudo_targets_decoded = true;
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
        let entry_description = require_description(
            entry,
            &entry_path,
            &format!("entry '{entry_name}'"),
            violations,
        );
        let sudo_target = is_sudo_target(entry).then(|| {
            validate_sudo_target(
                &entry_path,
                entry,
                entry_description,
                credentials,
                violations,
            )
        });
        validate_inject_table(name, entry_name, entry, violations);
        validate_entry_references(&entry_path, entry, credentials, violations);
        match sudo_target {
            Some(Some(target)) => {
                sudo_targets.insert(entry_name.clone(), target);
            }
            Some(None) => sudo_targets_decoded = false,
            None => {}
        }
    }
    let description = description?;
    if !sudo_targets_decoded {
        return None;
    }
    let mut entries = profile.clone();
    entries.remove("description");
    Some(Profile {
        name: name.to_owned(),
        description: description.to_owned(),
        entries,
        sudo_targets,
    })
}

fn is_sudo_target(entry: &Table) -> bool {
    entry.get("kind").and_then(Value::as_str) == Some("sudo-target")
}

/// Checks and decodes a `kind = "sudo-target"` entry. `description` is the
/// entry description its caller already checked.
fn validate_sudo_target(
    path: &str,
    entry: &Table,
    description: Option<&str>,
    credentials: Option<&Table>,
    violations: &mut Vec<Violation>,
) -> Option<SudoTarget> {
    for key in entry.keys() {
        if !["description", "kind", "sudo"].contains(&key.as_str()) {
            violation(violations, format!("{path}.{key}"), format!("unknown field '{key}' in sudo target; allowed fields are description, kind, and sudo"));
        }
    }
    let sudo = required_table(entry, "sudo", path, violations)?;
    let transport = required_string(sudo, "transport", &format!("{path}.sudo"), violations);
    let allowed = match transport {
        Some("local") => &[
            "transport",
            "credential",
            "auth_user",
            "run_as",
            "sudo_path",
        ][..],
        Some("ssh") => &[
            "transport",
            "credential",
            "auth_user",
            "run_as",
            "sudo_path",
            "ssh",
        ][..],
        Some(_) => {
            violation(
                violations,
                format!("{path}.sudo.transport"),
                "transport must be 'local' or 'ssh'".to_owned(),
            );
            &[][..]
        }
        None => &[][..],
    };
    close_table(sudo, &format!("{path}.sudo"), allowed, violations);
    let credential = validate_auth_reference(
        sudo.get("credential"),
        &format!("{path}.sudo.credential"),
        credentials,
        "sudo",
        violations,
    );
    let auth_user =
        validate_connection_atom(sudo, "auth_user", &format!("{path}.sudo"), violations);
    let run_as = validate_connection_atom(sudo, "run_as", &format!("{path}.sudo"), violations);
    let sudo_path = validate_absolute_path(
        sudo.get("sudo_path"),
        &format!("{path}.sudo.sudo_path"),
        PathKind::PosixDestination,
        violations,
    );
    let transport = match transport {
        Some("local") => {
            if sudo.contains_key("ssh") {
                violation(
                    violations,
                    format!("{path}.sudo.ssh"),
                    "local transport does not allow an ssh table".to_owned(),
                );
            }
            Some(SudoTransport::Local)
        }
        Some("ssh") => validate_ssh_target(path, sudo, credentials, auth_user, violations)
            .map(SudoTransport::Ssh),
        // A missing or unknown transport was reported above.
        _ => None,
    };
    Some(SudoTarget {
        description: description?.to_owned(),
        credential: credential?,
        auth_user: auth_user?.to_owned(),
        run_as: run_as?.to_owned(),
        sudo_path: sudo_path?,
        transport: transport?,
    })
}

fn validate_ssh_target(
    path: &str,
    sudo: &Table,
    credentials: Option<&Table>,
    auth_user: Option<&str>,
    violations: &mut Vec<Violation>,
) -> Option<SshTarget> {
    let base = format!("{path}.sudo.ssh");
    let ssh = required_table(sudo, "ssh", &format!("{path}.sudo"), violations)?;
    let mode = required_string(ssh, "mode", &base, violations);
    let common = [
        "mode",
        "host_key_alias",
        "known_hosts_file",
        "helper_path",
        "auth",
    ];
    let allowed: Vec<&str> = match mode {
        Some("ssh-config") => common
            .into_iter()
            .chain(["host_alias", "config_file"])
            .collect(),
        Some("explicit") => common
            .into_iter()
            .chain(["hostname", "user", "port"])
            .collect(),
        Some(_) => {
            violation(
                violations,
                format!("{base}.mode"),
                "mode must be 'ssh-config' or 'explicit'".to_owned(),
            );
            Vec::new()
        }
        None => Vec::new(),
    };
    close_table(ssh, &base, &allowed, violations);
    let host_key_alias = validate_connection_atom(ssh, "host_key_alias", &base, violations);
    let known_hosts_file = validate_absolute_path(
        ssh.get("known_hosts_file"),
        &format!("{base}.known_hosts_file"),
        PathKind::ClientNative,
        violations,
    );
    let helper_path = validate_absolute_path(
        ssh.get("helper_path"),
        &format!("{base}.helper_path"),
        PathKind::RemoteHelper,
        violations,
    );
    let connection = match mode {
        Some("ssh-config") => {
            let host_alias = validate_connection_atom(ssh, "host_alias", &base, violations);
            let config_file = match ssh.get("config_file") {
                Some(value) => validate_absolute_path(
                    Some(value),
                    &format!("{base}.config_file"),
                    PathKind::ClientNative,
                    violations,
                )
                .map(Some),
                None => Some(None),
            };
            match (host_alias, config_file) {
                (Some(host_alias), Some(config_file)) => Some(SshConnection::Config {
                    host_alias: host_alias.to_owned(),
                    config_file,
                }),
                _ => None,
            }
        }
        Some("explicit") => {
            let hostname = validate_connection_atom(ssh, "hostname", &base, violations);
            let user = validate_connection_atom(ssh, "user", &base, violations);
            if let (Some(expected), Some(actual)) = (auth_user, user) {
                if expected != actual {
                    violation(
                        violations,
                        format!("{base}.user"),
                        "explicit SSH user must equal sudo.auth_user".to_owned(),
                    );
                }
            }
            let port = match ssh
                .get("port")
                .and_then(Value::as_integer)
                .map(u16::try_from)
            {
                Some(Ok(port)) if port != 0 => Some(port),
                _ => {
                    violation(
                        violations,
                        format!("{base}.port"),
                        "port must be an integer from 1 through 65535".to_owned(),
                    );
                    None
                }
            };
            match (hostname, user, port) {
                (Some(hostname), Some(user), Some(port)) => Some(SshConnection::Explicit {
                    hostname: hostname.to_owned(),
                    user: user.to_owned(),
                    port,
                }),
                _ => None,
            }
        }
        // A missing or unknown mode was reported above.
        _ => None,
    };
    let auth_table = required_table(ssh, "auth", &base, violations)?;
    let auth_base = format!("{base}.auth");
    let auth = match required_string(auth_table, "method", &auth_base, violations) {
        Some("publickey") => {
            let explicit = mode == Some("explicit");
            let allowed = if explicit {
                &["method", "identity_files", "use_agent"][..]
            } else {
                &["method"][..]
            };
            close_table(auth_table, &auth_base, allowed, violations);
            if explicit {
                validate_explicit_publickey(auth_table, &auth_base, violations)
            } else {
                Some(SshAuth::PublicKey {
                    identity_files: Vec::new(),
                    use_agent: false,
                })
            }
        }
        Some("password") => {
            close_table(
                auth_table,
                &auth_base,
                &["method", "credential"],
                violations,
            );
            validate_auth_reference(
                auth_table.get("credential"),
                &format!("{auth_base}.credential"),
                credentials,
                "ssh-password",
                violations,
            )
            .map(|credential| SshAuth::Password { credential })
        }
        Some(_) => {
            violation(
                violations,
                format!("{auth_base}.method"),
                "method must be 'publickey' or 'password'".to_owned(),
            );
            None
        }
        None => None,
    };
    Some(SshTarget {
        host_key_alias: host_key_alias?.to_owned(),
        known_hosts_file: known_hosts_file?,
        helper_path: helper_path?,
        connection: connection?,
        auth: auth?,
    })
}

/// Checks and decodes the publickey authentication of an explicit SSH
/// connection, whose identity files and agent use must be stated.
fn validate_explicit_publickey(
    auth: &Table,
    auth_base: &str,
    violations: &mut Vec<Violation>,
) -> Option<SshAuth> {
    let files = auth.get("identity_files").and_then(Value::as_array);
    let identity_files = if let Some(files) = files {
        // Every path is checked before the results are combined.
        let decoded: Vec<Option<PathBuf>> = files
            .iter()
            .enumerate()
            .map(|(index, value)| {
                validate_absolute_path(
                    Some(value),
                    &format!("{auth_base}.identity_files[{index}]"),
                    PathKind::ClientNative,
                    violations,
                )
            })
            .collect();
        decoded.into_iter().collect::<Option<Vec<_>>>()
    } else {
        violation(
            violations,
            format!("{auth_base}.identity_files"),
            "identity_files must be an array of absolute paths (use an empty array when relying only on the agent)".to_owned(),
        );
        None
    };
    let use_agent = auth.get("use_agent").and_then(Value::as_bool);
    if use_agent.is_none() {
        violation(
            violations,
            format!("{auth_base}.use_agent"),
            "use_agent must be an explicit boolean".to_owned(),
        );
    }
    let has_files = files.is_some_and(|v| !v.is_empty());
    if !has_files && !use_agent.unwrap_or(false) {
        violation(
            violations,
            auth_base.to_owned(),
            "publickey authentication requires identity_files and/or use_agent = true".to_owned(),
        );
    }
    Some(SshAuth::PublicKey {
        identity_files: identity_files?,
        use_agent: use_agent?,
    })
}

/// Checks and decodes an authentication credential reference; `None` when
/// the reference is missing or malformed.
fn validate_auth_reference(
    value: Option<&Value>,
    path: &str,
    credentials: Option<&Table>,
    usage: &str,
    violations: &mut Vec<Violation>,
) -> Option<CredentialRef> {
    let Some(text) = value.and_then(Value::as_str) else {
        violation(
            violations,
            path.to_owned(),
            format!("a {usage} credential reference is required"),
        );
        return None;
    };
    let reference = match CredentialRef::parse(text) {
        Ok(value) => value,
        Err(message) => {
            violation(violations, path.to_owned(), message);
            return None;
        }
    };
    if reference.target_override.is_some() {
        violation(
            violations,
            path.to_owned(),
            "authentication credential references cannot use '?as=' aliases".to_owned(),
        );
    }
    let definition = credentials
        .and_then(|items| items.get(&reference.name))
        .and_then(Value::as_table);
    if definition.is_some_and(|definition| !raw_credential_permits(definition, usage)) {
        violation(
            violations,
            path.to_owned(),
            format!(
                "credential '{}' does not permit {usage} use",
                reference.name
            ),
        );
    }
    Some(reference)
}

fn raw_credential_permits(definition: &Table, usage: &str) -> bool {
    definition
        .get("usages")
        .and_then(Value::as_array)
        .is_some_and(|items| items.iter().any(|item| item.as_str() == Some(usage)))
}

fn required_table<'a>(
    table: &'a Table,
    field: &str,
    base: &str,
    violations: &mut Vec<Violation>,
) -> Option<&'a Table> {
    match table.get(field).and_then(Value::as_table) {
        Some(value) => Some(value),
        None => {
            violation(
                violations,
                format!("{base}.{field}"),
                format!("{field} must be a table"),
            );
            None
        }
    }
}
fn required_string<'a>(
    table: &'a Table,
    field: &str,
    base: &str,
    violations: &mut Vec<Violation>,
) -> Option<&'a str> {
    match table
        .get(field)
        .and_then(Value::as_str)
        .filter(|v| !v.is_empty())
    {
        Some(value) => Some(value),
        None => {
            violation(
                violations,
                format!("{base}.{field}"),
                format!("{field} must be a non-empty string"),
            );
            None
        }
    }
}
fn validate_connection_atom<'a>(
    table: &'a Table,
    field: &str,
    base: &str,
    violations: &mut Vec<Violation>,
) -> Option<&'a str> {
    let value = required_string(table, field, base, violations)?;
    let has_ambiguous_destination_syntax = match field {
        "hostname" | "host_alias" | "host_key_alias" => value.contains(['@', '/', '\\']),
        "auth_user" | "run_as" | "user" => value.contains(['@', ':', '/', '\\']),
        _ => false,
    };
    if value.len() > 255
        || value.starts_with('-')
        || value.chars().any(|c| c.is_control() || c.is_whitespace())
        || has_ambiguous_destination_syntax
    {
        violation(
            violations,
            format!("{base}.{field}"),
            format!("{field} contains unsupported connection metadata"),
        );
    }
    Some(value)
}
#[derive(Clone, Copy)]
enum PathKind {
    /// A file read by the local SSH client. POSIX, drive-letter, and UNC
    /// absolute paths are accepted so the same schema works on every client.
    ClientNative,
    /// An executable on a supported Unix destination.
    PosixDestination,
    /// The one executable path inserted into the remote login-shell command.
    RemoteHelper,
}

/// Checks and decodes an absolute path; `None` when the value is not a
/// non-empty string.
fn validate_absolute_path(
    value: Option<&Value>,
    path: &str,
    kind: PathKind,
    violations: &mut Vec<Violation>,
) -> Option<PathBuf> {
    let Some(text) = value.and_then(Value::as_str).filter(|v| !v.is_empty()) else {
        violation(
            violations,
            path.to_owned(),
            "must be a non-empty absolute path".to_owned(),
        );
        return None;
    };
    let bytes = text.as_bytes();
    let portable_absolute = text.starts_with('/')
        || text.starts_with("\\\\")
        || (bytes.len() >= 3
            && bytes[0].is_ascii_alphabetic()
            && bytes[1] == b':'
            && matches!(bytes[2], b'/' | b'\\'));
    let absolute = match kind {
        PathKind::ClientNative => portable_absolute,
        PathKind::PosixDestination | PathKind::RemoteHelper => text.starts_with('/'),
    };
    let has_control = text.chars().any(char::is_control);
    let helper_safe = !matches!(kind, PathKind::RemoteHelper)
        || (text.is_ascii()
            && !text
                .chars()
                .any(|c| c == '\\' || c.is_whitespace() || "'\"`$;&|<>*?()[]{}!".contains(c))
            && text
                .split('/')
                .all(|segment| segment != "." && segment != ".."));
    if !absolute || text.len() > 4096 || has_control || !helper_safe {
        let message = match kind {
            PathKind::ClientNative => {
                "must be an absolute client path without control characters"
            }
            PathKind::PosixDestination => {
                "must be an absolute POSIX destination path without control characters"
            }
            PathKind::RemoteHelper => "must be an absolute conservative ASCII path without control characters, backslashes, whitespace, shell metacharacters, or dot segments",
        };
        violation(violations, path.to_owned(), message.to_owned());
    }
    Some(PathBuf::from(text))
}
fn close_table(table: &Table, base: &str, allowed: &[&str], violations: &mut Vec<Violation>) {
    for key in table.keys() {
        if !allowed.contains(&key.as_str()) {
            violation(
                violations,
                format!("{base}.{key}"),
                format!("unknown field '{key}' in closed table"),
            );
        }
    }
}
fn violation(violations: &mut Vec<Violation>, path: String, message: String) {
    violations.push(Violation { path, message });
}

/// Checks the reserved `description` (SPEC-002 rule 3) and returns it when
/// it is a non-empty string.
fn require_description<'a>(
    table: &'a Table,
    path: &str,
    owner: &str,
    violations: &mut Vec<Violation>,
) -> Option<&'a str> {
    let message = match table.get("description") {
        Some(Value::String(description)) if !description.is_empty() => {
            return Some(description);
        }
        Some(Value::String(_)) => format!("{owner} requires a non-empty description"),
        Some(_) => format!("{owner} description must be a non-empty string"),
        None => format!("{owner} requires a non-empty string description; add a description field"),
    };
    violations.push(Violation {
        path: format!("{path}.description"),
        message,
    });
    None
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
                     '{source}'; run 'agentenv list {entry}' to see its fields"
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

/// Resolves an entry-relative path without applying query or injection
/// restrictions. Callers can use this for structural checks that accept every
/// TOML value kind.
pub fn resolve_in_entry<'a>(entry_table: &'a Table, segments: &Segments) -> Option<&'a Value> {
    let parts = segments.as_slice();
    let mut current = entry_table.get(parts.first()?)?;
    for segment in &parts[1..] {
        current = current.as_table()?.get(segment)?;
    }
    Some(current)
}

fn validate_entry_references(
    entry_path: &str,
    entry_table: &Table,
    credentials: Option<&Table>,
    violations: &mut Vec<Violation>,
) {
    walk_entry_references(entry_table, &mut |relative_path, reference| {
        let path = format!("{entry_path}.{relative_path}");
        match reference {
            Err(message) => violations.push(Violation {
                path,
                message: message.to_owned(),
            }),
            Ok(reference) => {
                let defined = credentials.is_some_and(|table| table.contains_key(&reference.name));
                if !defined {
                    violations.push(Violation {
                        path,
                        message: format!(
                            "credential '{}' is not defined; run 'agentenv credential add {} \
                             ...' to define it or fix the reference",
                            reference.name, reference.name
                        ),
                    });
                }
            }
        }
    });
}

/// Walks credential references in the validation reference-scanning scope.
///
/// Entry-level description and inject fields are excluded, nested table
/// descriptions are excluded, and array elements are deliberately not
/// traversed.
pub(crate) fn walk_entry_references(
    entry_table: &Table,
    visit: &mut impl FnMut(&str, Result<&CredentialRef, &str>),
) {
    for (key, value) in entry_table {
        if key == "description" || key == "inject" {
            continue;
        }
        walk_value_references(key, value, visit);
    }
}

fn walk_value_references(
    path: &str,
    value: &Value,
    visit: &mut impl FnMut(&str, Result<&CredentialRef, &str>),
) {
    if let Some(reference) = recognized_reference(value) {
        match reference {
            Ok(reference) => visit(path, Ok(&reference)),
            Err(message) => visit(path, Err(&message)),
        }
        return;
    }
    if let Value::Table(table) = value {
        for (key, inner) in table {
            if key == "description" {
                continue;
            }
            walk_value_references(&format!("{path}.{key}"), inner, visit);
        }
    }
}

/// Classifies a value inside the Reference scanning scope.
///
/// Callers exclude description keys, the reserved entry-level `inject`
/// table, and array elements. Query rendering uses this same helper, keeping
/// display classification identical to validation.
pub(crate) fn recognized_reference(value: &Value) -> Option<Result<CredentialRef, String>> {
    match value {
        Value::String(text) if text.starts_with(REFERENCE_PREFIX) => {
            Some(CredentialRef::parse(text))
        }
        _ => None,
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
                            "field '{key}' {PLAINTEXT_SECRET_PHRASE}; store the value \
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
fn validate_credentials(
    root: &Table,
    violations: &mut Vec<Violation>,
) -> Option<Vec<CredentialDef>> {
    // SPEC-002 rule 4 and rule 11.
    let value = match root.get("credentials") {
        None => return Some(Vec::new()),
        Some(value) => value,
    };
    let Some(credentials) = value.as_table() else {
        violations.push(Violation {
            path: "credentials".to_owned(),
            message: "credentials must be a table; write each credential as a \
                      [credentials.<name>] section"
                .to_owned(),
        });
        return None;
    };
    // Every definition is checked before the results are combined.
    let decoded: Vec<Option<CredentialDef>> = credentials
        .iter()
        .map(|(name, definition_value)| {
            let Some(definition) = definition_value.as_table() else {
                violations.push(Violation {
                    path: format!("credentials.{name}"),
                    message: format!(
                        "credential '{name}' must be a table; write it as \
                         [credentials.{name}]"
                    ),
                });
                return None;
            };
            validate_credential(name, definition, violations)
        })
        .collect();
    decoded.into_iter().collect()
}

/// The supported credential providers.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ProviderKind {
    Env,
    Keychain,
    Command,
}

impl ProviderKind {
    fn parse(token: &str) -> Option<Self> {
        match token {
            "env" => Some(Self::Env),
            "keychain" => Some(Self::Keychain),
            "command" => Some(Self::Command),
            _ => None,
        }
    }

    /// The closed field set of a definition with this provider.
    fn allowed_fields(self) -> &'static [&'static str] {
        match self {
            Self::Env => &["description", "inject_as", "usages", "provider", "name"],
            Self::Keychain => &[
                "description",
                "inject_as",
                "usages",
                "provider",
                "service",
                "account",
            ],
            Self::Command => &["description", "inject_as", "usages", "provider", "argv"],
        }
    }
}

fn validate_credential(
    name: &str,
    definition: &Table,
    violations: &mut Vec<Violation>,
) -> Option<CredentialDef> {
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
    let description = require_description(
        definition,
        &path,
        &format!("credential '{name}'"),
        violations,
    );
    let usages = validate_credential_usages(name, definition, &path, violations);
    // A rejected usages value counts as no usage for the dependent rules.
    let usage_list = usages.as_deref().unwrap_or_default();
    let inject_as = match definition.get("inject_as") {
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
            if let Some(rule) = check_inject_as(usage_list, true) {
                violations.push(usage_rule_violation(&path, name, rule));
            }
            Some(Some(inject_as.clone()))
        }
        Some(Value::String(_)) => {
            violations.push(Violation {
                path: format!("{path}.inject_as"),
                message: format!(
                    "credential '{name}' requires a non-empty string inject_as naming the target \
                     environment variable"
                ),
            });
            None
        }
        Some(_) => {
            violations.push(Violation {
                path: format!("{path}.inject_as"),
                message: "inject_as must be a non-empty string naming the target environment \
                          variable"
                    .to_owned(),
            });
            None
        }
        None => {
            if let Some(rule) = check_inject_as(usage_list, false) {
                violations.push(usage_rule_violation(&path, name, rule));
            }
            Some(None)
        }
    };
    let kind = match definition.get("provider") {
        Some(Value::String(provider)) => {
            let kind = ProviderKind::parse(provider);
            if kind.is_none() {
                violations.push(Violation {
                    path: format!("{path}.provider"),
                    message: format!(
                        "provider '{provider}' is not supported; expected one of: env, keychain, \
                         command"
                    ),
                });
            }
            kind
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
    if kind == Some(ProviderKind::Env) {
        if let Some(rule) = check_env_provider(usage_list) {
            violations.push(usage_rule_violation(&path, name, rule));
        }
    }
    let provider =
        kind.and_then(|kind| validate_provider_fields(kind, name, definition, &path, violations));
    // Closed schema (SPEC-AS-021). Skipped when the provider is missing or
    // unrecognized: the allowed field set is undeterminable then, and the
    // provider violation already names the problem.
    if let Some(kind) = kind {
        let allowed_fields = kind.allowed_fields();
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
    Some(CredentialDef {
        name: name.to_owned(),
        description: description?.to_owned(),
        provider: provider?,
        inject_as: inject_as?,
        usages: usages?,
    })
}

/// Checks and decodes the provider-specific fields of a definition.
fn validate_provider_fields(
    kind: ProviderKind,
    name: &str,
    definition: &Table,
    path: &str,
    violations: &mut Vec<Violation>,
) -> Option<Provider> {
    match kind {
        ProviderKind::Env => match definition.get("name") {
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
                Some(Provider::Env {
                    name: variable.clone(),
                })
            }
            _ => {
                violations.push(Violation {
                    path: format!("{path}.name"),
                    message: format!(
                        "env credential '{name}' requires a non-empty string 'name' naming the \
                         environment variable"
                    ),
                });
                None
            }
        },
        ProviderKind::Keychain => {
            let [service, account] =
                ["service", "account"].map(|field| match definition.get(field) {
                    Some(Value::String(value)) if !value.is_empty() => Some(value.clone()),
                    _ => {
                        violations.push(Violation {
                            path: format!("{path}.{field}"),
                            message: format!(
                                "keychain credential '{name}' requires a non-empty string \
                             '{field}'"
                            ),
                        });
                        None
                    }
                });
            Some(Provider::Keychain {
                service: service?,
                account: account?,
            })
        }
        ProviderKind::Command => {
            let message = match definition.get("argv") {
                Some(Value::Array(argv)) if !argv.is_empty() => {
                    let strings: Option<Vec<String>> = argv
                        .iter()
                        .map(|item| item.as_str().map(str::to_owned))
                        .collect();
                    match strings {
                        None => format!(
                            "command credential '{name}' requires argv to be an array of \
                             strings"
                        ),
                        Some(argv) if argv[0].is_empty() => format!(
                            "command credential '{name}' requires a non-empty argv[0] naming \
                             the command to run"
                        ),
                        Some(argv) => return Some(Provider::Command { argv }),
                    }
                }
                _ => format!(
                    "command credential '{name}' requires argv, a non-empty array of strings"
                ),
            };
            violations.push(Violation {
                path: format!("{path}.argv"),
                message,
            });
            None
        }
    }
}

fn validate_credential_usages(
    name: &str,
    definition: &Table,
    path: &str,
    violations: &mut Vec<Violation>,
) -> Option<Vec<CredentialUsage>> {
    let Some(value) = definition.get("usages") else {
        return Some(vec![CredentialUsage::Environment]);
    };
    let Some(array) = value.as_array() else {
        violation(
            violations,
            format!("{path}.usages"),
            "usages must be a non-empty array of purpose strings".to_owned(),
        );
        return None;
    };
    let mut list = UsageListCheck::default();
    for (index, item) in array.iter().enumerate() {
        let Some(token) = item.as_str() else {
            violation(
                violations,
                format!("{path}.usages[{index}]"),
                "usage must be one of: environment, sudo, ssh-password".to_owned(),
            );
            list.skip();
            continue;
        };
        match CredentialUsage::parse(token) {
            None => {
                violation(
                    violations,
                    format!("{path}.usages[{index}]"),
                    "unknown credential usage; expected environment, sudo, or ssh-password"
                        .to_owned(),
                );
                list.skip();
            }
            Some(usage) => {
                if let Err(rule) = list.add(usage) {
                    violations.push(usage_rule_violation(path, name, rule));
                }
            }
        }
    }
    let (usages, rule) = list.finish();
    if let Some(rule) = rule {
        violations.push(usage_rule_violation(path, name, rule));
    }
    Some(usages)
}

/// Phrases a broken usage rule as a violation of the definition at `path`.
fn usage_rule_violation(path: &str, name: &str, rule: UsageRuleViolation) -> Violation {
    let (path, message) = match rule {
        UsageRuleViolation::Empty => (
            format!("{path}.usages"),
            format!("credential '{name}' usages must not be empty"),
        ),
        UsageRuleViolation::Duplicate { index, usage } => (
            format!("{path}.usages[{index}]"),
            format!("duplicate credential usage '{}'", usage.token()),
        ),
        UsageRuleViolation::EnvironmentCombined => (
            format!("{path}.usages"),
            "environment usage cannot be combined with sudo or ssh-password".to_owned(),
        ),
        UsageRuleViolation::InjectAsMissing => (
            format!("{path}.inject_as"),
            format!(
                "credential '{name}' permits environment use and requires a non-empty string inject_as"
            ),
        ),
        UsageRuleViolation::InjectAsForbidden => (
            format!("{path}.inject_as"),
            "inject_as is forbidden for authentication credentials".to_owned(),
        ),
        UsageRuleViolation::EnvProviderAuthentication => (
            format!("{path}.provider"),
            "the env provider cannot be used for sudo or SSH password authentication".to_owned(),
        ),
    };
    Violation { path, message }
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
            "version = 1\n\n[credentials.c]\ndescription = \"d\"\nprovider = \"keychain\"\nservice = \"agentenv\"\naccount = \"personal\"\ninject_as = \"X\"\n",
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
