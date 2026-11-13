//! Native helper integration using only a disposable Keychain and synthetic values.

#![cfg(all(target_os = "macos", feature = "test-support"))]

use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use assert_cmd::cargo::cargo_bin;
use core_foundation::base::TCFType;
use security_framework::os::macos::keychain::{CreateOptions, SecKeychain};
use security_framework_sys::base::SecKeychainRef;
use sha2::{Digest, Sha256};

extern "C" {
    fn SecKeychainDelete(keychain: SecKeychainRef) -> i32;
    fn SecKeychainLock(keychain: SecKeychainRef) -> i32;
}

struct Fixture {
    keychain: SecKeychain,
    keychain_path: PathBuf,
    directory: tempfile::TempDir,
    config: PathBuf,
    binary: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let keychain_path = directory.path().join("native-helper.keychain");
        let keychain = CreateOptions::new()
            .password("agentenv-native-fixture")
            .create(&keychain_path)
            .unwrap();
        let config = directory.path().join("config.toml");
        fs::write(
            &config,
            r#"version = 1
default_profile = "test"
[profiles.test]
description = "Native fixture."
[credentials.fixture]
description = "Synthetic native credential."
provider = "keychain"
service = "agentenv-test-native-helper"
account = "fixture"
inject_as = "AGENTENV_SYNTHETIC_TOKEN"
[profiles.test.token]
description = "Synthetic token consumer."
credential = "credential://fixture"
"#,
        )
        .unwrap();
        fs::set_permissions(&config, fs::Permissions::from_mode(0o600)).unwrap();
        Self {
            keychain,
            keychain_path,
            directory,
            config,
            binary: cargo_bin("agentenv"),
        }
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(&self.binary);
        command
            .env_clear()
            .current_dir(self.directory.path())
            .env("HOME", self.directory.path())
            .env("XDG_DATA_HOME", self.directory.path().join("data"))
            .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
            .env("AGENTENV_FILE", &self.config)
            .env("AGENTENV_TEST_NATIVE_KEYCHAIN", &self.keychain_path)
            .env("AGENTENV_TEST_KEYCHAIN_NO_UI", "1")
            .args(args);
        command
    }

    fn run(&self, args: &[&str]) -> Output {
        self.command(args).output().unwrap()
    }

    fn without_companion(&mut self) {
        self.binary = self.directory.path().join("agentenv");
        fs::copy(cargo_bin("agentenv"), &self.binary).unwrap();
    }

    fn setup(&self, replace: bool) -> Output {
        let source = cargo_bin("agentenv-keychain-helper");
        let mut command = self.command(&["keychain", "setup", "--json", "--from"]);
        command.arg(source);
        if replace {
            command.arg("--replace-helper");
        }
        let output = command.output().unwrap();
        if !output.status.success() {
            // Identity listings contain public certificate metadata only.
            // Compare the subprocess home with the native login environment.
            for fixture_home in [true, false] {
                let mut inspect = Command::new("/usr/bin/security");
                inspect
                    .args(["find-identity", "-p", "codesigning"])
                    .arg(&self.keychain_path);
                if fixture_home {
                    inspect.env("HOME", self.directory.path());
                }
                let result = inspect.output().unwrap();
                eprintln!(
                    "Fixture identity lookup (isolated home: {fixture_home}): {}\n{}\n{}",
                    result.status,
                    String::from_utf8_lossy(&result.stdout),
                    String::from_utf8_lossy(&result.stderr)
                );
            }
        }
        output
    }

    fn helper(&self) -> PathBuf {
        self.directory
            .path()
            .join("data/agentenv/macos-keychain/agentenv-keychain-helper")
    }

    fn store(&self, value: &str, lose_reply: bool) -> Output {
        let mut command = self.command(&["credential", "set", "fixture"]);
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if lose_reply {
            command.env("AGENTENV_TEST_KEYCHAIN_DROP_STORE_REPLY", "1");
        }
        let mut child = command.spawn().unwrap();
        let mut input = child.stdin.take().unwrap();
        input.write_all(value.as_bytes()).unwrap();
        // Piped credential entry strips one conventional terminating newline.
        input.write_all(b"\n").unwrap();
        drop(input);
        child.wait_with_output().unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        assert_eq!(
            unsafe { SecKeychainDelete(self.keychain.as_concrete_TypeRef()) },
            0
        );
    }
}

