//! Config mutation: the format-preserving read–modify–validate–write
//! pipeline behind `set`, `unset`, `init`, and `credential add` (SPEC-001 of
//! change 002).
//!
//! Every mutation follows one path: load and pre-validate the current file,
//! apply the mutation to a `toml_edit` document (comments, blank lines, and
//! key order are preserved; replaced values keep their decor), re-validate
//! the serialized result with [`validate::validate`], and atomically replace
//! the file only when validation passes. A refused mutation leaves the file
//! byte-identical.
//!
//! Diagnostics follow the SPEC-019 boundary: no open-schema profile value
//! and no user-supplied value is echoed; paths, credential names, and
//! `credential://` reference strings may appear.

use std::fs;
use std::path::{Path, PathBuf};

use toml_edit::{DocumentMut, Item, Table, TableLike, Value};

use super::model::{is_valid_credential_name, is_valid_env_name};
use super::validate;
use super::Config;
use crate::error::{AppError, Violation};
use crate::path::Segments;
use crate::project::model::ProjectPin;

/// A parsed `--type` selection with the raw value argument.
#[derive(Debug, Clone)]
pub enum ValueSpec {
    /// Written as a TOML string (the default).
    String(String),
    /// Parsed as a TOML integer.
    Int(String),
    /// Parsed as a TOML float (`inf` and `nan` accepted, mirroring reads).
    Float(String),
    /// Parsed as `true` or `false`.
    Bool(String),
    /// Parsed as JSON and converted (objects become inline tables; `null`
    /// is rejected).
    Json(String),
}

/// One `set` invocation.
#[derive(Debug, Clone)]
pub struct SetRequest {
    /// The explicit `--profile` flag value, when given.
    pub profile_flag: Option<String>,
    /// The raw target path in the segment grammar.
    pub path: String,
    /// The value and its typing.
    pub value: ValueSpec,
    /// `--description`: written to the entry named by the first path segment.
    pub description: Option<String>,
    /// `--create-profile <text>`: create the (absent) `--profile` profile
    /// with this description.
    pub create_profile: Option<String>,
}

/// Provider selection for `credential add`, mirroring the credential schema.
#[derive(Debug, Clone)]
pub enum ProviderSpec {
    /// `provider = "env"` with its `name` field.
    Env { var: String },
    /// `provider = "keychain"` with `service` and `account`.
    Keychain { service: String, account: String },
    /// `provider = "command"` with the ordered `argv`.
    Command { argv: Vec<String> },
}

/// One `credential add` invocation.
#[derive(Debug, Clone)]
pub struct CredentialAddRequest {
    /// The credential name (validated against `[A-Za-z0-9_-]+`).
    pub name: String,
    /// The required description.
    pub description: String,
    /// The provider and its fields.
    pub provider: ProviderSpec,
    /// The required injection target.
    pub inject_as: String,
}

/// Applies a `set` mutation and returns the success message.
pub fn set(
    request: SetRequest,
    env: &impl Fn(&str) -> Option<String>,
    project_pin: Option<&ProjectPin>,
) -> Result<String, AppError> {
    let segments = Segments::parse(&request.path)?;
    let mut loaded = LoadedDocument::load(env)?;
    let profile_name = if request.create_profile.is_some() {
        resolve_write_profile(
            &loaded.config,
            request.profile_flag.as_deref(),
            request.create_profile.as_deref(),
            env,
        )?
    } else {
        let env_profile = super::env_value(env, "AGENTENV_PROFILE");
        loaded
            .config
            .select_profile(
                request.profile_flag.as_deref(),
                env_profile.as_deref(),
                project_pin,
            )?
            .name
            .clone()
    };
    if request.description.is_some() && segments.len() == 1 {
        return Err(AppError::Usage(
            "--description writes an entry's description, so the target path needs at least \
             two segments (<entry>.<field>); to set a profile or entry description directly, \
             target its description path without --description"
                .to_owned(),
        ));
    }
    let value = build_value(&request.value)?;

    let root: &mut dyn TableLike = loaded.document.as_table_mut();
    let profiles = ensure_table(root, "profiles", "profiles")?;
    let profile_path = format!("profiles.{profile_name}");
    let profile = ensure_table(profiles, &profile_name, &profile_path)?;
    if let Some(profile_description) = &request.create_profile {
        profile.insert("description", string_item(profile_description));
    }
    if let Some(entry_description) = &request.description {
        let entry_name = &segments.as_slice()[0];
        let entry_path = format!("{profile_path}.{entry_name}");
        let entry = ensure_table(profile, entry_name, &entry_path)?;
        entry.insert("description", string_item(entry_description));
    }
    set_at_path(profile, &profile_path, &segments, value)?;

    loaded
        .validate_and_persist()
        .map_err(upgrade_sensitive_violations)?;
    Ok(format!(
        "Set 'profiles.{profile_name}.{}'.\n",
        segments.render()
    ))
}

