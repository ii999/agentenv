//! Whole-file validation for the `validate` command: schema validation via
//! `Config::load` plus the Unix permission check, with violations from both
//! aggregated into one report.

use std::path::Path;

use agentenv::config::Config;
use agentenv::error::AppError;

pub(super) fn validate_config(env: &impl Fn(&str) -> Option<String>) -> Result<(), AppError> {
    let config_result = Config::load(None, env);
    let permission_result = validate_permissions(None, env);
    match (config_result, permission_result) {
        (Ok(_), Ok(())) => Ok(()),
        (Err(AppError::Config(mut violations)), Err(AppError::Config(permission_violations))) => {
            violations.extend(permission_violations);
            Err(AppError::Config(violations))
        }
        (Err(error), Ok(())) | (Err(error), Err(_)) => Err(error),
        (Ok(_), Err(error)) => Err(error),
    }
}

fn validate_permissions(
    explicit_file: Option<&Path>,
    env: &impl Fn(&str) -> Option<String>,
) -> Result<(), AppError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        let path = agentenv::config::locate::resolve_path(explicit_file, env)?;
        let metadata = std::fs::metadata(&path).map_err(|error| {
            AppError::Config(vec![agentenv::error::Violation {
                path: path.display().to_string(),
                message: format!("cannot inspect config-file permissions: {error}"),
            }])
        })?;
        let mode = metadata.permissions().mode() & 0o777;
        if mode & !0o600 != 0 {
            return Err(AppError::Config(vec![agentenv::error::Violation {
                path: path.display().to_string(),
                message: format!(
                    "config-file permissions are {mode:04o}; permissions must be a subset of 0600"
                ),
            }]));
        }
    }
    #[cfg(not(unix))]
    {
        let _ = (explicit_file, env);
    }
    Ok(())
}
