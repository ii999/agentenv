//! Typed, order-preserving model of a validated configuration file.
//!
//! [`Config`] is produced only by [`Config::load`](super::Config::load),
//! which guarantees the architecture's load invariant: every `Config` handed
//! to a query passed the full SPEC-002 rule set, so downstream modules never
//! re-check structure. Profiles, entries, and credentials keep config-file
//! order (SPEC-021); the open-schema entry data stays in `toml::Table`s.

use toml::{Table, Value};

use crate::error::AppError;

/// The prefix that marks a string value as a credential reference.
pub const REFERENCE_PREFIX: &str = "credential://";

/// A profile: a named group of entries under `[profiles.<name>]`.
#[derive(Debug, Clone)]
pub struct Profile {
    /// The profile's table key in the config file.
    pub name: String,
    /// The profile's reserved `description` value.
    pub description: String,
    /// Entry name to entry table, in config-file order. The profile-level
    /// `description` key is metadata, not an entry, and is excluded here; an
    /// entry's own `description` and `inject` keys stay inside its table so
    /// `get <entry>.description` and `get <entry>` keep working.
    pub entries: Table,
}

/// A credential provider and its closed-schema fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Provider {
    /// `provider = "env"`: read the named environment variable.
    Env { name: String },
    /// `provider = "keychain"`: read the platform credential store by
    /// `service` and `account`.
    Keychain { service: String, account: String },
    /// `provider = "command"`: run `argv` directly (no shell).
    Command { argv: Vec<String> },
}

impl Provider {
    /// The provider type token used in text and JSON output.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Env { .. } => "env",
            Self::Keychain { .. } => "keychain",
            Self::Command { .. } => "command",
        }
    }
}

/// A credential definition from the top-level `credentials` table.
#[derive(Debug, Clone)]
pub struct CredentialDef {
    /// The definition's table key in the config file.
    pub name: String,
    /// The credential's reserved `description` value.
    pub description: String,
    /// The provider and its fields.
    pub provider: Provider,
    /// The environment variable this credential injects by default.
    pub inject_as: String,
}

/// A parsed `credential://<name>[?as=<ENV>]` reference (SPEC-012 grammar).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CredentialRef {
    /// The referenced credential's name.
    pub name: String,
    /// The `?as=<ENV>` target override, when present.
    pub target_override: Option<String>,
}

impl CredentialRef {
    /// Parses the strict reference grammar: exactly `credential://<name>` or
    /// `credential://<name>?as=<ENV>`, where `<name>` matches
    /// `[A-Za-z0-9_-]+` and `<ENV>` is a valid environment variable name.
    /// Anything else - empty names, unknown or duplicated query parameters,
    /// wrong-case `?As=`, trailing garbage, or a string without the prefix -
    /// is an error; a malformed reference is never treated as ordinary data.
    pub fn parse(text: &str) -> Result<Self, String> {
        let Some(rest) = text.strip_prefix(REFERENCE_PREFIX) else {
            return Err(format!(
                "not a credential reference: expected '{REFERENCE_PREFIX}<name>'"
            ));
        };
        let (name, query) = match rest.split_once('?') {
            Some((name, query)) => (name, Some(query)),
            None => (rest, None),
        };
        if !is_valid_credential_name(name) {
            return Err(format!(
                "invalid credential reference '{text}': the credential name must match [A-Za-z0-9_-]+"
            ));
        }
        let Some(query) = query else {
            return Ok(Self {
                name: name.to_owned(),
                target_override: None,
            });
        };
        let Some(target) = query.strip_prefix("as=") else {
            return Err(format!(
                "invalid credential reference '{text}': only the '?as=<ENV>' query parameter is allowed"
            ));
        };
        if !is_valid_env_name(target) {
            return Err(format!(
                "invalid credential reference '{text}': '?as=' needs a valid environment variable name ([A-Za-z_][A-Za-z0-9_]*)"
            ));
        }
        Ok(Self {
            name: name.to_owned(),
            target_override: Some(target.to_owned()),
        })
    }
}

/// A fully validated configuration: the typed view of one config file.
#[derive(Debug, Clone)]
pub struct Config {
    /// The config file's `version` (only `1` is supported).
    pub version: i64,
    /// The `default_profile` value, when present.
    pub default_profile: Option<String>,
    /// Profiles in config-file order (SPEC-021).
    pub profiles: Vec<Profile>,
    /// Credential definitions in config-file order; name lookup goes through
    /// [`Config::credential`].
    pub credentials: Vec<CredentialDef>,
}

