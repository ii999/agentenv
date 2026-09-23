#![cfg(unix)]

use agentenv::credential::CapturedSecret;
use agentenv::sudo::ssh_askpass::Session;
use std::process::Stdio;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use std::time::Duration;
use tokio::process::Command;

fn helper() -> &'static std::path::Path {
    std::path::Path::new(env!("CARGO_BIN_EXE_agentenv-ssh-askpass"))
}

#[tokio::test]
async fn response_preserves_bytes_and_second_challenge_is_closed() {
    let session = Session::create(helper(), Duration::from_secs(2))
        .await
        .unwrap();
    let timing = session.auth_timing();
    let prompt = "test@host's password: ";
    let command = || {
        let mut command = Command::new(helper());
        session.configure(&mut command);
        command
            .arg(prompt)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        command
    };
    let first = command().spawn().unwrap();
    let mut second = command();
    let count = Arc::new(AtomicUsize::new(0));
    let calls = count.clone();
    session
        .serve(
            std::process::id(),
            prompt,
            Duration::from_secs(2),
            Duration::from_secs(2),
            move || async move {
                calls.fetch_add(1, Ordering::SeqCst);
                Ok(CapturedSecret::new("  opaque-é  ".as_bytes().to_vec())
                    .into_secret()
                    .unwrap())
            },
        )
        .await
        .unwrap();
    let output = first.wait_with_output().await.unwrap();
    assert!(output.status.success());
    assert_eq!(output.stdout, "  opaque-é  \n".as_bytes());
    assert!(output.stderr.is_empty());
    assert!(timing.borrow().as_ref().unwrap().1.is_some());
    let output = second.output().await.unwrap();
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(output.stderr.is_empty());
    assert_eq!(count.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn wrong_prompt_or_wrong_parent_never_resolves() {
    for wrong_parent in [false, true] {
        let session = Session::create(helper(), Duration::from_secs(2))
            .await
            .unwrap();
        let count = Arc::new(AtomicUsize::new(0));
        let calls = count.clone();
        let mut command = Command::new(helper());
        session.configure(&mut command);
        let child = command
            .arg(if wrong_parent {
                "test@host's password: "
            } else {
                "Are you sure?"
            })
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let result = session
            .serve(
                if wrong_parent { 0 } else { std::process::id() },
                "test@host's password: ",
                Duration::from_secs(2),
                Duration::from_secs(2),
                move || async move {
                    calls.fetch_add(1, Ordering::SeqCst);
                    Ok(CapturedSecret::new(b"test-only-value".to_vec())
                        .into_secret()
                        .unwrap())
                },
            )
            .await;
        assert_eq!(result.unwrap_err().exit_code(), 9);
        let output = child.wait_with_output().await.unwrap();
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert!(output.stderr.is_empty());
        assert_eq!(count.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test]
async fn every_login_value_is_validated_before_any_reply() {
    for value in [b"with\nline".to_vec(), vec![b'x'; 256]] {
        let session = Session::create(helper(), Duration::from_secs(2))
            .await
            .unwrap();
        let mut command = Command::new(helper());
        session.configure(&mut command);
        let child = command
            .arg("test@host's password: ")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let result = session
            .serve(
                std::process::id(),
                "test@host's password: ",
                Duration::from_secs(2),
                Duration::from_secs(2),
                move || async move { Ok(CapturedSecret::new(value).into_secret().unwrap()) },
            )
            .await;
        assert_eq!(result.unwrap_err().exit_code(), 4);
        let output = child.wait_with_output().await.unwrap();
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert!(output.stderr.is_empty());
    }
}

#[tokio::test]
async fn confirmation_hint_and_missing_channel_fail_silently() {
    for hint in [None, Some("confirm"), Some("none")] {
        let mut command = Command::new(helper());
        command.env_clear().arg("test@host's password: ");
        if let Some(hint) = hint {
            command.env("SSH_ASKPASS_PROMPT", hint);
        }
        let output = command.output().await.unwrap();
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert!(output.stderr.is_empty());
    }
}

#[tokio::test]
async fn cancelling_resolution_revokes_the_login_channel() {
    let session = Session::create(helper(), Duration::from_secs(2))
        .await
        .unwrap();
    let mut command = Command::new(helper());
    session.configure(&mut command);
    let child = command
        .arg("test@host's password: ")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let (started, pending) = tokio::sync::oneshot::channel();
    let operation = tokio::spawn(session.serve(
        std::process::id(),
        "test@host's password: ",
        Duration::from_secs(2),
        Duration::from_secs(2),
        move || async move {
            let _ = started.send(());
            std::future::pending().await
        },
    ));
    pending.await.unwrap();
    operation.abort();
    assert!(operation.await.unwrap_err().is_cancelled());
    let output = tokio::time::timeout(Duration::from_secs(2), child.wait_with_output())
        .await
        .unwrap()
        .unwrap();
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(output.stderr.is_empty());
}

#[tokio::test]
async fn non_utf8_prompt_is_rejected_without_a_panic_or_diagnostic() {
    use std::os::unix::ffi::OsStringExt;
    let output = Command::new(helper())
        .env_clear()
        .env("RUST_BACKTRACE", "1")
        .arg(std::ffi::OsString::from_vec(
            b"sentinel-\xff-prompt".to_vec(),
        ))
        .output()
        .await
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert!(output.stderr.is_empty());
}