#[track_caller]
fn success(output: &Output) {
    assert!(
        output.status.success(),
        "status: {}\nstdout: {}\nstderr: {}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn digest(path: &Path) -> Vec<u8> {
    Sha256::digest(fs::read(path).unwrap()).to_vec()
}

#[test]
fn native_helper_preserves_values_and_setup_and_classifies_lock_and_store_failures() {
    let _no_ui = SecKeychain::disable_user_interaction().unwrap();
    let mut fixture = Fixture::new();
    let before = fixture.run(&["keychain", "status", "--json"]);
    success(&before);
    let before: serde_json::Value = serde_json::from_slice(&before.stdout).unwrap();
    assert_eq!(before["helper"]["status"], "not-installed");
    assert!(
        !fixture.directory.path().join("data").exists(),
        "status does not enroll an identity"
    );

    success(&fixture.setup(false));
    let original = fs::read(fixture.helper()).unwrap();
    let original_hash = digest(&fixture.helper());
    let value = format!("  native-密钥-🙂\n{}\r\n", "x".repeat(20_000));
    success(&fixture.store(&value, false));
    success(&fixture.run(&["credential", "check", "fixture"]));
    let checked = fixture.run(&[
        "run",
        "--with",
        "token",
        "--",
        "/bin/sh",
        "-c",
        "test \"$AGENTENV_SYNTHETIC_TOKEN\" = \"$1\"",
        "fixture",
        &value,
    ]);
    success(&checked);
    assert!(checked.stdout.is_empty() && checked.stderr.is_empty());
    assert!(
        fixture
            .keychain
            .find_generic_password("agentenv-test-native-helper", "fixture")
            .is_err(),
        "an unrelated executable cannot read the helper's credential without authorization"
    );

    let reused = fixture.setup(false);
    success(&reused);
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&reused.stdout).unwrap()["details"]["reused"],
        true
    );
    assert_eq!(
        digest(&fixture.helper()),
        original_hash,
        "setup retains the exact signed helper"
    );

    let uncertain = fixture.store("synthetic-replacement", true);
    assert_eq!(uncertain.status.code(), Some(4));
    assert!(String::from_utf8_lossy(&uncertain.stderr).contains("may have been stored"));
    success(&fixture.run(&["credential", "check", "fixture"]));

    assert_eq!(
        unsafe { SecKeychainLock(fixture.keychain.as_concrete_TypeRef()) },
        0
    );
    let locked = fixture.run(&["keychain", "status", "--json"]);
    success(&locked);
    let locked: serde_json::Value = serde_json::from_slice(&locked.stdout).unwrap();
    assert_eq!(locked["keychain"]["unlocked"], false);
    assert_eq!(
        locked["helper"]["status"], "ready",
        "helper signature verification needs no unlocked Keychain"
    );
    let denied = fixture.run(&["credential", "check", "fixture"]);
    assert_eq!(denied.status.code(), Some(4));
    assert!(String::from_utf8_lossy(&denied.stderr).contains("Keychain locked"));
    assert!(String::from_utf8_lossy(&denied.stderr).contains("interaction"));
    let wrapper = fixture.directory.path().join("confidential-resolver");
    let quoted = |path: &Path| format!("'{}'", path.to_string_lossy().replace('\'', "'\"'\"'"));
    fs::write(&wrapper, format!(
        "#!/bin/sh\nexport HOME={}\nexport XDG_DATA_HOME={}\nexport AGENTENV_TEST_NATIVE_KEYCHAIN={}\nexport AGENTENV_TEST_KEYCHAIN_NO_UI=1\nexec {} \"$@\"\n",
        quoted(fixture.directory.path()), quoted(&fixture.directory.path().join("data")),
        quoted(&fixture.keychain_path), quoted(&cargo_bin("agentenv")),
    )).unwrap();
    fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o700)).unwrap();
    let definition = agentenv::config::CredentialDef {
        name: "fixture".into(),
        description: "Synthetic locked-Keychain resolver fixture.".into(),
        provider: agentenv::config::Provider::Keychain {
            service: "agentenv-test-native-helper".into(),
            account: "fixture".into(),
        },
        inject_as: Some("AGENTENV_SYNTHETIC_TOKEN".into()),
        usages: vec![agentenv::config::CredentialUsage::Environment],
    };
    let resolution = tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(agentenv::credential::resolver::resolve_detailed(
            &wrapper,
            &definition,
            agentenv::credential::resolver::ResolutionStage::Fill,
            8192,
            std::time::Duration::from_secs(5),
        ))
        .unwrap_err();
    assert!(
        matches!(resolution, agentenv::credential::resolver::ResolveError::Keychain(error)
        if error.locked == Some(true) && error.reason == agentenv::credential::KeychainReason::InteractionUnavailable)
    );
    fixture
        .keychain
        .unlock(Some("agentenv-native-fixture"))
        .unwrap();
    success(&fixture.run(&["credential", "check", "fixture"]));

    let mut changed = original.clone();
    changed.push(0);
    let staged = fixture.directory.path().join("modified-helper");
    fs::write(&staged, changed).unwrap();
    fs::set_permissions(&staged, fs::Permissions::from_mode(0o700)).unwrap();
    fs::rename(&staged, fixture.helper()).unwrap();
    let modified = fixture.run(&["credential", "check", "fixture"]);
    assert_eq!(modified.status.code(), Some(4));
    assert!(String::from_utf8_lossy(&modified.stderr).contains("could not be verified"));
    assert!(
        !fixture.setup(false).status.success(),
        "ordinary setup cannot silently replace a modified helper"
    );
    fs::write(&staged, original).unwrap();
    fs::set_permissions(&staged, fs::Permissions::from_mode(0o700)).unwrap();
    fs::rename(&staged, fixture.helper()).unwrap();
    success(&fixture.run(&["credential", "check", "fixture"]));
}