/// AC-002.7: sensitive-name violations produced by a `set` gain the
/// credential-workflow remedy. The scope decision stays entirely with the
/// validator; this only enriches the message it already produced.
fn upgrade_sensitive_violations(error: AppError) -> AppError {
    let AppError::Config(violations) = error else {
        return error;
    };
    AppError::Config(
        violations
            .into_iter()
            .map(|mut violation| {
                if violation
                    .message
                    .contains(validate::PLAINTEXT_SECRET_PHRASE)
                {
                    violation.message.push_str(
                        "; define the credential with 'agentenv credential add <name> ...' and \
                         store its value with 'agentenv credential set <name>'",
                    );
                }
                violation
            })
            .collect(),
    )
}

/// Applies an `unset` mutation and returns the success message.
pub fn unset(
    profile_flag: Option<&str>,
    path: &str,
    env: &impl Fn(&str) -> Option<String>,
    project_pin: Option<&ProjectPin>,
) -> Result<String, AppError> {
    let segments = Segments::parse(path)?;
    let mut loaded = LoadedDocument::load(env)?;
    let profile_name = {
        let env_profile = super::env_value(env, "AGENTENV_PROFILE");
        loaded
            .config
            .select_profile(profile_flag, env_profile.as_deref(), project_pin)?
            .name
            .clone()
    };
    let profile_path = format!("profiles.{profile_name}");
    let root: &mut dyn TableLike = loaded.document.as_table_mut();
    let profile = existing_table_like_mut(root, &["profiles".to_owned(), profile_name.clone()])
        .ok_or_else(|| not_found_path(&profile_path))?;
    remove_at_path(profile, &profile_path, &segments)?;

    loaded.validate_and_persist()?;
    Ok(format!("Removed '{profile_path}.{}'.\n", segments.render()))
}

/// Creates the config file at the resolved path (SPEC-004) and returns the
/// success message.
pub fn init(env: &impl Fn(&str) -> Option<String>) -> Result<String, AppError> {
    let path = super::locate::resolve_path(None, env)?;
    match fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.is_dir() => {
            return Err(config_error(
                path.display().to_string(),
                "the config path is a directory, not a file; point AGENTENV_FILE at a regular \
                 file"
                    .to_owned(),
            ));
        }
        Ok(_) => match fs::canonicalize(&path) {
            Ok(resolved) => {
                return Err(config_error(
                    resolved.display().to_string(),
                    "a config file already exists; edit it with 'agentenv set' or point \
                     AGENTENV_FILE elsewhere"
                        .to_owned(),
                ));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Err(config_error(
                    path.display().to_string(),
                    "the config path is a symlink to a nonexistent target; fix or remove the \
                     symlink and retry 'agentenv init'"
                        .to_owned(),
                ));
            }
            Err(error) => {
                return Err(config_error(
                    path.display().to_string(),
                    format!("cannot resolve the config path: {error}"),
                ));
            }
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(config_error(
                path.display().to_string(),
                format!("cannot inspect the config path: {error}"),
            ));
        }
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            config_error(
                parent.display().to_string(),
                format!("cannot create the config directory: {error}"),
            )
        })?;
    }
    const INIT_CONTENT: &str = "# agentenv configuration.\n\
        # Schema and command reference: see the agentenv README.\n\
        version = 1\n";
    write_new_file(&path, INIT_CONTENT)?;
    Ok(format!(
        "Created configuration file: {}\nAdd a first entry: agentenv set <entry>.<field> \
         <value> --profile <name> --create-profile \"<profile description>\" --description \
         \"<entry description>\"\n",
        path.display()
    ))
}

