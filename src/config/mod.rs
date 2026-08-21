//! Config loading: locate, read, parse, validate, and build the typed model.
//!
//! [`Config::load`] is the single entry point. It aggregates every SPEC-002
//! violation into one [`AppError::Config`](crate::error::AppError::Config)
//! (exit 2) so no command ever operates on a partially valid file, and it
//! applies the empty-value rule of SPEC-AS-028 itself: callers may pass raw
//! environment values. Parse diagnostics carry the line and column position
//! but never the offending source line (SPEC-002 diagnostics rule;
//! `toml::de::Error`'s `Display` renders the source line with a caret and is
//! never forwarded).

pub(crate) mod locate;
pub mod model;
pub(crate) mod validate;

pub use model::{Config, CredentialDef, CredentialRef, Profile, Provider, REFERENCE_PREFIX};

use std::path::Path;

use toml::de::Error as TomlError;
use toml::Table;

use crate::error::{AppError, Violation};

/// Reads an environment value, treating the empty string as unset
/// (SPEC-AS-028).
pub(crate) fn env_value(env: &impl Fn(&str) -> Option<String>, name: &str) -> Option<String> {
    match env(name) {
        Some(value) if !value.is_empty() => Some(value),
        _ => None,
    }
}

impl Config {
    /// Locates, reads, parses, and validates the config file.
    ///
    /// `explicit_file`, when given, wins over every environment-based rule.
    /// `env` supplies environment values; empty values count as unset. Every
    /// failure is an exit-2 [`AppError::Config`]: an undeterminable base
    /// directory, a missing or unreadable file, a parse error, or the
    /// aggregated SPEC-002 violations.
    pub fn load(
        explicit_file: Option<&Path>,
        env: &impl Fn(&str) -> Option<String>,
    ) -> Result<Config, AppError> {
        let path = locate::resolve_path(explicit_file, env)?;
        let text = read_config(&path)?;
        let root = parse_config(&path, &text)?;
        let violations = validate::validate(&root);
        if violations.is_empty() {
            Ok(Config::from_validated(&root))
        } else {
            Err(AppError::Config(violations))
        }
    }
}

/// Reads the config file, mapping every failure to an exit-2 error whose
/// violation names the path (the file itself is the config path here) and
/// the failure.
fn read_config(path: &Path) -> Result<String, AppError> {
    let failure = |message: String| {
        AppError::Config(vec![Violation {
            path: path.display().to_string(),
            message,
        }])
    };
    match std::fs::metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Err(failure(
            "config file not found; create it or point AGENT_CONTEXT_FILE at an existing file"
                .to_owned(),
        )),
        Err(error) => Err(failure(format!("cannot inspect the config file: {error}"))),
        Ok(metadata) if metadata.is_dir() => Err(failure(
            "the config path is a directory, not a file; point AGENT_CONTEXT_FILE at a regular \
             file"
                .to_owned(),
        )),
        Ok(_) => std::fs::read_to_string(path)
            .map_err(|error| failure(format!("cannot read the config file: {error}"))),
    }
}

/// Parses TOML, reporting the error position without echoing the source
/// line or any field value.
fn parse_config(path: &Path, text: &str) -> Result<Table, AppError> {
    text.parse::<Table>().map_err(|error: TomlError| {
        let message = match error.span() {
            Some(span) => {
                let (line, column) = line_column(text, span.start);
                format!(
                    "invalid TOML at line {line}, column {column}: {}",
                    error.message()
                )
            }
            None => format!("invalid TOML: {}", error.message()),
        };
        AppError::Config(vec![Violation {
            path: path.display().to_string(),
            message,
        }])
    })
}

