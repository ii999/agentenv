//! Dialog preflight stays ahead of GUI and credential resolution. Interactive
//! acceptance uses a real window with a separate test-owned credential store.
#![cfg(feature = "test-keychain")]

mod helpers;

use std::fs;

use helpers::{assert_exit, assert_mentions, assert_omits, run_ac};
use tempfile::TempDir;

#[test]
fn unknown_and_externally_managed_credentials_fail_before_opening_a_window() {
    let directory = TempDir::new().unwrap();
    let config = directory.path().join("config.toml");
    fs::write(&config, "version = 1\n[credentials.external]\ndescription = \"Externally managed fixture.\"\nprovider = \"env\"\nname = \"DIALOG_EXTERNAL_VALUE\"\ninject_as = \"TEST_TOKEN\"\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&config, fs::Permissions::from_mode(0o600)).unwrap();
    }
    let missing = run_ac(
        &config,
        &[],
        &[
            "credential",
            "set",
            "missing",
            "--input",
            "dialog",
            "--json",
        ],
    );
    assert_exit(&missing, 3, "an unknown definition is a preflight error");
    assert!(missing.stdout.is_empty());
    let external = run_ac(
        &config,
        &[("DIALOG_EXTERNAL_VALUE", "dialog-external-canary-K8m2")],
        &[
            "credential",
            "set",
            "external",
            "--input",
            "dialog",
            "--json",
        ],
    );
    assert_exit(
        &external,
        1,
        "external credentials cannot be stored through a dialog",
    );
    assert_mentions(
        &external,
        "managed externally",
        "the provider ownership is explained",
    );
    assert_omits(
        &external,
        "dialog-external-canary-K8m2",
        "preflight does not resolve or print the value",
    );
}

#[test]
fn explicit_terminal_input_does_not_read_a_tool_pipe() {
    let directory = TempDir::new().unwrap();
    let config = directory.path().join("config.toml");
    fs::write(&config, "version = 1\n[credentials.local]\ndescription = \"Dialog fixture.\"\nprovider = \"keychain\"\nservice = \"agentenv-dialog-test\"\naccount = \"fixture\"\ninject_as = \"TEST_TOKEN\"\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&config, fs::Permissions::from_mode(0o600)).unwrap();
    }
    let store = directory.path().join("keychain.json");
    let output = run_ac(
        &config,
        &[("AGENTENV_TEST_KEYCHAIN", store.to_str().unwrap())],
        &["credential", "set", "local", "--input", "terminal"],
    );
    assert_exit(&output, 1, "explicit terminal mode requires terminal input");
    assert_mentions(
        &output,
        "interactive terminal",
        "the required interface is explained",
    );
    assert!(!store.exists());
}

#[cfg(target_os = "linux")]
#[test]
fn missing_desktop_returns_structured_failure_without_reading_stdin() {
    let directory = TempDir::new().unwrap();
    let config = directory.path().join("config.toml");
    fs::write(&config, "version = 1\n[credentials.local]\ndescription = \"Dialog fixture.\"\nprovider = \"keychain\"\nservice = \"agentenv-dialog-test\"\naccount = \"fixture\"\ninject_as = \"TEST_TOKEN\"\n").unwrap();
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(&config, fs::Permissions::from_mode(0o600)).unwrap();
    let store = directory.path().join("keychain.json");
    let output = run_ac(
        &config,
        &[("AGENTENV_TEST_KEYCHAIN", store.to_str().unwrap())],
        &["credential", "set", "local", "--input", "dialog", "--json"],
    );
    assert_exit(&output, 4, "a graphical desktop is required");
    let result: serde_json::Value = serde_json::from_str(&output.stdout).unwrap();
    assert_eq!(
        result,
        serde_json::json!({"version":1,"credential":"local","input":"dialog","status":"failed","reason":"desktop-unavailable"})
    );
    assert!(!store.exists());
}
