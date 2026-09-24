//! Native Windows private-channel and process-lifetime regression coverage.
//! Every value and process belongs to a disposable synthetic fixture.
#![cfg(windows)]

use agentenv::config::{CredentialDef, CredentialUsage, Provider};
use agentenv::credential::resolver::{resolve_detailed, ResolutionStage, ResolveError};
use agentenv::credential::{CapturedSecret, FILL_VALUE_LIMIT};
use agentenv::sudo::ssh_askpass::Session;
use std::path::Path;
use std::time::Duration;

fn definition(args: &[&str], usage: CredentialUsage) -> CredentialDef {
    let mut argv = vec![
        env!("CARGO_BIN_EXE_test-probe").to_owned(),
        "--resolver-fixture".into(),
    ];
    argv.extend(args.iter().map(|arg| (*arg).to_owned()));
    CredentialDef {
        name: "synthetic".into(),
        description: "Disposable test value".into(),
        provider: Provider::Command { argv },
        inject_as: None,
        usages: vec![usage],
    }
}

async fn lookup(
    args: &[&str],
    stage: ResolutionStage,
) -> Result<agentenv::credential::Secret, ResolveError> {
    let (usage, limit) = match stage {
        ResolutionStage::Fill => (CredentialUsage::Environment, FILL_VALUE_LIMIT),
        ResolutionStage::Sudo => (CredentialUsage::Sudo, 255),
        ResolutionStage::SshPassword => (CredentialUsage::SshPassword, 255),
    };
    resolve_detailed(
        Path::new(env!("CARGO_BIN_EXE_agentenv")),
        &definition(args, usage),
        stage,
        limit,
        Duration::from_secs(10),
    )
    .await
}

#[tokio::test]
async fn resolver_supports_all_stages_with_stage_specific_limits() {
    for stage in [
        ResolutionStage::Fill,
        ResolutionStage::Sudo,
        ResolutionStage::SshPassword,
    ] {
        let secret = lookup(&["value", " 密码🔑 "], stage).await.unwrap();
        assert_eq!(format!("{secret:?}"), "Secret(<redacted>)");
        assert!(secret.validate_fill(" 密码🔑 ".len()).is_ok());
        assert!(secret.validate_fill(" 密码🔑 ".len() - 1).is_err());
    }
    assert!(lookup(&["repeat", "8192"], ResolutionStage::Fill)
        .await
        .is_ok());
    assert_eq!(
        lookup(&["repeat", "8193"], ResolutionStage::Fill)
            .await
            .unwrap_err(),
        ResolveError::Value
    );
    assert_eq!(
        lookup(&["repeat", "256"], ResolutionStage::Sudo)
            .await
            .unwrap_err(),
        ResolveError::Value
    );
    assert!(lookup(&["value", "value\r\n"], ResolutionStage::Fill)
        .await
        .is_ok());
    assert_eq!(
        lookup(&["value", "value\r\n"], ResolutionStage::SshPassword)
            .await
            .unwrap_err(),
        ResolveError::Value
    );
}

#[tokio::test]
async fn invalid_values_and_provider_errors_stay_distinct_and_redacted() {
    for value in ["a\tb", "a\nb", "a\u{202e}b", "value\n\n"] {
        assert_eq!(
            lookup(&["value", value], ResolutionStage::Fill)
                .await
                .unwrap_err(),
            ResolveError::Value
        );
    }
    for args in [&["failure"][..], &["value", ""][..]] {
        let error = lookup(args, ResolutionStage::Fill).await.unwrap_err();
        assert_eq!(error, ResolveError::Provider);
        assert!(!error.to_string().contains("SYNTHETIC"));
    }
    let unauthorized = definition(&["failure"], CredentialUsage::Sudo);
    assert_eq!(
        resolve_detailed(
            Path::new(env!("CARGO_BIN_EXE_agentenv")),
            &unauthorized,
            ResolutionStage::Fill,
            FILL_VALUE_LIMIT,
            Duration::from_secs(10)
        )
        .await
        .unwrap_err(),
        ResolveError::NotPermitted
    );
}

#[tokio::test]
async fn cancelling_resolution_terminates_provider_descendants() {
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use windows_sys::Win32::Foundation::WAIT_OBJECT_0;
    use windows_sys::Win32::System::Threading::{
        OpenProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE,
    };
    let dir = tempfile::tempdir().unwrap();
    let pid_file = dir.path().join("descendant.pid");
    let definition = definition(
        &["descendant", pid_file.to_str().unwrap()],
        CredentialUsage::Environment,
    );
    let task = tokio::spawn(async move {
        resolve_detailed(
            Path::new(env!("CARGO_BIN_EXE_agentenv")),
            &definition,
            ResolutionStage::Fill,
            FILL_VALUE_LIMIT,
            Duration::from_secs(30),
        )
        .await
    });
    let pid = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let Ok(text) = std::fs::read_to_string(&pid_file) {
                if let Ok(pid) = text.parse::<u32>() {
                    break pid;
                }
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    let handle = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) };
    assert!(!handle.is_null());
    let handle = unsafe { OwnedHandle::from_raw_handle(handle) };
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    assert_eq!(
        unsafe { WaitForSingleObject(handle.as_raw_handle(), 2000) },
        WAIT_OBJECT_0
    );
}