#[test]
fn helper_refuses_an_invocation_without_a_private_channel() {
    let output = Command::new(cargo_bin("agentenv-keychain-helper"))
        .arg("--serve")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(4));
    assert!(output.stdout.is_empty());
}

#[test]
fn missing_helper_fails_without_implicit_installation_or_cli_substitution() {
    let _no_ui = SecKeychain::disable_user_interaction().unwrap();
    let mut fixture = Fixture::new();
    fixture.without_companion();
    for args in [
        vec!["--version"],
        vec!["--help"],
        vec!["keychain", "status", "--json"],
    ] {
        success(&fixture.run(&args));
    }
    for args in [vec!["--identity"], vec!["--serve"]] {
        assert_eq!(fixture.run(&args).status.code(), Some(1));
    }
    let missing = fixture.run(&["credential", "check", "fixture"]);
    assert_eq!(missing.status.code(), Some(4));
    assert!(String::from_utf8_lossy(&missing.stderr).contains("keychain setup"));
    assert_eq!(
        fixture
            .store("synthetic-unwritten-value", false)
            .status
            .code(),
        Some(4)
    );
    assert!(!fixture.directory.path().join("data").exists());
    assert_eq!(
        fixture
            .keychain
            .find_generic_password("agentenv-test-native-helper", "fixture")
            .unwrap_err()
            .code(),
        -25300,
        "failed storage does not create a credential"
    );

    assert!(!fixture.run(&["keychain", "setup"]).status.success());
    assert!(!fixture.helper().exists());
    assert!(!fixture
        .helper()
        .parent()
        .unwrap()
        .join("signing.toml")
        .exists());
    let mut selected_cli = fixture.command(&["keychain", "setup", "--from"]);
    selected_cli.arg(&fixture.binary);
    assert!(!selected_cli.output().unwrap().status.success());
    assert!(!fixture.helper().exists());

    let candidate = fixture.directory.path().join("agentenv-keychain-helper");
    fs::write(&candidate, b"invalid executable").unwrap();
    fs::set_permissions(&candidate, fs::Permissions::from_mode(0o700)).unwrap();
    assert!(!fixture.run(&["keychain", "setup"]).status.success());
    assert!(!fixture.helper().exists());
    fs::remove_file(candidate).unwrap();
    success(&fixture.setup(false));
    success(&fixture.store("synthetic-explicitly-stored-value", false));
    success(&fixture.run(&["credential", "check", "fixture"]));
}

#[test]
#[ignore = "opens system unlock windows for a disposable Keychain; requires explicit local UI verification"]
fn system_unlock_resumes_read_and_cancellation_remains_explicit() {
    let fixture = Fixture::new();
    success(&fixture.setup(false));
    success(&fixture.store("synthetic-unlock-fixture", false));
    for should_unlock in [true, false] {
        assert_eq!(
            unsafe { SecKeychainLock(fixture.keychain.as_concrete_TypeRef()) },
            0
        );
        let mut command = fixture.command(&["credential", "check", "fixture"]);
        command
            .env_remove("AGENTENV_TEST_KEYCHAIN_NO_UI")
            .env("HOME", std::env::var_os("HOME").expect("real desktop home"));
        println!(
            "System unlock fixture phase: {}. Keychain: {}",
            if should_unlock {
                "complete unlock"
            } else {
                "cancel unlock"
            },
            fixture.keychain_path.display()
        );
        let result = command.output().unwrap();
        if should_unlock {
            success(&result);
            let status = fixture.run(&["keychain", "status", "--json"]);
            success(&status);
            assert_eq!(
                serde_json::from_slice::<serde_json::Value>(&status.stdout).unwrap()["keychain"]
                    ["unlocked"],
                true
            );
        } else {
            assert_eq!(result.status.code(), Some(4));
            assert!(String::from_utf8_lossy(&result.stderr).contains("cancelled"));
            let status = fixture.run(&["keychain", "status", "--json"]);
            success(&status);
            assert_eq!(
                serde_json::from_slice::<serde_json::Value>(&status.stdout).unwrap()["keychain"]
                    ["unlocked"],
                false
            );
        }
    }
}