/// Applies a `credential add` mutation and returns the success message.
pub fn credential_add(
    request: CredentialAddRequest,
    env: &impl Fn(&str) -> Option<String>,
) -> Result<String, AppError> {
    if !is_valid_credential_name(&request.name) {
        return Err(AppError::Usage(format!(
            "credential name '{}' must match [A-Za-z0-9_-]+; pick a name from that character set",
            request.name
        )));
    }
    if !is_valid_env_name(&request.inject_as) {
        return Err(AppError::Usage(format!(
            "--inject-as '{}' is not a valid environment variable name (expected \
             [A-Za-z_][A-Za-z0-9_]*)",
            request.inject_as
        )));
    }
    let mut loaded = LoadedDocument::load(env)?;
    if loaded.config.credential(&request.name).is_some() {
        return Err(AppError::Usage(format!(
            "credential '{}' is already defined; pick another name or edit \
             [credentials.{}] in the config file",
            request.name, request.name
        )));
    }

    let mut definition = Table::new();
    definition.insert("description", string_item(&request.description));
    match &request.provider {
        ProviderSpec::Env { var } => {
            definition.insert("provider", string_item("env"));
            definition.insert("name", string_item(var));
        }
        ProviderSpec::Keychain { service, account } => {
            definition.insert("provider", string_item("keychain"));
            definition.insert("service", string_item(service));
            definition.insert("account", string_item(account));
        }
        ProviderSpec::Command { argv } => {
            definition.insert("provider", string_item("command"));
            let mut array = toml_edit::Array::new();
            for argument in argv {
                array.push(Value::from(argument.as_str()));
            }
            definition.insert("argv", Item::Value(Value::Array(array)));
        }
    }
    definition.insert("inject_as", string_item(&request.inject_as));

    let credentials = ensure_table(loaded.document.as_table_mut(), "credentials", "credentials")?;
    credentials.insert(&request.name, Item::Table(definition));

    let is_keychain = matches!(&request.provider, ProviderSpec::Keychain { .. });
    loaded.validate_and_persist()?;
    let mut message = format!("Credential '{}' added.", request.name);
    if is_keychain {
        message.push_str(&format!(
            " Store its value with 'agentenv credential set {}'.",
            request.name
        ));
    }
    message.push('\n');
    Ok(message)
}

/// A loaded, pre-validated config file ready for one mutation.
struct LoadedDocument {
    /// The fully symlink-resolved file being replaced.
    resolved_path: PathBuf,
    /// Unix permission bits of the existing file.
    #[cfg_attr(not(unix), allow(dead_code))]
    mode: Option<u32>,
    /// The format-preserving document the mutation edits.
    document: DocumentMut,
    /// The typed model of the pre-mutation file (profile and credential
    /// lookups).
    config: Config,
}

impl LoadedDocument {
    /// Locates, symlink-resolves, reads, parses, and pre-validates the
    /// config file (AC-001.5, EDGE-001, EDGE-013).
    fn load(env: &impl Fn(&str) -> Option<String>) -> Result<Self, AppError> {
        let path = super::locate::resolve_path(None, env)?;
        let resolved_path = resolve_existing_target(&path)?;
        let text = fs::read_to_string(&resolved_path).map_err(|error| {
            config_error(
                resolved_path.display().to_string(),
                format!("cannot read the config file: {error}"),
            )
        })?;
        let root = super::parse_config(&resolved_path, &text)?;
        let violations = validate::validate(&root);
        if !violations.is_empty() {
            return Err(AppError::Config(violations));
        }
        let config = Config::from_validated(&root);
        let document = text.parse::<DocumentMut>().map_err(|error| {
            // The text already parsed as TOML above; a divergence between the
            // two parsers is surfaced explicitly, without echoing source.
            config_error(
                resolved_path.display().to_string(),
                format!(
                    "cannot parse the config file for editing{}: {}",
                    error
                        .span()
                        .map(|span| {
                            let (line, column) = super::line_column(&text, span.start);
                            format!(" (line {line}, column {column})")
                        })
                        .unwrap_or_default(),
                    error.message()
                ),
            )
        })?;
        let mode = file_mode(&resolved_path)?;
        Ok(Self {
            resolved_path,
            mode,
            document,
            config,
        })
    }

    /// Serializes the mutated document, re-validates it with the same rule
    /// set as reads, and atomically replaces the file (SPEC-001).
    fn validate_and_persist(self) -> Result<(), AppError> {
        let text = self.document.to_string();
        let root = super::parse_config(&self.resolved_path, &text)?;
        let violations = validate::validate(&root);
        if !violations.is_empty() {
            return Err(AppError::Config(violations));
        }
        atomic_replace(&self.resolved_path, &text, self.mode)
    }
}

