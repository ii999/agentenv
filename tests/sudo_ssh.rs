//! SSH policy preparation against the local OpenSSH client.
//!
//! Unix route-policy cases. Native Windows OpenSSH is exercised by
//! tests/windows_lab/ssh.py; its platform boundary rejects MSYS/Cygwin ssh.
#![cfg(unix)]

use std::path::PathBuf;
use std::time::{Duration, Instant};

use agentenv::config::{
    CredentialRef, SshAuth, SshConnection, SshTarget, SudoTarget, SudoTransport,
};
use agentenv::sudo::ssh::{prepare, PreparedAuth};
use tempfile::TempDir;

static ENVIRONMENT_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

async fn lock_environment() -> tokio::sync::MutexGuard<'static, ()> {
    ENVIRONMENT_LOCK.lock().await
}

fn reference(name: &str) -> CredentialRef {
    CredentialRef {
        name: name.to_owned(),
        target_override: None,
    }
}

fn explicit_target(known_hosts_file: PathBuf) -> SudoTarget {
    SudoTarget {
        description: "Explicit SSH target".to_owned(),
        credential: reference("sudo-password"),
        auth_user: "deploy".to_owned(),
        run_as: "root".to_owned(),
        sudo_path: PathBuf::from("/usr/bin/sudo"),
        transport: SudoTransport::Ssh(SshTarget {
            host_key_alias: "agentenv-prod".to_owned(),
            known_hosts_file,
            helper_path: PathBuf::from("/home/deploy/.local/libexec/agentenv-sudo-helper"),
            connection: SshConnection::Explicit {
                hostname: "203.0.113.10".to_owned(),
                user: "deploy".to_owned(),
                port: 2222,
            },
            auth: SshAuth::Password {
                credential: reference("login-password"),
            },
        }),
    }
}

fn explicit_public_key_target(
    known_hosts_file: PathBuf,
    identity_files: Vec<PathBuf>,
    use_agent: bool,
) -> SudoTarget {
    let mut target = explicit_target(known_hosts_file);
    let SudoTransport::Ssh(ssh) = &mut target.transport else {
        unreachable!()
    };
    ssh.auth = SshAuth::PublicKey {
        identity_files,
        use_agent,
    };
    target
}

fn config_target(config_file: PathBuf, known_hosts_file: PathBuf) -> SudoTarget {
    SudoTarget {
        description: "Configured SSH target".to_owned(),
        credential: reference("sudo-password"),
        auth_user: "deploy".to_owned(),
        run_as: "root".to_owned(),
        sudo_path: PathBuf::from("/usr/bin/sudo"),
        transport: SudoTransport::Ssh(SshTarget {
            host_key_alias: "agentenv-prod".to_owned(),
            known_hosts_file,
            helper_path: PathBuf::from("/home/deploy/.local/libexec/agentenv-sudo-helper"),
            connection: SshConnection::Config {
                host_alias: "prod".to_owned(),
                config_file: Some(config_file),
            },
            auth: SshAuth::PublicKey {
                identity_files: Vec::new(),
                use_agent: false,
            },
        }),
    }
}

fn write_config(directory: &TempDir, body: &str) -> PathBuf {
    let path = directory.path().join("ssh_config");
    std::fs::write(&path, body).expect("SSH config fixture writes");
    path
}

