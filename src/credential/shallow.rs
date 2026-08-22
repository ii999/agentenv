//! Shallow credential status (SPEC-012).
//!
//! Status is computed from credential metadata alone: environment presence
//! for `env` (empty values count as unset), a constant `configured` for
//! `keychain` (no store read), and executable discovery for `command` (no
//! process launch). Executability means direct-launch semantics: any execute
//! bit on Unix; `.exe`/`.com` extensions on Windows, where
//! interpreter-dependent extensions (`.bat`, `.cmd`, `.ps1`) are not
//! discoverable - a script provider must name its interpreter as `argv[0]`.

use std::fmt;
use std::path::{Path, PathBuf};

use crate::config::{env_value, CredentialDef, Provider};

/// The shallow status of a credential (SPEC-012).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// `env` provider: the variable is set to a non-empty value.
    Available,
    /// `env` provider: the variable is unset (or empty).
    NotSet,
    /// `keychain` provider, or `command` with a discoverable executable.
    Configured,
    /// `command` provider: `argv[0]` is not an executable file.
    CommandMissing,
}

impl Status {
    /// The stable JSON token (SPEC-010 `status` tokens).
    pub fn json_token(&self) -> &'static str {
        match self {
            Self::Available => "available",
            Self::NotSet => "not_set",
            Self::Configured => "configured",
            Self::CommandMissing => "command_missing",
        }
    }
}

impl fmt::Display for Status {
    /// The humanized text form; JSON output always uses
    /// [`Status::json_token`] instead.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text = match self {
            Self::Available => "available",
            Self::NotSet => "not set",
            Self::Configured => "configured",
            Self::CommandMissing => "command missing",
        };
        formatter.write_str(text)
    }
}

/// Computes the shallow status of `credential` from environment values
/// supplied by `env` (empty values count as unset). Never reads a store and
/// never launches a process.
pub fn shallow_status(credential: &CredentialDef, env: &impl Fn(&str) -> Option<String>) -> Status {
    match &credential.provider {
        Provider::Env { name } => env_status(name, env),
        Provider::Keychain { .. } => keychain_status(),
        Provider::Command { argv } => {
            let program = argv.first().map(String::as_str).unwrap_or_default();
            command_status(program, env)
        }
    }
}

pub(crate) fn env_status(name: &str, env: &impl Fn(&str) -> Option<String>) -> Status {
    if env_value(env, name).is_some() {
        Status::Available
    } else {
        Status::NotSet
    }
}

pub(crate) fn keychain_status() -> Status {
    Status::Configured
}

pub(crate) fn command_status(program: &str, env: &impl Fn(&str) -> Option<String>) -> Status {
    if program.is_empty() {
        return Status::CommandMissing;
    }
    if program.chars().any(std::path::is_separator) {
        // A separator means a direct path; relative paths resolve against
        // the current working directory.
        return if is_executable_file(Path::new(program)) {
            Status::Configured
        } else {
            Status::CommandMissing
        };
    }
    let Some(path_value) = env_value(env, "PATH") else {
        return Status::CommandMissing;
    };
    for directory in path_value.split(path_separator()) {
        for candidate in search_candidates(Path::new(directory), program) {
            if is_executable_file(&candidate) {
                return Status::Configured;
            }
        }
    }
    Status::CommandMissing
}

#[cfg(unix)]
fn path_separator() -> char {
    ':'
}

#[cfg(windows)]
fn path_separator() -> char {
    ';'
}

/// The candidate paths for `program` within one PATH directory. On Windows,
/// a `program` without an extension is also probed as `.exe` and `.com`.
#[cfg(unix)]
fn search_candidates(directory: &Path, program: &str) -> Vec<PathBuf> {
    vec![directory.join(program)]
}