/// Resolves the config path for a mutation: the full symlink chain for an
/// existing file, an `agentenv init` hint when the file is missing, and an
/// explicit refusal for a dangling symlink.
fn resolve_existing_target(path: &Path) -> Result<PathBuf, AppError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() => Err(config_error(
            path.display().to_string(),
            "the config path is a directory, not a file; point AGENTENV_FILE at a regular file"
                .to_owned(),
        )),
        Ok(_) => fs::canonicalize(path).map_err(|error| {
            let message = if error.kind() == std::io::ErrorKind::NotFound {
                "the config path is a symlink to a nonexistent target; fix or remove the \
                 symlink"
                    .to_owned()
            } else {
                format!("cannot resolve the config path: {error}")
            };
            config_error(path.display().to_string(), message)
        }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Err(config_error(
            path.display().to_string(),
            "config file not found; run 'agentenv init' to create it or point AGENTENV_FILE \
             at an existing file"
                .to_owned(),
        )),
        Err(error) => Err(config_error(
            path.display().to_string(),
            format!("cannot inspect the config file: {error}"),
        )),
    }
}

/// Resolves the profile name a `set` writes to (AC-002.4 / AC-002.5).
fn resolve_write_profile(
    config: &Config,
    profile_flag: Option<&str>,
    create_profile: Option<&str>,
    env: &impl Fn(&str) -> Option<String>,
) -> Result<String, AppError> {
    if create_profile.is_some() {
        let Some(flag) = profile_flag else {
            return Err(AppError::Usage(
                "--create-profile requires an explicit --profile <name> naming the profile to \
                 create"
                    .to_owned(),
            ));
        };
        if flag.is_empty() {
            return Err(AppError::Usage(
                "--profile requires a profile name; run 'agentenv list --profiles' to see the \
                 defined profiles"
                    .to_owned(),
            ));
        }
        if config.profile(flag).is_some() {
            return Err(AppError::Usage(format!(
                "profile '{flag}' already exists; drop --create-profile to write into it"
            )));
        }
        return Ok(flag.to_owned());
    }
    let env_profile = super::env_value(env, "AGENTENV_PROFILE");
    Ok(config
        .select_profile(profile_flag, env_profile.as_deref(), None)?
        .name
        .clone())
}

/// Builds the TOML value for a `set` from its `--type` selection. Parse
/// failures are usage errors that do not echo the value (SPEC-006).
fn build_value(spec: &ValueSpec) -> Result<Value, AppError> {
    match spec {
        ValueSpec::String(text) => Ok(Value::from(text.as_str())),
        ValueSpec::Int(text) => text.parse::<i64>().map(Value::from).map_err(|_| {
            AppError::Usage(
                "--type int requires an integer value; the given value does not parse".to_owned(),
            )
        }),
        ValueSpec::Float(text) => {
            let keyword = matches!(
                text.as_str(),
                "inf" | "+inf" | "-inf" | "nan" | "+nan" | "-nan"
            );
            match text.parse::<f64>() {
                Ok(value) if keyword || value.is_finite() => Ok(Value::from(value)),
                // A finite literal overflowing to infinity is the float twin
                // of the JSON integer-range refusal.
                Ok(_) => Err(AppError::Usage(
                    "--type float overflows the TOML float range; write 'inf' or '-inf' \
                     explicitly if that is the intent"
                        .to_owned(),
                )),
                Err(_) => Err(AppError::Usage(
                    "--type float requires a floating-point value; the given value does not \
                     parse"
                        .to_owned(),
                )),
            }
        }
        ValueSpec::Bool(text) => match text.as_str() {
            "true" => Ok(Value::from(true)),
            "false" => Ok(Value::from(false)),
            _ => Err(AppError::Usage(
                "--type bool requires 'true' or 'false'".to_owned(),
            )),
        },
        ValueSpec::Json(text) => {
            let json: serde_json::Value = serde_json::from_str(text).map_err(|_| {
                AppError::Usage(
                    "--type json requires a valid JSON value; the given value does not parse"
                        .to_owned(),
                )
            })?;
            json_to_toml(&json)
        }
    }
}