#[tokio::test(flavor = "current_thread")]
async fn explicit_password_mode_is_config_free_pinned_and_nonconnecting() {
    let _lock = lock_environment().await;
    let directory = TempDir::new().expect("temp directory");
    let known_hosts = directory.path().join("known hosts");
    let prepared = prepare(&explicit_target(known_hosts), Duration::from_secs(5))
        .await
        .expect("explicit policy prepares without connecting");

    assert_eq!(prepared.effective_host, "203.0.113.10");
    assert_eq!(prepared.effective_user, "deploy");
    assert_eq!(prepared.effective_port, 2222);
    assert!(matches!(
        prepared.auth,
        PreparedAuth::Password {
            ref credential,
            ref expected_prompt,
        } if credential.name == "login-password"
            && expected_prompt == "deploy@agentenv-prod's password: "
    ));
    let arguments = prepared
        .arguments
        .iter()
        .map(|argument| argument.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    assert_eq!(arguments[0..2], ["-F", "none"]);
    assert_eq!(
        &arguments[arguments.len() - 2..],
        [
            "203.0.113.10",
            "/home/deploy/.local/libexec/agentenv-sudo-helper --serve",
        ]
    );
    assert!(arguments
        .windows(2)
        .any(|pair| pair == ["-o", "BatchMode=no"]));
    assert!(arguments
        .windows(2)
        .any(|pair| pair == ["-o", "NumberOfPasswordPrompts=1"]));
    assert!(arguments
        .windows(2)
        .any(|pair| pair == ["-o", "RemoteCommand=none"]));
    assert!(!arguments
        .iter()
        .any(|argument| argument.contains("credential://")));
}

#[tokio::test(flavor = "current_thread")]
async fn explicit_agent_only_mode_clears_implicit_files_without_excluding_agent_keys() {
    let _lock = lock_environment().await;
    let directory = TempDir::new().expect("temp directory");
    let prepared = prepare(
        &explicit_public_key_target(directory.path().join("known_hosts"), Vec::new(), true),
        Duration::from_secs(5),
    )
    .await
    .expect("agent-only policy prepares");
    let arguments = prepared
        .arguments
        .iter()
        .map(|argument| argument.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    assert!(arguments
        .windows(2)
        .any(|pair| pair == ["-o", "IdentityFile=none"]));
    assert!(arguments
        .windows(2)
        .any(|pair| pair == ["-o", "IdentitiesOnly=no"]));
    assert!(!arguments
        .windows(2)
        .any(|pair| pair == ["-o", "IdentityAgent=none"]));
}

#[tokio::test(flavor = "current_thread")]
async fn final_policy_disables_dns_and_command_host_key_sources() {
    let _lock = lock_environment().await;
    let directory = TempDir::new().expect("temp directory");
    let config = write_config(
        &directory,
        r#"Host prod
    HostName 203.0.113.10
    User deploy
    Port 2222
    VerifyHostKeyDNS yes
    KnownHostsCommand /usr/bin/true
"#,
    );
    let prepared = prepare(
        &config_target(config, directory.path().join("known_hosts")),
        Duration::from_secs(5),
    )
    .await
    .expect("constructed options disable alternate trust sources");
    let arguments = prepared
        .arguments
        .iter()
        .map(|argument| argument.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    assert!(arguments
        .windows(2)
        .any(|pair| pair == ["-o", "VerifyHostKeyDNS=no"]));
    assert!(arguments
        .windows(2)
        .any(|pair| pair == ["-o", "KnownHostsCommand=none"]));
}

#[tokio::test(flavor = "current_thread")]
async fn alias_endpoint_is_resolved_then_pinned_with_safe_environment_exports() {
    let _lock = lock_environment().await;
    let directory = TempDir::new().expect("temp directory");
    let config = write_config(
        &directory,
        r#"Host prod
    HostName 203.0.113.10
    User deploy
    Port 2222
    SendEnv LANG LC_* TZ
"#,
    );
    let prepared = prepare(
        &config_target(config, directory.path().join("known_hosts")),
        Duration::from_secs(5),
    )
    .await
    .expect("safe alias policy prepares");
    assert_eq!(prepared.effective_host, "203.0.113.10");
    assert_eq!(prepared.effective_user, "deploy");
    assert_eq!(prepared.effective_port, 2222);
    assert_eq!(
        prepared.arguments[prepared.arguments.len() - 2],
        std::ffi::OsString::from("prod"),
        "the alias remains the config-selection destination"
    );
    assert!(prepared
        .arguments
        .iter()
        .any(|argument| { argument.to_string_lossy().contains("HostName=203.0.113.10") }));
}

#[tokio::test(flavor = "current_thread")]
async fn password_routes_and_metadata_exports_fail_before_connection() {
    let _lock = lock_environment().await;
    let directory = TempDir::new().expect("temp directory");
    for (extra, reason) in [
        ("ProxyJump jump\n", "password-route-unsupported"),
        ("SetEnv SAFE=value\n", "ssh-policy-rejected"),
        ("SendEnv AGENTENV_SSH_SESSION\n", "ssh-policy-rejected"),
    ] {
        let config = write_config(
            &directory,
            &format!(
                "Host prod\n    HostName 203.0.113.10\n    User deploy\n    Port 2222\n    {extra}"
            ),
        );
        let mut target = config_target(config, directory.path().join("known_hosts"));
        let SudoTransport::Ssh(ssh) = &mut target.transport else {
            unreachable!()
        };
        ssh.auth = SshAuth::Password {
            credential: reference("login-password"),
        };
        let error = prepare(&target, Duration::from_secs(5))
            .await
            .expect_err("unsafe password route is rejected");
        assert!(error.to_string().contains(reason), "{error}");
    }
}

#[cfg(unix)]
#[tokio::test(flavor = "current_thread")]
async fn policy_probe_timeout_kills_the_owned_child() {
    use std::os::unix::fs::PermissionsExt;

    let _lock = lock_environment().await;
    let directory = TempDir::new().expect("temp directory");
    let fake = directory.path().join("ssh");
    let pid_file = directory.path().join("probe.pid");
    std::fs::write(
        &fake,
        format!(
            "#!/bin/sh\necho $$ > '{}'\nwhile :; do :; done\n",
            pid_file.display()
        ),
    )
    .expect("fake SSH writes");
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755))
        .expect("fake SSH is executable");
    let original_path = std::env::var_os("PATH");
    std::env::set_var("PATH", directory.path());
    let result = prepare(
        &explicit_target(directory.path().join("known_hosts")),
        Duration::from_secs(1),
    )
    .await;
    restore_environment("PATH", original_path.as_deref());
    let error = result.expect_err("hung policy probe times out");
    assert!(error.to_string().contains("ssh-setup-timeout"), "{error}");
    let pid = std::fs::read_to_string(pid_file)
        .expect("probe published its PID")
        .trim()
        .parse::<i32>()
        .expect("PID is numeric");
    assert_eq!(unsafe { libc::kill(pid, 0) }, -1, "probe child was reaped");
}

#[cfg(unix)]
#[tokio::test(flavor = "current_thread")]
async fn oversized_stdout_aborts_and_joins_a_stderr_reader_held_by_a_descendant() {
    use std::os::unix::fs::PermissionsExt;

    let _lock = lock_environment().await;
    let directory = TempDir::new().expect("temp directory");
    let fake = directory.path().join("ssh");
    let descendant_pid = directory.path().join("descendant.pid");
    std::fs::write(
        &fake,
        format!(
            "#!/bin/sh\n(/bin/sleep 30) >&2 &\necho $! > '{}'\n/usr/bin/head -c 1100000 /dev/zero\nexit 0\n",
            descendant_pid.display()
        ),
    )
    .expect("fake SSH writes");
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755))
        .expect("fake SSH is executable");
    let original_path = std::env::var_os("PATH");
    std::env::set_var("PATH", directory.path());
    let started = Instant::now();
    let result = prepare(
        &explicit_target(directory.path().join("known_hosts")),
        Duration::from_secs(5),
    )
    .await;
    restore_environment("PATH", original_path.as_deref());
    let error = result.expect_err("oversized policy output is rejected");
    assert!(
        error.to_string().contains("ssh-probe-output-limit"),
        "{error}"
    );
    assert!(started.elapsed() < Duration::from_secs(2));
    let pid = std::fs::read_to_string(descendant_pid)
        .expect("descendant published its PID")
        .trim()
        .parse::<i32>()
        .expect("PID is numeric");
    let _ = unsafe { libc::kill(pid, libc::SIGKILL) };
}