impl Config {
    /// The profile named `name`, if defined.
    pub fn profile(&self, name: &str) -> Option<&Profile> {
        self.profiles.iter().find(|profile| profile.name == name)
    }

    /// The credential named `name`, if defined.
    pub fn credential(&self, name: &str) -> Option<&CredentialDef> {
        self.credentials
            .iter()
            .find(|credential| credential.name == name)
    }

    /// Resolves the active profile (SPEC-004): the `--profile` flag, else
    /// the `AGENTENV_PROFILE` environment value, else the file's
    /// `default_profile`. An empty flag value is a usage error; an empty
    /// environment value counts as unset (SPEC-AS-028), so callers may pass
    /// raw environment values.
    pub fn select_profile(
        &self,
        flag: Option<&str>,
        env_val: Option<&str>,
    ) -> Result<&Profile, AppError> {
        if let Some(flag) = flag {
            if flag.is_empty() {
                return Err(AppError::Usage(
                    "--profile requires a profile name; run 'agentenv list --profiles' to see the defined profiles"
                        .to_owned(),
                ));
            }
            return self.resolve_profile_name(flag);
        }
        if let Some(env_val) = env_val.filter(|value| !value.is_empty()) {
            return self.resolve_profile_name(env_val);
        }
        match &self.default_profile {
            Some(name) => self.resolve_profile_name(name),
            None => Err(no_selection_error(&self.profiles)),
        }
    }

    fn resolve_profile_name(&self, name: &str) -> Result<&Profile, AppError> {
        if let Some(profile) = self.profile(name) {
            return Ok(profile);
        }
        let names: Vec<&str> = self.profiles.iter().map(|p| p.name.as_str()).collect();
        if names.is_empty() {
            Err(AppError::NotFound(format!(
                "profile '{name}' is not defined; the config file defines no profiles; add a [profiles.<name>] table or run 'agentenv list --profiles'"
            )))
        } else {
            Err(AppError::NotFound(format!(
                "profile '{name}' is not defined; available profiles: {}; run 'agentenv list --profiles'",
                names.join(", ")
            )))
        }
    }

    /// Builds the typed model from a table that passed validation. Every
    /// `expect` below encodes a validation invariant; a panic here means
    /// `validate` and this builder disagree, which is a bug, not a config
    /// error.
    pub(crate) fn from_validated(root: &Table) -> Self {
        let version = root
            .get("version")
            .and_then(Value::as_integer)
            .expect("validated config has an integer version");
        let default_profile = root
            .get("default_profile")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let profiles = root
            .get("profiles")
            .and_then(Value::as_table)
            .map(|profiles| {
                profiles
                    .iter()
                    .map(|(name, value)| profile_from_validated(name, value))
                    .collect()
            })
            .unwrap_or_default();
        let credentials = root
            .get("credentials")
            .and_then(Value::as_table)
            .map(|credentials| {
                credentials
                    .iter()
                    .map(|(name, value)| credential_from_validated(name, value))
                    .collect()
            })
            .unwrap_or_default();
        Self {
            version,
            default_profile,
            profiles,
            credentials,
        }
    }
}

fn profile_from_validated(name: &str, value: &Value) -> Profile {
    let table = value.as_table().expect("validated profile is a table");
    let description = table
        .get("description")
        .and_then(Value::as_str)
        .expect("validated profile has a string description");
    let mut entries = table.clone();
    entries.remove("description");
    Profile {
        name: name.to_owned(),
        description: description.to_owned(),
        entries,
    }
}

fn credential_from_validated(name: &str, value: &Value) -> CredentialDef {
    let table = value.as_table().expect("validated credential is a table");
    let description = table
        .get("description")
        .and_then(Value::as_str)
        .expect("validated credential has a string description");
    let inject_as = table
        .get("inject_as")
        .and_then(Value::as_str)
        .expect("validated credential has a string inject_as");
    let provider = match table
        .get("provider")
        .and_then(Value::as_str)
        .expect("validated credential has a string provider")
    {
        "env" => Provider::Env {
            name: table
                .get("name")
                .and_then(Value::as_str)
                .expect("validated env credential has a string name")
                .to_owned(),
        },
        "keychain" => Provider::Keychain {
            service: table
                .get("service")
                .and_then(Value::as_str)
                .expect("validated keychain credential has a string service")
                .to_owned(),
            account: table
                .get("account")
                .and_then(Value::as_str)
                .expect("validated keychain credential has a string account")
                .to_owned(),
        },
        "command" => Provider::Command {
            argv: table
                .get("argv")
                .and_then(Value::as_array)
                .expect("validated command credential has an argv array")
                .iter()
                .map(|item| {
                    item.as_str()
                        .expect("validated argv entry is a string")
                        .to_owned()
                })
                .collect(),
        },
        other => unreachable!("validated provider is env, keychain, or command; got {other:?}"),
    };
    CredentialDef {
        name: name.to_owned(),
        description: description.to_owned(),
        provider,
        inject_as: inject_as.to_owned(),
    }
}