/// Converts a JSON value to a `toml_edit` value: objects become inline
/// tables, `null` is refused (EDGE-004).
fn json_to_toml(json: &serde_json::Value) -> Result<Value, AppError> {
    match json {
        serde_json::Value::Null => Err(AppError::Usage(
            "--type json does not accept null: TOML has no null value".to_owned(),
        )),
        serde_json::Value::Bool(boolean) => Ok(Value::from(*boolean)),
        serde_json::Value::Number(number) => {
            // With arbitrary_precision the original literal is available, so
            // an integer-formed literal either fits i64 or is refused - it
            // never silently becomes a lossy float.
            let literal = number.as_str();
            if literal.contains(['.', 'e', 'E']) {
                number.as_f64().map(Value::from).ok_or_else(|| {
                    AppError::Usage(
                        "--type json number cannot be represented as a TOML float".to_owned(),
                    )
                })
            } else {
                literal.parse::<i64>().map(Value::from).map_err(|_| {
                    AppError::Usage(
                        "--type json integer is out of the TOML integer range".to_owned(),
                    )
                })
            }
        }
        serde_json::Value::String(text) => Ok(Value::from(text.as_str())),
        serde_json::Value::Array(items) => {
            let mut array = toml_edit::Array::new();
            for item in items {
                array.push(json_to_toml(item)?);
            }
            Ok(Value::Array(array))
        }
        serde_json::Value::Object(fields) => {
            let mut table = toml_edit::InlineTable::new();
            for (key, item) in fields {
                table.insert(key, json_to_toml(item)?);
            }
            Ok(Value::InlineTable(table))
        }
    }
}

/// Sets `value` at `segments` below `profile`, creating missing intermediate
/// tables (marked implicit) and carrying decor over replaced values. A leaf
/// that currently holds a structural table is refused: `set` writes exactly
/// one value and never silently deletes a table's contents.
fn set_at_path(
    profile: &mut dyn TableLike,
    profile_path: &str,
    segments: &Segments,
    value: Value,
) -> Result<(), AppError> {
    let parts = segments.as_slice();
    let mut current = profile;
    let mut current_path = profile_path.to_owned();
    for segment in &parts[..parts.len() - 1] {
        current_path = format!("{current_path}.{segment}");
        current = ensure_table(current, segment, &current_path)?;
    }
    let key = &parts[parts.len() - 1];
    match current.get_mut(key) {
        Some(Item::Table(_))
        | Some(Item::ArrayOfTables(_))
        | Some(Item::Value(Value::InlineTable(_))) => {
            return Err(AppError::NotFound(format!(
                "'{current_path}.{key}' is a table, not a single value; set a field inside it, \
                 or remove it first with 'agentenv unset {}'",
                segments.render()
            )));
        }
        Some(Item::Value(existing)) => {
            // AC-001.6: a replaced value keeps its decor (trailing comment,
            // spacing).
            let decor = existing.decor().clone();
            *existing = value;
            *existing.decor_mut() = decor;
        }
        Some(Item::None) | None => {
            current.insert(key, Item::Value(value));
        }
    }
    Ok(())
}

/// Removes the item at `segments` below `profile` (AC-003.1..3).
fn remove_at_path(
    profile: &mut dyn TableLike,
    profile_path: &str,
    segments: &Segments,
) -> Result<(), AppError> {
    let parts = segments.as_slice();
    let mut current = profile;
    let mut current_path = profile_path.to_owned();
    for segment in &parts[..parts.len() - 1] {
        current_path = format!("{current_path}.{segment}");
        current = match current.get_mut(segment) {
            Some(item) => match item.as_table_like_mut() {
                Some(table) => table,
                None => return Err(value_not_table(&current_path)),
            },
            None => return Err(not_found_path(&current_path)),
        };
    }
    let key = &parts[parts.len() - 1];
    if current.remove(key).is_none() {
        return Err(not_found_path(&format!("{current_path}.{key}")));
    }
    Ok(())
}

