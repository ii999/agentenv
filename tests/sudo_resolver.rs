#![cfg(unix)]

use std::path::Path;
use std::time::Duration;

use agentenv::config::{CredentialDef, CredentialUsage, Provider};
use agentenv::credential::resolver::{resolve, AuthenticationStage};
use agentenv::credential::CapturedSecret;

fn definition(script: &str) -> CredentialDef {
    CredentialDef {
        name: "lab".into(),
        description: "Synthetic authentication fixture".into(),
        inject_as: None,
        usages: vec![CredentialUsage::Sudo],
        provider: Provider::Command {
            argv: vec!["/bin/sh".into(), "-c".into(), script.into()],
        },
    }
}

async fn lookup(
    definition: &CredentialDef,
    timeout: Duration,
) -> Result<agentenv::credential::Secret, agentenv::error::AppError> {
    resolve(
        Path::new(env!("CARGO_BIN_EXE_agentenv")),
        definition,
        AuthenticationStage::Sudo,
        255,
        timeout,
    )
    .await
}

#[test]
fn authentication_values_preserve_bytes_and_enforce_byte_limits() {
    for (value, valid) in [
        (" leading and trailing ".to_owned(), true),
        ("é".repeat(127) + " ", true),
        ("é".repeat(128), false),
        ("a".repeat(255), true),
        ("a".repeat(256), false),
        ("a\nb".to_owned(), false),
        ("a\rb".to_owned(), false),
    ] {
        let secret = CapturedSecret::new(value.into_bytes())
            .into_secret()
            .unwrap();
        assert_eq!(secret.validate_authentication(255).is_ok(), valid);
    }
    for value in [vec![], vec![0], vec![255]] {
        assert!(CapturedSecret::new(value).into_secret().is_err());
    }
}

#[tokio::test]
async fn confidential_provider_has_no_input_and_does_not_forward_stderr() {
    let secret = lookup(
        &definition(
            "test -z \"$(cat)\" || exit 1; printf 'SENTINEL-STDERR' >&2; printf '  lab-value  '",
        ),
        Duration::from_secs(5),
    )
    .await
    .unwrap();
    assert_eq!(format!("{secret:?}"), "Secret(<redacted>)");
    // Spaces were retained: the value needs 13 bytes, not 9.
    assert!(secret.validate_authentication(12).is_err());
    assert!(secret.validate_authentication(13).is_ok());
}

#[tokio::test]
async fn provider_failure_and_invalid_values_return_fixed_diagnostics() {
    for script in [
        "printf 'SENTINEL-CANDIDATE' >&2; exit 1",
        "printf 'SENTINEL-CANDIDATE\\n'",
        "printf '\\377'",
        "printf 'a\\000b'",
        "printf ''",
        "head -c 256 /dev/zero | tr '\\000' a",
    ] {
        let error = lookup(&definition(script), Duration::from_secs(5))
            .await
            .unwrap_err();
        assert_eq!(error.exit_code(), 4);
        assert!(!error.to_string().contains("SENTINEL"));
    }
}

#[tokio::test]
async fn purpose_mismatch_fails_before_provider_execution() {
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("provider-ran");
    let mut def = definition("exit 1");
    def.usages = vec![CredentialUsage::SshPassword];
    def.provider = Provider::Command {
        argv: vec!["/usr/bin/touch".into(), marker.to_str().unwrap().into()],
    };
    assert!(lookup(&def, Duration::from_secs(5)).await.is_err());
    assert!(!marker.exists());
}

#[tokio::test]
async fn timeout_and_task_cancellation_reap_owned_resolvers() {
    let dir = tempfile::tempdir().unwrap();
    let pid_file = dir.path().join("resolver-pid");
    let mut slow = definition("");
    slow.provider = Provider::Command {
        argv: vec![
            "/bin/sh".into(),
            "-c".into(),
            "printf '%s' \"$PPID\" >\"$1\"; sleep 30; printf late-value".into(),
            "resolver-fixture".into(),
            pid_file.to_str().unwrap().into(),
        ],
    };
    let task = tokio::spawn(async move { lookup(&slow, Duration::from_secs(60)).await });
    tokio::time::timeout(Duration::from_secs(5), async {
        while !pid_file.exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let resolver_pid: i32 = std::fs::read_to_string(&pid_file).unwrap().parse().unwrap();
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    assert_eq!(unsafe { libc::kill(resolver_pid, 0) }, -1);
    assert_eq!(
        std::io::Error::last_os_error().raw_os_error(),
        Some(libc::ESRCH)
    );

    let started = std::time::Instant::now();
    assert!(lookup(
        &definition("sleep 30; printf late-value"),
        Duration::from_millis(100)
    )
    .await
    .is_err());
    assert!(started.elapsed() < Duration::from_secs(3));
    // A late response or previous cancellation cannot contaminate a new session.
    assert!(
        lookup(&definition("printf fresh-value"), Duration::from_secs(5))
            .await
            .is_ok()
    );
}

#[test]
fn internal_mode_without_private_socket_is_silent_and_fails() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_agentenv"))
        .arg("--agentenv-resolver")
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(output.stderr.is_empty());
}