fn no_selection_error(profiles: &[Profile]) -> AppError {
    let names: Vec<&str> = profiles.iter().map(|p| p.name.as_str()).collect();
    if names.is_empty() {
        AppError::NotFound(
            "no profile is selected and the config file defines no profiles; add a [profiles.<name>] \
             table with a default_profile, or run 'agentenv list --profiles'"
                .to_owned(),
        )
    } else {
        AppError::NotFound(format!(
            "no profile is selected; available profiles: {}; set --profile or \
             AGENTENV_PROFILE, set default_profile in the config file, or run \
             'agentenv list --profiles'",
            names.join(", ")
        ))
    }
}

/// Valid environment variable name: `[A-Za-z_][A-Za-z0-9_]*`
/// (SPEC-AS-005, POSIX portable set).
pub(crate) fn is_valid_env_name(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(first) if first.is_ascii_alphabetic() || first == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Valid credential name (definition key or reference name):
/// `[A-Za-z0-9_-]+`.
pub(crate) fn is_valid_credential_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reference_grammar_accepts_exact_forms() {
        assert_eq!(
            CredentialRef::parse("credential://company_llm"),
            Ok(CredentialRef {
                name: "company_llm".to_owned(),
                target_override: None,
            })
        );
        assert_eq!(
            CredentialRef::parse("credential://gh?as=GITHUB_TOKEN"),
            Ok(CredentialRef {
                name: "gh".to_owned(),
                target_override: Some("GITHUB_TOKEN".to_owned()),
            })
        );
        assert!(CredentialRef::parse("credential://a-b_C9").is_ok());
    }

    #[test]
    fn reference_grammar_rejects_malformed_forms() {
        // AC-012.4 cases.
        for text in [
            "credential://company_llm?As=X",
            "credential://",
            "credential://name?x=1",
            "credential://name trailing",
            "credential://name?as=",
            "credential://name?as=1BAD",
            "credential://name?as=X&y=2",
            "credential://name?as=X?as=Y",
            "credential://name?",
            "credential://my cred",
            "endpoint",
            "",
        ] {
            assert!(
                CredentialRef::parse(text).is_err(),
                "expected '{text}' to be rejected"
            );
        }
    }

    #[test]
    fn env_name_grammar() {
        assert!(is_valid_env_name("A"));
        assert!(is_valid_env_name("_x9"));
        assert!(is_valid_env_name("OPENAI_API_KEY"));
        for name in ["", "1BAD", "9", "has-dash", "has space", "日本"] {
            assert!(!is_valid_env_name(name), "expected {name:?} to be invalid");
        }
    }

    #[test]
    fn credential_name_grammar() {
        assert!(is_valid_credential_name("a"));
        assert!(is_valid_credential_name("A-b_C9"));
        for name in ["", "my cred", "a/b", "日本"] {
            assert!(
                !is_valid_credential_name(name),
                "expected {name:?} to be invalid"
            );
        }
    }

    fn config_with_profiles(names: &[&str], default: Option<&str>) -> Config {
        Config {
            version: 1,
            default_profile: default.map(str::to_owned),
            profiles: names
                .iter()
                .map(|name| Profile {
                    name: (*name).to_owned(),
                    description: format!("the {name} profile"),
                    entries: Table::new(),
                })
                .collect(),
            credentials: Vec::new(),
        }
    }

    #[test]
    fn select_profile_flag_beats_env_and_default() {
        // AC-004.1 / AC-004.2 (logic level).
        let config = config_with_profiles(&["work", "personal"], Some("work"));
        let selected = config
            .select_profile(Some("work"), Some("personal"))
            .expect("the flag must win");
        assert_eq!(selected.name, "work");
    }

    #[test]
    fn select_profile_env_beats_default() {
        // AC-004.1: AGENTENV_PROFILE beats default_profile.
        let config = config_with_profiles(&["work", "personal"], Some("work"));
        let selected = config
            .select_profile(None, Some("personal"))
            .expect("the environment value must beat default_profile");
        assert_eq!(selected.name, "personal");
    }

    #[test]
    fn select_profile_falls_back_to_default() {
        let config = config_with_profiles(&["work", "personal"], Some("work"));
        let selected = config
            .select_profile(None, None)
            .expect("default_profile must be used");
        assert_eq!(selected.name, "work");
    }

    #[test]
    fn select_profile_empty_env_counts_as_unset() {
        // SPEC-AS-028.
        let config = config_with_profiles(&["work"], Some("work"));
        let selected = config
            .select_profile(None, Some(""))
            .expect("an empty environment value is unset");
        assert_eq!(selected.name, "work");
    }

    #[test]
    fn select_profile_empty_flag_is_a_usage_error() {
        // SPEC-001: `--profile ""` is a usage error (exit 1).
        let config = config_with_profiles(&["work"], Some("work"));
        match config.select_profile(Some(""), None) {
            Err(AppError::Usage(message)) => {
                assert!(message.contains("--profile"), "{message}");
            }
            other => panic!("expected a usage error, got {other:?}"),
        }
    }

    #[test]
    fn select_profile_unknown_name_lists_profiles() {
        // AC-004.3 (logic level).
        let config = config_with_profiles(&["work", "personal"], Some("work"));
        match config.select_profile(None, Some("nope")) {
            Err(AppError::NotFound(message)) => {
                assert!(message.contains("nope"), "{message}");
                assert!(message.contains("work"), "{message}");
                assert!(message.contains("personal"), "{message}");
                assert!(message.contains("list --profiles"), "{message}");
            }
            other => panic!("expected a not-found error, got {other:?}"),
        }
    }

    #[test]
    fn select_profile_without_any_selection_is_not_found() {
        let config = config_with_profiles(&["work", "personal"], None);
        match config.select_profile(None, None) {
            Err(AppError::NotFound(message)) => {
                assert!(message.contains("work"), "{message}");
                assert!(message.contains("no profile is selected"), "{message}");
            }
            other => panic!("expected a not-found error, got {other:?}"),
        }
    }

    #[test]
    fn select_profile_with_zero_profiles() {
        let config = config_with_profiles(&[], None);
        let error = config
            .select_profile(None, Some("anything"))
            .expect_err("nothing can be selected");
        assert!(matches!(error, AppError::NotFound(_)));
    }

    #[test]
    fn from_validated_preserves_order_and_extracts_descriptions() {
        let root: Table = r#"
version = 1
default_profile = "work"

[profiles.zeta]
description = "the zeta profile"

[profiles.zeta.first]
description = "first entry"
field = 1

[profiles.alpha]
description = "the alpha profile"

[credentials.c2]
description = "second credential"
provider = "env"
name = "C2_TOKEN"
inject_as = "C2"

[credentials.c1]
description = "first credential"
provider = "command"
argv = ["op", "read", "x"]
inject_as = "C1"
"#
        .parse()
        .expect("the fixture parses");
        let config = Config::from_validated(&root);
        assert_eq!(config.version, 1);
        assert_eq!(config.default_profile.as_deref(), Some("work"));
        assert_eq!(
            config
                .profiles
                .iter()
                .map(|p| p.name.as_str())
                .collect::<Vec<_>>(),
            ["zeta", "alpha"],
            "profiles keep config-file order"
        );
        assert_eq!(
            config
                .credentials
                .iter()
                .map(|c| c.name.as_str())
                .collect::<Vec<_>>(),
            ["c2", "c1"],
            "credentials keep config-file order"
        );
        let zeta = config.profile("zeta").expect("zeta exists");
        assert_eq!(zeta.description, "the zeta profile");
        assert_eq!(
            zeta.entries.keys().collect::<Vec<_>>(),
            ["first"],
            "the profile-level description is not an entry"
        );
        let first = zeta.entries.get("first").expect("the entry survives");
        assert_eq!(
            first
                .as_table()
                .expect("entries are tables")
                .get("description")
                .and_then(Value::as_str),
            Some("first entry"),
            "an entry's own description stays inside its table"
        );
        assert_eq!(
            config.credential("c2").expect("c2 exists").provider,
            Provider::Env {
                name: "C2_TOKEN".to_owned()
            }
        );
        assert_eq!(
            config.credential("c1").expect("c1 exists").provider,
            Provider::Command {
                argv: vec!["op".to_owned(), "read".to_owned(), "x".to_owned()]
            }
        );
        assert_eq!(config.credential("c1").expect("c2 exists").inject_as, "C1");
    }
}