/// Gets or creates the table at `key` inside `parent`. Traversal accepts
/// standard and inline tables alike (the read path resolves both). A created
/// table is marked implicit so no header materializes unless it gains direct
/// values; an existing non-table value is an exit-3 conflict (AC-002.6).
fn ensure_table<'a>(
    parent: &'a mut dyn TableLike,
    key: &str,
    key_path: &str,
) -> Result<&'a mut dyn TableLike, AppError> {
    if parent.get(key).is_none() {
        let mut table = Table::new();
        table.set_implicit(true);
        // An inline-table parent converts the item to an inline table on
        // insert, so both parent kinds stay internally consistent.
        parent.insert(key, Item::Table(table));
    }
    match parent.get_mut(key).and_then(Item::as_table_like_mut) {
        Some(table) => Ok(table),
        None => Err(value_not_table(key_path)),
    }
}

/// The traversal conflict: a present, non-table item blocks a deeper path.
fn value_not_table(path: &str) -> AppError {
    AppError::NotFound(format!(
        "'{path}' holds a value, not a table; a deeper path cannot be created beneath it; \
         remove the value first with 'agentenv unset'"
    ))
}

/// Descends through existing tables (standard or inline) only, without
/// creating anything.
fn existing_table_like_mut<'a>(
    root: &'a mut dyn TableLike,
    keys: &[String],
) -> Option<&'a mut dyn TableLike> {
    let mut current = root;
    for key in keys {
        current = current.get_mut(key)?.as_table_like_mut()?;
    }
    Some(current)
}

fn not_found_path(path: &str) -> AppError {
    AppError::NotFound(format!(
        "'{path}' does not resolve to an existing table or field; run 'agentenv list' to \
         inspect the active profile"
    ))
}

fn string_item(text: &str) -> Item {
    Item::Value(Value::from(text))
}

fn config_error(path: String, message: String) -> AppError {
    AppError::Config(vec![Violation { path, message }])
}

/// The existing file's permission bits; a metadata failure surfaces instead
/// of silently narrowing the replacement to 0600 (AC-001.3).
fn file_mode(path: &Path) -> Result<Option<u32>, AppError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        fs::metadata(path)
            .map(|metadata| Some(metadata.mode() & 0o777))
            .map_err(|error| {
                config_error(
                    path.display().to_string(),
                    format!("cannot read config-file permissions: {error}"),
                )
            })
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Ok(None)
    }
}

/// Atomically replaces `path` with `content`: a 0600 temporary file in the
/// same directory, content write, permission carry-over, fsync, rename,
/// directory fsync (AC-001.3, AC-001.4).
fn atomic_replace(path: &Path, content: &str, mode: Option<u32>) -> Result<(), AppError> {
    let failure = |message: String| config_error(path.display().to_string(), message);
    let directory = path
        .parent()
        .ok_or_else(|| failure("the config path has no parent directory".to_owned()))?;
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| failure("the config path has no file name".to_owned()))?;
    // PID plus a nanosecond timestamp: unique per attempt, so a leftover from
    // a killed process never blocks future writes.
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    let temp_path = directory.join(format!(
        ".{file_name}.agentenv-tmp-{}-{unique}",
        std::process::id()
    ));

    let result = write_temp_and_rename(path, &temp_path, directory, content, mode);
    if result.is_err() {
        let _ = fs::remove_file(&temp_path);
    }
    result
}

fn write_temp_and_rename(
    path: &Path,
    temp_path: &Path,
    directory: &Path,
    content: &str,
    mode: Option<u32>,
) -> Result<(), AppError> {
    use std::io::Write;

    let failure = |message: String| config_error(path.display().to_string(), message);
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(temp_path).map_err(|error| {
        let hint = if error.kind() == std::io::ErrorKind::AlreadyExists {
            "; a leftover temporary file from an interrupted write may exist - remove it and retry"
        } else {
            ""
        };
        failure(format!(
            "cannot create a temporary file for the write: {error}{hint}"
        ))
    })?;
    file.write_all(content.as_bytes())
        .map_err(|error| failure(format!("cannot write the updated config file: {error}")))?;
    #[cfg(unix)]
    if let Some(mode) = mode {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(fs::Permissions::from_mode(mode))
            .map_err(|error| {
                failure(format!("cannot preserve config-file permissions: {error}"))
            })?;
    }
    #[cfg(not(unix))]
    let _ = mode;
    file.sync_all()
        .map_err(|error| failure(format!("cannot flush the updated config file: {error}")))?;
    drop(file);
    fs::rename(temp_path, path)
        .map_err(|error| failure(format!("cannot replace the config file: {error}")))?;
    // Deliberately best-effort: the renamed file's contents are already
    // durable, and a failed directory fsync can only delay the rename's
    // metadata reaching disk after a crash - it cannot corrupt either file,
    // so it does not fail an otherwise-complete write.
    #[cfg(unix)]
    if let Ok(dir) = fs::File::open(directory) {
        let _ = dir.sync_all();
    }
    #[cfg(not(unix))]
    let _ = directory;
    Ok(())
}