fn restore_environment(name: &str, value: Option<&std::ffi::OsStr>) {
    match value {
        Some(value) => std::env::set_var(name, value),
        None => std::env::remove_var(name),
    }
}

#[tokio::test(flavor = "current_thread")]
async fn deployment_sessions_reuse_the_route_with_only_the_remote_command_replaced() {
    let _lock = lock_environment().await;
    let directory = TempDir::new().expect("temp directory");
    let prepared = prepare(
        &explicit_target(directory.path().join("known_hosts")),
        Duration::from_secs(5),
    )
    .await
    .expect("explicit policy prepares without connecting");
    assert_eq!(
        prepared.helper_path(),
        "/home/deploy/.local/libexec/agentenv-sudo-helper"
    );
    let serve_command = prepared.command();
    let preflight_command = prepared.command_for("sh -c 'uname -s' agentenv-preflight '/x'");
    let serve: Vec<String> = serve_command
        .as_std()
        .get_args()
        .map(|argument| argument.to_string_lossy().into_owned())
        .collect();
    let preflight: Vec<String> = preflight_command
        .as_std()
        .get_args()
        .map(|argument| argument.to_string_lossy().into_owned())
        .collect();
    assert_eq!(serve.len(), preflight.len());
    assert_eq!(serve[..serve.len() - 1], preflight[..preflight.len() - 1]);
    assert_eq!(
        serve.last().map(String::as_str),
        Some("/home/deploy/.local/libexec/agentenv-sudo-helper --serve")
    );
    assert_eq!(
        preflight.last().map(String::as_str),
        Some("sh -c 'uname -s' agentenv-preflight '/x'")
    );
    assert_eq!(
        preflight_command.as_std().get_envs().collect::<Vec<_>>(),
        serve_command.as_std().get_envs().collect::<Vec<_>>(),
        "the curated environment is identical for every session"
    );
}