#[cfg(windows)]
fn search_candidates(directory: &Path, program: &str) -> Vec<PathBuf> {
    let mut candidates = vec![directory.join(program)];
    if !program.contains('.') {
        candidates.push(directory.join(format!("{program}.exe")));
        candidates.push(directory.join(format!("{program}.com")));
    }
    candidates
}

fn is_executable_file(path: &Path) -> bool {
    let Ok(metadata) = std::fs::metadata(path) else {
        return false;
    };
    metadata.is_file() && is_executable(path, &metadata)
}

#[cfg(unix)]
fn is_executable(_path: &Path, metadata: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;
    metadata.permissions().mode() & 0o111 != 0
}

#[cfg(windows)]
fn is_executable(path: &Path, _metadata: &std::fs::Metadata) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| matches!(extension.to_ascii_lowercase().as_str(), "exe" | "com"))
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;

    use tempfile::TempDir;

    use super::{shallow_status, Status};
    use crate::config::{CredentialDef, Provider};

    fn env_credential() -> CredentialDef {
        CredentialDef {
            name: "company_llm".to_owned(),
            description: "d".to_owned(),
            provider: Provider::Env {
                name: "COMPANY_LLM_TOKEN".to_owned(),
            },
            inject_as: "OPENAI_API_KEY".to_owned(),
        }
    }

    fn keychain_credential() -> CredentialDef {
        CredentialDef {
            name: "personal".to_owned(),
            description: "d".to_owned(),
            provider: Provider::Keychain {
                service: "agentenv".to_owned(),
                account: "personal".to_owned(),
            },
            inject_as: "OPENAI_API_KEY".to_owned(),
        }
    }

    fn command_credential(argv: &[&str]) -> CredentialDef {
        CredentialDef {
            name: "c".to_owned(),
            description: "d".to_owned(),
            provider: Provider::Command {
                argv: argv.iter().map(|s| s.to_string()).collect(),
            },
            inject_as: "C".to_owned(),
        }
    }

    fn env_of<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |name| {
            pairs
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| value.to_string())
        }
    }

    #[test]
    fn env_provider_status_follows_variable_presence() {
        // AC-012.3.
        let credential = env_credential();
        assert_eq!(
            shallow_status(&credential, &env_of(&[("COMPANY_LLM_TOKEN", "value")])),
            Status::Available
        );
        assert_eq!(shallow_status(&credential, &env_of(&[])), Status::NotSet);
        // SPEC-AS-028: empty counts as unset.
        assert_eq!(
            shallow_status(&credential, &env_of(&[("COMPANY_LLM_TOKEN", "")])),
            Status::NotSet
        );
    }

    #[test]
    fn keychain_provider_is_always_configured() {
        // No store read happens, so nothing can be probed here.
        let credential = keychain_credential();
        assert_eq!(
            shallow_status(&credential, &env_of(&[])),
            Status::Configured
        );
    }

    fn executable_file(dir: &TempDir, name: &str, executable: bool) -> PathBuf {
        let path = dir.path().join(name);
        fs::write(&path, "#!/bin/sh\nexit 0\n").expect("the file is written");
        let mode = if executable { 0o755 } else { 0o644 };
        fs::set_permissions(&path, fs::Permissions::from_mode(mode)).expect("permissions are set");
        path
    }

    #[test]
    fn command_provider_direct_path_executable() {
        let dir = TempDir::new().expect("a temp dir");
        let program = executable_file(&dir, "get-token", true);
        let credential = command_credential(&[&program.display().to_string(), "--flag"]);
        assert_eq!(
            shallow_status(&credential, &env_of(&[])),
            Status::Configured,
            "an executable file with a path separator is configured"
        );
    }

    #[test]
    fn command_provider_direct_path_not_executable() {
        let dir = TempDir::new().expect("a temp dir");
        let program = executable_file(&dir, "get-token", false);
        let credential = command_credential(&[&program.display().to_string()]);
        assert_eq!(
            shallow_status(&credential, &env_of(&[])),
            Status::CommandMissing,
            "a non-executable file is command missing"
        );
    }

    #[test]
    fn command_provider_direct_path_directory() {
        // EDGE-020.
        let dir = TempDir::new().expect("a temp dir");
        let program = dir.path().join("subdir");
        fs::create_dir(&program).expect("the directory is created");
        let credential = command_credential(&[&program.display().to_string()]);
        assert_eq!(
            shallow_status(&credential, &env_of(&[])),
            Status::CommandMissing,
            "a directory is command missing"
        );
    }

    #[test]
    fn command_provider_direct_path_missing() {
        let dir = TempDir::new().expect("a temp dir");
        let program = dir.path().join("absent");
        let credential = command_credential(&[&program.display().to_string()]);
        assert_eq!(
            shallow_status(&credential, &env_of(&[])),
            Status::CommandMissing
        );
    }

    #[test]
    fn command_provider_path_search_finds_executable() {
        let dir = TempDir::new().expect("a temp dir");
        executable_file(&dir, "probe-cmd", true);
        let credential = command_credential(&["probe-cmd"]);
        let path_value = dir.path().display().to_string();
        assert_eq!(
            shallow_status(&credential, &env_of(&[("PATH", &path_value)])),
            Status::Configured,
            "an executable on PATH is configured"
        );
    }

    #[test]
    fn command_provider_path_search_skips_non_executable() {
        let dir = TempDir::new().expect("a temp dir");
        executable_file(&dir, "probe-cmd", false);
        let credential = command_credential(&["probe-cmd"]);
        let path_value = dir.path().display().to_string();
        assert_eq!(
            shallow_status(&credential, &env_of(&[("PATH", &path_value)])),
            Status::CommandMissing,
            "a non-executable file on PATH is command missing"
        );
    }

    #[test]
    fn command_provider_without_path_is_command_missing() {
        let credential = command_credential(&["probe-cmd"]);
        assert_eq!(
            shallow_status(&credential, &env_of(&[])),
            Status::CommandMissing
        );
        assert_eq!(
            shallow_status(&credential, &env_of(&[("PATH", "")])),
            Status::CommandMissing,
            "an empty PATH counts as unset"
        );
    }

    #[test]
    fn command_provider_path_search_not_found() {
        let dir = TempDir::new().expect("a temp dir");
        let credential = command_credential(&["probe-cmd"]);
        let path_value = dir.path().display().to_string();
        assert_eq!(
            shallow_status(&credential, &env_of(&[("PATH", &path_value)])),
            Status::CommandMissing
        );
    }

    #[test]
    fn command_provider_searches_every_directory() {
        let dir = TempDir::new().expect("a temp dir");
        let empty = TempDir::new().expect("another temp dir");
        executable_file(&dir, "probe-cmd", true);
        let credential = command_credential(&["probe-cmd"]);
        let path_value = format!("{}:{}", empty.path().display(), dir.path().display());
        assert_eq!(
            shallow_status(&credential, &env_of(&[("PATH", &path_value)])),
            Status::Configured
        );
    }

    #[test]
    fn command_provider_empty_argv0_is_command_missing() {
        // Unreachable through a validated config (argv[0] must be non-empty)
        // but honest for any CredentialDef.
        let credential = command_credential(&[]);
        assert_eq!(
            shallow_status(&credential, &env_of(&[])),
            Status::CommandMissing
        );
    }

    #[test]
    fn status_tokens_and_display() {
        assert_eq!(Status::Available.json_token(), "available");
        assert_eq!(Status::NotSet.json_token(), "not_set");
        assert_eq!(Status::Configured.json_token(), "configured");
        assert_eq!(Status::CommandMissing.json_token(), "command_missing");
        assert_eq!(Status::Available.to_string(), "available");
        assert_eq!(Status::NotSet.to_string(), "not set");
        assert_eq!(Status::Configured.to_string(), "configured");
        assert_eq!(Status::CommandMissing.to_string(), "command missing");
    }
}