/// Creates a brand-new file with exact 0600 bits on Unix (SPEC-004), fsyncs
/// it and its directory.
fn write_new_file(path: &Path, content: &str) -> Result<(), AppError> {
    use std::io::Write;

    let failure = |message: String| config_error(path.display().to_string(), message);
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(path)
        .map_err(|error| failure(format!("cannot create the config file: {error}")))?;
    #[cfg(unix)]
    {
        // Applied explicitly so the umask can neither widen nor narrow the
        // result (AC-004.1).
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(fs::Permissions::from_mode(0o600))
            .map_err(|error| failure(format!("cannot set config-file permissions: {error}")))?;
    }
    file.write_all(content.as_bytes())
        .map_err(|error| failure(format!("cannot write the config file: {error}")))?;
    file.sync_all()
        .map_err(|error| failure(format!("cannot flush the config file: {error}")))?;
    drop(file);
    #[cfg(unix)]
    if let Some(parent) = path.parent() {
        if let Ok(dir) = fs::File::open(parent) {
            let _ = dir.sync_all();
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;

    use tempfile::TempDir;

    use super::*;

    const COMMENTED_CONFIG: &str = "# top comment\nversion = 1\ndefault_profile = \"work\"\n\n\
        [profiles.work]\ndescription = \"Work env.\"  # profile note\n\n\
        [profiles.work.llm]\ndescription = \"LLM entry.\"\n\
        endpoint = \"https://old.example.com/v1\"  # prod\nmodel = \"m1\"\n";

    fn staged(content: &str) -> (TempDir, PathBuf, impl Fn(&str) -> Option<String>) {
        let dir = TempDir::new().expect("a temp dir");
        let path = dir.path().join("config.toml");
        fs::write(&path, content).expect("the config file is written");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o600))
                .expect("permissions are set");
        }
        let path_text = path.display().to_string();
        let env = move |name: &str| match name {
            "AGENTENV_FILE" => Some(path_text.clone()),
            _ => None,
        };
        (dir, path, env)
    }

    fn set_request(path: &str, value: ValueSpec) -> SetRequest {
        SetRequest {
            profile_flag: None,
            path: path.to_owned(),
            value,
            description: None,
            create_profile: None,
        }
    }

    #[test]
    fn set_preserves_comments_blank_lines_and_decor() {
        // AC-001.1 + AC-001.6.
        let (_dir, path, env) = staged(COMMENTED_CONFIG);
        set(
            set_request(
                "llm.endpoint",
                ValueSpec::String("https://new.example.com/v1".to_owned()),
            ),
            &env,
            None,
        )
        .expect("the set succeeds");
        let after = fs::read_to_string(&path).expect("the file reads");
        assert!(after.contains("# top comment\n"), "{after}");
        assert!(after.contains("description = \"Work env.\"  # profile note\n"));
        assert!(
            after.contains("endpoint = \"https://new.example.com/v1\"  # prod\n"),
            "trailing decor survives the replacement: {after}"
        );
        assert!(after.contains("model = \"m1\"\n"));
    }

    #[test]
    fn refused_mutation_leaves_the_file_byte_identical() {
        // AC-001.2: a new entry without a description fails validation.
        let (_dir, path, env) = staged(COMMENTED_CONFIG);
        let before = fs::read_to_string(&path).expect("the file reads");
        let error = set(
            set_request("newentry.field", ValueSpec::String("x".to_owned())),
            &env,
            None,
        )
        .expect_err("the set is refused");
        match &error {
            AppError::Config(violations) => {
                assert!(
                    violations
                        .iter()
                        .any(|violation| violation.path.contains("newentry.description")),
                    "the violation names the missing description: {violations:?}"
                );
            }
            other => panic!("expected a config error, got {other:?}"),
        }
        let after = fs::read_to_string(&path).expect("the file reads");
        assert_eq!(before, after, "the refused write left the file untouched");
    }

    #[cfg(unix)]
    #[test]
    fn successful_write_preserves_permission_bits() {
        // AC-001.3 + EDGE-010 (0400 is a 0600 subset; 0640 is broader).
        use std::os::unix::fs::PermissionsExt;
        for mode in [0o600u32, 0o640] {
            let (_dir, path, env) = staged(COMMENTED_CONFIG);
            fs::set_permissions(&path, fs::Permissions::from_mode(mode)).expect("chmod works");
            set(
                set_request("llm.model", ValueSpec::String("m2".to_owned())),
                &env,
                None,
            )
            .expect("the set succeeds");
            let bits = fs::metadata(&path).expect("metadata").permissions().mode() & 0o777;
            assert_eq!(bits, mode, "bits survive the replacement");
        }
    }

    #[test]
    fn pre_existing_invalid_file_is_refused_before_mutation() {
        // AC-001.5.
        let (_dir, path, env) = staged("version = 1\n[profiles.work]\n");
        let before = fs::read_to_string(&path).expect("the file reads");
        let error = set(
            set_request("llm.endpoint", ValueSpec::String("x".to_owned())),
            &env,
            None,
        )
        .expect_err("the pre-existing problem is refused");
        assert!(matches!(error, AppError::Config(_)), "{error:?}");
        let after = fs::read_to_string(&path).expect("the file reads");
        assert_eq!(before, after);
    }

    #[test]
    fn missing_file_names_the_init_remedy() {
        // EDGE-001.
        let dir = TempDir::new().expect("a temp dir");
        let path_text = dir.path().join("absent.toml").display().to_string();
        let env = move |name: &str| match name {
            "AGENTENV_FILE" => Some(path_text.clone()),
            _ => None,
        };
        let error = set(
            set_request("llm.endpoint", ValueSpec::String("x".to_owned())),
            &env,
            None,
        )
        .expect_err("the missing file is refused");
        match &error {
            AppError::Config(violations) => {
                assert!(
                    violations[0].message.contains("agentenv init"),
                    "{violations:?}"
                );
            }
            other => panic!("expected a config error, got {other:?}"),
        }
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_config_replaces_the_resolved_target() {
        // EDGE-005.
        let (dir, real_path, _unused_env) = staged(COMMENTED_CONFIG);
        let link_path = dir.path().join("link.toml");
        std::os::unix::fs::symlink(&real_path, &link_path).expect("the symlink is created");
        let link_text = link_path.display().to_string();
        let env = move |name: &str| match name {
            "AGENTENV_FILE" => Some(link_text.clone()),
            _ => None,
        };
        set(
            set_request("llm.model", ValueSpec::String("m3".to_owned())),
            &env,
            None,
        )
        .expect("the set succeeds through the symlink");
        assert!(
            fs::symlink_metadata(&link_path)
                .expect("link metadata")
                .file_type()
                .is_symlink(),
            "the symlink is preserved"
        );
        let target_content = fs::read_to_string(&real_path).expect("the target reads");
        assert!(
            target_content.contains("model = \"m3\""),
            "{target_content}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn dangling_symlink_is_refused() {
        // EDGE-013.
        let dir = TempDir::new().expect("a temp dir");
        let link_path = dir.path().join("link.toml");
        std::os::unix::fs::symlink(dir.path().join("gone.toml"), &link_path)
            .expect("the symlink is created");
        let link_text = link_path.display().to_string();
        let env = move |name: &str| match name {
            "AGENTENV_FILE" => Some(link_text.clone()),
            _ => None,
        };
        let error = set(
            set_request("llm.model", ValueSpec::String("x".to_owned())),
            &env,
            None,
        )
        .expect_err("the dangling symlink is refused");
        match &error {
            AppError::Config(violations) => {
                assert!(violations[0].message.contains("symlink"), "{violations:?}");
            }
            other => panic!("expected a config error, got {other:?}"),
        }
    }

    #[test]
    fn implicit_parents_gain_no_headers() {
        // AC-001.1: profiles/work exist only through dotted headers; a set
        // must not materialize [profiles] or [profiles.work].
        let (_dir, path, env) = staged(COMMENTED_CONFIG);
        set(
            set_request("llm.model", ValueSpec::String("m4".to_owned())),
            &env,
            None,
        )
        .expect("the set succeeds");
        let after = fs::read_to_string(&path).expect("the file reads");
        assert!(!after.contains("[profiles]\n"), "{after}");
        assert!(after.contains("[profiles.work]\n"), "{after}");
    }
}