/// One-based line and column of `offset` within `text`.
fn line_column(text: &str, offset: usize) -> (usize, usize) {
    let mut line = 1;
    let mut line_start = 0;
    for (index, char) in text.char_indices() {
        if index >= offset {
            break;
        }
        if char == '\n' {
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

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;

    use tempfile::TempDir;

    use super::Config;
    use crate::error::AppError;

    const VALID_CONFIG: &str = r#"
version = 1
default_profile = "work"

[profiles.work]
description = "Day-to-day development environment."

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

    fn no_env(_: &str) -> Option<String> {
        None
    }

    fn staged_file(name: &str, content: &str) -> (TempDir, PathBuf) {
        let dir = TempDir::new().expect("a temp dir");
        let path = dir.path().join(name);
        fs::write(&path, content).expect("the config file is written");
        (dir, path)
    }

    #[test]
    fn load_builds_the_model_from_a_valid_file() {
        let (_dir, path) = staged_file("context.toml", VALID_CONFIG);
        let config = Config::load(Some(&path), &no_env).expect("the valid config loads");
        assert_eq!(config.version, 1);
        assert_eq!(config.default_profile.as_deref(), Some("work"));
        assert_eq!(
            config
                .profiles
                .iter()
                .map(|p| p.name.as_str())
                .collect::<Vec<_>>(),
            ["work"]
        );
        let credential = config
            .credential("company_llm")
            .expect("the credential loads");
        assert_eq!(credential.inject_as, "OPENAI_API_KEY");
        assert_eq!(credential.provider.kind(), "env");
    }

    #[test]
    fn load_reads_the_file_via_the_environment() {
        // AC-001.1 (logic level): AGENT_CONTEXT_FILE is honored.
        let (_dir, path) = staged_file("context.toml", VALID_CONFIG);
        let path_text = path.display().to_string();
        let env = move |name: &str| match name {
            "AGENT_CONTEXT_FILE" => Some(path_text.clone()),
            _ => None,
        };
        let config = Config::load(None, &env).expect("the env-specified file loads");
        assert_eq!(config.profiles.len(), 1);
    }

    #[test]
    fn load_treats_an_empty_environment_value_as_unset() {
        // SPEC-AS-028: with no HOME and an empty AGENT_CONTEXT_FILE, the
        // base directory cannot be determined.
        let env = |name: &str| match name {
            "AGENT_CONTEXT_FILE" => Some(String::new()),
            _ => None,
        };
        match Config::load(None, &env) {
            Err(AppError::Config(violations)) => {
                assert_eq!(violations[0].path, "HOME");
            }
            other => panic!("expected a config error, got {other:?}"),
        }
    }

    #[test]
    fn load_missing_file_names_the_path() {
        // AC-001.2 (logic level).
        let dir = TempDir::new().expect("a temp dir");
        let path = dir.path().join("absent.toml");
        match Config::load(Some(&path), &no_env) {
            Err(AppError::Config(violations)) => {
                assert_eq!(violations.len(), 1);
                let violation = &violations[0];
                assert!(
                    violation.path.contains("absent.toml"),
                    "the violation names the resolved path: {violation}"
                );
                assert!(violation.message.contains("not found"), "{violation}");
            }
            other => panic!("expected a config error, got {other:?}"),
        }
    }

    #[test]
    fn load_directory_instead_of_file() {
        // EDGE-002 (logic level).
        let dir = TempDir::new().expect("a temp dir");
        match Config::load(Some(dir.path()), &no_env) {
            Err(AppError::Config(violations)) => {
                assert_eq!(violations.len(), 1);
                assert_eq!(violations[0].path, dir.path().display().to_string());
                assert!(
                    violations[0].message.contains("directory"),
                    "{}",
                    violations[0]
                );
            }
            other => panic!("expected a config error, got {other:?}"),
        }
    }

    #[test]
    fn load_parse_error_reports_position_without_source_content() {
        // AC-019.3 (logic level): line/column without echoing the value.
        let sentinel = "sk-sentinel-zz";
        let text = format!(
            "version = 1\n\n[profiles.work]\ndescription = \"d\"\n\n[profiles.work.llm]\ndescription = \"d\"\napi_key = {sentinel}\n"
        );
        let (_dir, path) = staged_file("context.toml", &text);
        match Config::load(Some(&path), &no_env) {
            Err(AppError::Config(violations)) => {
                assert_eq!(violations.len(), 1);
                let violation = &violations[0];
                assert!(violation.message.contains("line 8"), "{violation}");
                assert!(
                    violation.message.to_ascii_lowercase().contains("line"),
                    "{violation}"
                );
                assert!(
                    !violation.message.contains(sentinel),
                    "the parse diagnostic must not echo the sentinel: {violation}"
                );
                assert!(!violation.message.contains("api_key"), "{violation}");
            }
            other => panic!("expected a config error, got {other:?}"),
        }
    }

    #[test]
    fn load_reports_every_validation_violation() {
        // AC-002.4 (logic level): three independent violations, all present.
        let text = r#"
version = 1
default_profile = "work"

[profiles.work]
description = "d"

[profiles.work.llm]
endpoint = "https://llm.example.com/v1"
inject = "x"

[credentials.c]
description = "d"
provider = "vault"
name = "X"
inject_as = "X"
"#;
        let (_dir, path) = staged_file("context.toml", text);
        match Config::load(Some(&path), &no_env) {
            Err(AppError::Config(violations)) => {
                let paths: Vec<&str> = violations.iter().map(|v| v.path.as_str()).collect();
                for expected in [
                    "profiles.work.llm.description",
                    "profiles.work.llm.inject",
                    "credentials.c.provider",
                ] {
                    assert!(
                        paths.contains(&expected),
                        "expected a violation at {expected}, got {paths:?}"
                    );
                }
            }
            other => panic!("expected a config error, got {other:?}"),
        }
    }

    #[test]
    fn load_empty_file_reports_missing_version() {
        // EDGE-003 (logic level).
        let (_dir, path) = staged_file("context.toml", "");
        match Config::load(Some(&path), &no_env) {
            Err(AppError::Config(violations)) => {
                assert_eq!(violations[0].path, "version");
            }
            other => panic!("expected a config error, got {other:?}"),
        }
    }

    #[test]
    fn load_rejects_an_unreadable_file() {
        // SPEC-001 (logic level): EACCES maps to an exit-2 config error
        // naming the path and the error.
        let (_dir, path) = staged_file("context.toml", VALID_CONFIG);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o000))
                .expect("permissions are set");
        }
        match Config::load(Some(&path), &no_env) {
            Err(AppError::Config(violations)) => {
                assert!(
                    violations[0].message.contains("cannot read"),
                    "{}",
                    violations[0]
                );
            }
            other => panic!("expected a config error, got {other:?}"),
        }
    }

    #[test]
    fn load_accepts_a_config_without_profiles_or_credentials() {
        // EDGE-016 (logic level): absent tables are valid.
        let (_dir, path) = staged_file("context.toml", "version = 1\n");
        let config = Config::load(Some(&path), &no_env).expect("a bare version loads");
        assert!(config.profiles.is_empty());
        assert!(config.credentials.is_empty());
        assert_eq!(config.default_profile, None);
    }

    #[test]
    fn parse_position_math_matches_toml_spans() {
        let text = "a = 1\nb = 2\nc = 3\n";
        assert_eq!(super::line_column(text, 0), (1, 1));
        assert_eq!(super::line_column(text, 6), (2, 1));
        assert_eq!(super::line_column(text, 10), (2, 5));
        assert_eq!(super::line_column(text, 12), (3, 1));
        assert_eq!(super::line_column(text, 18), (4, 1));
    }

    #[test]
    fn load_uses_the_explicit_file_over_the_environment() {
        let (_dir, path) = staged_file("explicit.toml", VALID_CONFIG);
        let (_other_dir, other_path) = staged_file("other.toml", "version = 1\n");
        let other_text = other_path.display().to_string();
        let env = move |name: &str| match name {
            "AGENT_CONTEXT_FILE" => Some(other_text.clone()),
            _ => None,
        };
        let config = Config::load(Some(&path), &env).expect("the explicit file wins");
        assert_eq!(config.profiles.len(), 1, "the explicit file was loaded");
    }

    #[test]
    fn load_missing_default_directory_error_names_remedies() {
        // AC-001.3 (logic level) through load itself.
        match Config::load(None, &no_env) {
            Err(AppError::Config(violations)) => {
                assert!(violations[0].message.contains("HOME"), "{}", violations[0]);
            }
            other => panic!("expected a config error, got {other:?}"),
        }
    }
}