#[tokio::test]
async fn resolver_timeout_closes_the_channel() {
    let dir = tempfile::tempdir().unwrap();
    let pid = dir.path().join("provider.pid");
    let definition = definition(
        &["sleep", pid.to_str().unwrap()],
        CredentialUsage::Environment,
    );
    let started = std::time::Instant::now();
    let result = resolve_detailed(
        Path::new(env!("CARGO_BIN_EXE_agentenv")),
        &definition,
        ResolutionStage::Fill,
        FILL_VALUE_LIMIT,
        Duration::from_millis(500),
    )
    .await;
    assert_eq!(result.unwrap_err(), ResolveError::Timeout);
    assert!(started.elapsed() < Duration::from_secs(5));
}

#[tokio::test]
async fn resolver_exiting_before_it_connects_fails_promptly_as_a_provider_error() {
    // test-probe is not agentenv: it exits without ever opening the pipe.
    let started = std::time::Instant::now();
    let result = resolve_detailed(
        Path::new(env!("CARGO_BIN_EXE_test-probe")),
        &definition(&["value", "unused"], CredentialUsage::Environment),
        ResolutionStage::Fill,
        FILL_VALUE_LIMIT,
        Duration::from_secs(10),
    )
    .await;
    assert_eq!(result.unwrap_err(), ResolveError::Provider);
    assert!(started.elapsed() < Duration::from_secs(5));
}

#[tokio::test]
async fn askpass_accepts_only_the_bound_process_and_prompt() {
    for good in [true, false] {
        let session = Session::create(
            Path::new(env!("CARGO_BIN_EXE_agentenv-ssh-askpass")),
            Duration::from_secs(10),
        )
        .await
        .unwrap();
        let expected = "deploy@fixture's password: ";
        let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_test-probe"));
        command
            .args([
                "--askpass-fixture",
                if good {
                    expected
                } else {
                    "unrecognized prompt"
                },
            ])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);
        session.configure(&mut command);
        let child = command.spawn().unwrap();
        let pid = child.id().unwrap();
        let called = std::sync::atomic::AtomicBool::new(false);
        let broker = session.serve(
            pid,
            expected,
            Duration::from_secs(10),
            Duration::from_secs(10),
            || async {
                called.store(true, std::sync::atomic::Ordering::SeqCst);
                Ok(CapturedSecret::new(b"synthetic-login".to_vec())
                    .into_secret()
                    .unwrap())
            },
        );
        let (result, output) = tokio::join!(broker, child.wait_with_output());
        let output = output.unwrap();
        assert_eq!(result.is_ok(), good);
        assert_eq!(called.load(std::sync::atomic::Ordering::SeqCst), good);
        assert_eq!(output.status.success(), good);
        if good {
            assert_eq!(output.stdout, b"synthetic-login\n");
        } else {
            assert!(output.stdout.is_empty());
        }
        assert!(output.stderr.is_empty());
    }
}

#[test]
fn internal_entries_refuse_standalone_secret_requests() {
    for (exe, argument) in [
        (env!("CARGO_BIN_EXE_agentenv"), "--agentenv-resolver"),
        (
            env!("CARGO_BIN_EXE_agentenv-ssh-askpass"),
            "deploy@fixture's password: ",
        ),
    ] {
        let output = std::process::Command::new(exe)
            .arg(argument)
            .stdin(std::process::Stdio::null())
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert!(output.stderr.is_empty());
    }
}

#[tokio::test]
async fn native_credential_manager_is_read_from_the_isolated_resolver() {
    // A unique synthetic item, removed even when an assertion fails. No
    // existing credential is read, overwritten, enumerated or deleted.
    struct Item(keyring::Entry);
    impl Drop for Item {
        fn drop(&mut self) {
            let _ = self.0.delete_credential();
        }
    }
    let service = format!(
        "agentenv.windows-test.{}.{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let item = Item(keyring::Entry::new(&service, "fixture").unwrap());
    let value = "Synthetic 密码🔑 ";
    item.0.set_password(value).unwrap();
    let definition = CredentialDef {
        name: "native-test".into(),
        description: "Synthetic isolated access".into(),
        provider: Provider::Keychain {
            service,
            account: "fixture".into(),
        },
        inject_as: None,
        usages: vec![CredentialUsage::Environment],
    };
    let secret = resolve_detailed(
        Path::new(env!("CARGO_BIN_EXE_agentenv")),
        &definition,
        ResolutionStage::Fill,
        FILL_VALUE_LIMIT,
        Duration::from_secs(10),
    )
    .await
    .unwrap();
    assert!(secret.validate_fill(value.len()).is_ok());
    assert!(secret.validate_fill(value.len() - 1).is_err());
}
