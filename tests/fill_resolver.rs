//! The confidential resolver's fill stage: environment-usage gate, fill
//! limits, single-line validation, exact preservation, and cancellation.
#![cfg(unix)]

use std::path::Path;
use std::time::Duration;

use agentenv::config::{CredentialDef, CredentialUsage, Provider};
use agentenv::credential::resolver::{resolve_detailed, ResolutionStage, ResolveError};
use agentenv::credential::FILL_VALUE_LIMIT;

fn definition(script: &str, usages: Vec<CredentialUsage>) -> CredentialDef {
    CredentialDef {
        name: "lab".into(),
        description: "Synthetic filling fixture".into(),
        inject_as: None,
        usages,
        provider: Provider::Command {
            argv: vec!["/bin/sh".into(), "-c".into(), script.into()],
        },
    }
}

fn fillable(script: &str) -> CredentialDef {
    definition(script, vec![CredentialUsage::Environment])
}

async fn fill_lookup(
    definition: &CredentialDef,
    timeout: Duration,
) -> Result<agentenv::credential::Secret, ResolveError> {
    resolve_detailed(
        Path::new(env!("CARGO_BIN_EXE_agentenv")),
        definition,
        ResolutionStage::Fill,
        FILL_VALUE_LIMIT,
        timeout,
    )
    .await
}

#[tokio::test]
async fn fill_stage_preserves_spaces_unicode_and_long_values_exactly() {
    for value in [
        "  leading and trailing  ".to_owned(),
        "pässwörd-日本語-🔑-\"quoted\"-<b>&amp;</b>-\\slash".to_owned(),
        "a".repeat(FILL_VALUE_LIMIT),
        "é".repeat(FILL_VALUE_LIMIT / 2),
    ] {
        let script = format!("printf '%s' '{value}'");
        let secret = fill_lookup(&fillable(&script), Duration::from_secs(5))
            .await
            .unwrap();
        assert_eq!(format!("{secret:?}"), "Secret(<redacted>)");
        assert!(secret.validate_fill(value.len()).is_ok());
        assert!(secret.validate_fill(value.len() - 1).is_err());
    }
}

#[tokio::test]
async fn fill_stage_rejects_values_the_destination_cannot_accept() {
    for script in [
        "printf 'line one\\nline two'",
        "printf 'tab\\there'",
        "printf 'cr\\rhere'",
        "printf 'esc\\033here'",
        "head -c 8193 /dev/zero | tr '\\000' a",
    ] {
        assert_eq!(
            fill_lookup(&fillable(script), Duration::from_secs(5))
                .await
                .unwrap_err(),
            ResolveError::Value,
            "{script}"
        );
    }
    // Command output is line-oriented: exactly one trailing line ending is
    // removed, a second one is part of the value and is rejected.
    let secret = fill_lookup(&fillable("printf 'value\\n'"), Duration::from_secs(5))
        .await
        .unwrap();
    assert!(secret.validate_fill(5).is_ok());
    let secret = fill_lookup(&fillable("printf 'value\\r\\n'"), Duration::from_secs(5))
        .await
        .unwrap();
    assert!(secret.validate_fill(5).is_ok());
    assert_eq!(
        fill_lookup(&fillable("printf 'value\\n\\n'"), Duration::from_secs(5))
            .await
            .unwrap_err(),
        ResolveError::Value
    );
}

#[tokio::test]
async fn provider_failures_are_reported_separately_from_value_rejection() {
    // Output that is not a usable value at all is a provider failure, as it
    // is for ordinary resolution; only a valid value the destination cannot
    // accept is a value rejection.
    for script in [
        "printf 'SENTINEL-CANDIDATE' >&2; exit 1",
        "printf ''",
        "printf '\\n'",
        "printf '\\377'",
        "printf 'a\\000b'",
    ] {
        let error = fill_lookup(&fillable(script), Duration::from_secs(5))
            .await
            .unwrap_err();
        assert_eq!(error, ResolveError::Provider, "{script}");
        assert!(!error.to_string().contains("SENTINEL"));
    }
    assert_eq!(
        fill_lookup(
            &fillable("head -c 20000 /dev/zero | tr '\\000' a"),
            Duration::from_secs(5)
        )
        .await
        .unwrap_err(),
        ResolveError::Value
    );
}

#[tokio::test]
async fn usage_gates_hold_in_both_directions_before_provider_execution() {
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("provider-ran");
    let touch = Provider::Command {
        argv: vec!["/usr/bin/touch".into(), marker.to_str().unwrap().into()],
    };

    let mut authentication_only = fillable("exit 1");
    authentication_only.usages = vec![CredentialUsage::Sudo, CredentialUsage::SshPassword];
    authentication_only.provider = touch.clone();
    assert_eq!(
        fill_lookup(&authentication_only, Duration::from_secs(5))
            .await
            .unwrap_err(),
        ResolveError::NotPermitted
    );

    let mut environment_only = fillable("exit 1");
    environment_only.provider = touch;
    for stage in [ResolutionStage::Sudo, ResolutionStage::SshPassword] {
        assert_eq!(
            resolve_detailed(
                Path::new(env!("CARGO_BIN_EXE_agentenv")),
                &environment_only,
                stage,
                255,
                Duration::from_secs(5),
            )
            .await
            .unwrap_err(),
            ResolveError::NotPermitted
        );
    }
    assert!(!marker.exists());

    // Authentication stages keep their 255-byte limit even when asked for more.
    let mut sudo_credential = fillable("printf 'x'");
    sudo_credential.usages = vec![CredentialUsage::Sudo];
    assert_eq!(
        resolve_detailed(
            Path::new(env!("CARGO_BIN_EXE_agentenv")),
            &sudo_credential,
            ResolutionStage::Sudo,
            256,
            Duration::from_secs(5),
        )
        .await
        .unwrap_err(),
        ResolveError::NotPermitted
    );
}

#[tokio::test]
async fn fill_timeout_and_cancellation_reap_owned_resolvers() {
    let dir = tempfile::tempdir().unwrap();
    let pid_file = dir.path().join("resolver-pid");
    let mut slow = fillable("");
    slow.provider = Provider::Command {
        argv: vec![
            "/bin/sh".into(),
            "-c".into(),
            "printf '%s' \"$PPID\" >\"$1\"; sleep 30; printf late-value".into(),
            "resolver-fixture".into(),
            pid_file.to_str().unwrap().into(),
        ],
    };
    let task = tokio::spawn(async move { fill_lookup(&slow, Duration::from_secs(60)).await });
    let resolver_pid: i32 = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Ok(pid) = std::fs::read_to_string(&pid_file)
                .unwrap_or_default()
                .trim()
                .parse()
            {
                break pid;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    assert_eq!(unsafe { libc::kill(resolver_pid, 0) }, -1);
    assert_eq!(
        std::io::Error::last_os_error().raw_os_error(),
        Some(libc::ESRCH)
    );

    let started = std::time::Instant::now();
    assert_eq!(
        fill_lookup(
            &fillable("sleep 30; printf late-value"),
            Duration::from_millis(100)
        )
        .await
        .unwrap_err(),
        ResolveError::Timeout
    );
    assert!(started.elapsed() < Duration::from_secs(3));
    assert!(
        fill_lookup(&fillable("printf fresh-value"), Duration::from_secs(5))
            .await
            .is_ok()
    );
}
