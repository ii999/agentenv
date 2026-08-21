//! Config file location (SPEC-001).
//!
//! Priority: an explicit file (caller-provided), else `AGENT_CONTEXT_FILE`,
//! else the platform default - `$XDG_CONFIG_HOME` when it names an absolute
//! path, else `~/.config` on Unix, `%APPDATA%` on Windows (`XDG_CONFIG_HOME`
//! is not consulted there). Environment values that are empty count as
//! unset; a relative `XDG_CONFIG_HOME` counts as unset (AC-001.4).

use std::path::{Path, PathBuf};

use super::env_value;
use crate::error::{AppError, Violation};

const CONFIG_DIR: &str = "agent-context";
const CONFIG_FILE: &str = "context.toml";

/// Resolves the config file path. Pure environment logic with no filesystem
/// access, so callers (such as the CLI, to name the file in diagnostics) can
/// re-resolve deterministically.
pub fn resolve_path(
    explicit_file: Option<&Path>,
    env: &impl Fn(&str) -> Option<String>,
) -> Result<PathBuf, AppError> {
    if let Some(path) = explicit_file {
        return Ok(path.to_path_buf());
    }
    if let Some(file) = env_value(env, "AGENT_CONTEXT_FILE") {
        return Ok(PathBuf::from(file));
    }
    default_path(env)
}

#[cfg(unix)]
fn default_path(env: &impl Fn(&str) -> Option<String>) -> Result<PathBuf, AppError> {
    if let Some(xdg) = env_value(env, "XDG_CONFIG_HOME") {
        let base = Path::new(&xdg);
        if base.is_absolute() {
            return Ok(base.join(CONFIG_DIR).join(CONFIG_FILE));
        }
        // An empty or relative value is treated as unset (AC-001.4).
    }
    match env_value(env, "HOME") {
        Some(home) => Ok(Path::new(&home)
            .join(".config")
            .join(CONFIG_DIR)
            .join(CONFIG_FILE)),
        None => Err(AppError::Config(vec![Violation {
            path: "HOME".to_owned(),
            message: "cannot locate the config file: HOME is not set and AGENT_CONTEXT_FILE is \
                      not set; set HOME, XDG_CONFIG_HOME, or AGENT_CONTEXT_FILE"
                .to_owned(),
        }])),
    }
}

#[cfg(windows)]
fn default_path(env: &impl Fn(&str) -> Option<String>) -> Result<PathBuf, AppError> {
    // XDG_CONFIG_HOME is not consulted on Windows (design §3).
    match env_value(env, "APPDATA") {
        Some(appdata) => Ok(Path::new(&appdata).join(CONFIG_DIR).join(CONFIG_FILE)),
        None => Err(AppError::Config(vec![Violation {
            path: "APPDATA".to_owned(),
            message: "cannot locate the config file: APPDATA is not set and AGENT_CONTEXT_FILE \
                      is not set; set APPDATA or AGENT_CONTEXT_FILE"
                .to_owned(),
        }])),
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::resolve_path;
    use crate::error::AppError;

    fn env_of<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |name| {
            pairs
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| value.to_string())
        }
    }

    #[test]
    fn explicit_file_wins_over_everything() {
        // AC-001.1 (logic level).
        let env = env_of(&[("AGENT_CONTEXT_FILE", "/tmp/other.toml")]);
        let path = resolve_path(Some(Path::new("/tmp/x.toml")), &env).expect("explicit wins");
        assert_eq!(path, PathBuf::from("/tmp/x.toml"));
    }

    #[test]
    fn agent_context_file_beats_platform_default() {
        // AC-001.1 (logic level).
        let env = env_of(&[
            ("AGENT_CONTEXT_FILE", "/tmp/x.toml"),
            ("HOME", "/home/user"),
        ]);
        let path = resolve_path(None, &env).expect("the env var wins");
        assert_eq!(path, PathBuf::from("/tmp/x.toml"));
    }

    #[test]
    fn empty_agent_context_file_counts_as_unset() {
        // SPEC-AS-028.
        let env = env_of(&[("AGENT_CONTEXT_FILE", ""), ("HOME", "/home/user")]);
        let path = resolve_path(None, &env).expect("HOME is used");
        assert_eq!(
            path,
            PathBuf::from("/home/user/.config/agent-context/context.toml")
        );
    }

    #[test]
    fn absolute_xdg_config_home_is_used() {
        let env = env_of(&[("XDG_CONFIG_HOME", "/xdg/base"), ("HOME", "/home/user")]);
        let path = resolve_path(None, &env).expect("XDG wins");
        assert_eq!(path, PathBuf::from("/xdg/base/agent-context/context.toml"));
    }

    #[test]
    fn relative_xdg_config_home_is_ignored() {
        // AC-001.4.
        let env = env_of(&[("XDG_CONFIG_HOME", "relative/dir"), ("HOME", "/home/user")]);
        let path = resolve_path(None, &env).expect("HOME is used");
        assert_eq!(
            path,
            PathBuf::from("/home/user/.config/agent-context/context.toml")
        );
    }

    #[test]
    fn empty_xdg_config_home_is_ignored() {
        let env = env_of(&[("XDG_CONFIG_HOME", ""), ("HOME", "/home/user")]);
        let path = resolve_path(None, &env).expect("HOME is used");
        assert_eq!(
            path,
            PathBuf::from("/home/user/.config/agent-context/context.toml")
        );
    }

    #[test]
    fn home_default_path() {
        let env = env_of(&[("HOME", "/home/user")]);
        let path = resolve_path(None, &env).expect("HOME is used");
        assert_eq!(
            path,
            PathBuf::from("/home/user/.config/agent-context/context.toml")
        );
    }

    #[test]
    fn missing_home_names_the_remedies() {
        // AC-001.3 / EDGE-014 (logic level).
        let env = env_of(&[]);
        match resolve_path(None, &env) {
            Err(AppError::Config(violations)) => {
                assert_eq!(violations.len(), 1);
                let violation = &violations[0];
                assert_eq!(violation.path, "HOME");
                assert!(violation.message.contains("HOME"), "{}", violation);
                assert!(violation.message.contains("AGENT_CONTEXT_FILE"));
            }
            other => panic!("expected a config error, got {other:?}"),
        }
    }

    #[test]
    fn empty_home_counts_as_unset() {
        let env = env_of(&[("HOME", "")]);
        match resolve_path(None, &env) {
            Err(AppError::Config(violations)) => {
                assert_eq!(violations[0].path, "HOME");
            }
            other => panic!("expected a config error, got {other:?}"),
        }
    }

    #[cfg(windows)]
    #[test]
    fn appdata_default_path() {
        let env = env_of(&[("APPDATA", r"C:\Users\u\AppData\Roaming")]);
        let path = resolve_path(None, &env).expect("APPDATA is used");
        assert_eq!(
            path,
            PathBuf::from(r"C:\Users\u\AppData\Roaming\agent-context\context.toml")
        );
    }

    #[cfg(windows)]
    #[test]
    fn xdg_config_home_is_not_consulted_on_windows() {
        let env = env_of(&[
            ("XDG_CONFIG_HOME", r"C:\xdg"),
            ("APPDATA", r"C:\Users\u\AppData\Roaming"),
        ]);
        let path = resolve_path(None, &env).expect("APPDATA is used");
        assert_eq!(
            path,
            PathBuf::from(r"C:\Users\u\AppData\Roaming\agent-context\context.toml")
        );
    }

    #[cfg(windows)]
    #[test]
    fn missing_appdata_names_the_remedies() {
        let env = env_of(&[]);
        match resolve_path(None, &env) {
            Err(AppError::Config(violations)) => {
                assert!(violations[0].message.contains("APPDATA"));
            }
            other => panic!("expected a config error, got {other:?}"),
        }
    }
}
