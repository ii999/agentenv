#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::process::Stdio;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use agentenv::credential::CapturedSecret;
use agentenv::sudo::{self, ExecutionRequest, LocalOptions, ProcessIo, PROTOCOL_VERSION};
use assert_cmd::Command;

fn account_name() -> String {
    let output = std::process::Command::new("/usr/bin/id")
        .arg("-un")
        .output()
        .expect("id is available");
    String::from_utf8(output.stdout)
        .expect("account is UTF-8")
        .trim_end()
        .to_owned()
}

fn helper_path() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_BIN_EXE_agentenv-sudo-helper"))
}

fn fake_sudo(source: &str) -> (tempfile::TempDir, std::path::PathBuf) {
    let directory = tempfile::tempdir().expect("temporary directory");
    let path = directory.path().join("sudo");
    fs::write(&path, source).expect("write fake sudo");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).expect("chmod fake sudo");
    (directory, path)
}

fn run_agentenv(config: &std::path::Path, arguments: &[&str]) -> std::process::Output {
    let mut command = Command::cargo_bin("agentenv").expect("agentenv binary");
    command
        .env_clear()
        .env("AGENTENV_FILE", config)
        .env("AGENTENV_NO_PROJECT", "1")
        .args(arguments)
        .output()
        .expect("run agentenv")
}

fn options(sudo_path: std::path::PathBuf) -> LocalOptions {
    LocalOptions {
        sudo_path,
        helper_path: helper_path(),
        auth_user: account_name(),
        run_as: "root".to_owned(),
        setup_timeout: Duration::from_secs(5),
        auth_timeout: Duration::from_secs(5),
    }
}

#[test]
fn request_rejects_relative_and_non_utf8_paths_and_is_immutable() {
    assert!(ExecutionRequest::new("bin/echo", Vec::new(), None).is_err());
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        let invalid = std::path::PathBuf::from(std::ffi::OsString::from_vec(vec![b'/', 0xff]));
        assert!(ExecutionRequest::new(invalid, Vec::new(), None).is_err());
    }
    let request = ExecutionRequest::new(
        "/bin/echo",
        vec!["literal $() ; newline\nargument".to_owned()],
        Some(std::path::PathBuf::from("/tmp")),
    )
    .expect("valid request");
    assert_eq!(request.executable(), std::path::Path::new("/bin/echo"));
    assert_eq!(request.arguments(), ["literal $() ; newline\nargument"]);
    assert_eq!(request.cwd(), Some(std::path::Path::new("/tmp")));
}

#[test]
fn helper_identity_is_exact_and_check_never_resolves_a_password() {
    let output = std::process::Command::new(helper_path())
        .arg("--identity")
        .output()
        .expect("run helper identity");
    assert_eq!(
        output.stdout,
        format!(
            "agentenv-sudo-helper {PROTOCOL_VERSION} {}\n",
            env!("CARGO_PKG_VERSION")
        )
        .as_bytes()
    );

    let (_directory, sudo_path) = fake_sudo("#!/bin/sh\nexit 0\n");
    let runtime = tokio::runtime::Runtime::new().expect("runtime");
    runtime
        .block_on(sudo::check_local(&options(sudo_path)))
        .expect("check succeeds without a provider");
}

#[test]
fn cli_plan_is_offline_and_remote_defaults_remain_unresolved() {
    let directory = tempfile::tempdir().expect("config tempdir");
    let marker = directory.path().join("provider-ran");
    let config = directory.path().join("agentenv.toml");
    let source = format!(
        r#"version = 1
default_profile = "work"
[profiles.work]
description = "Work."
[credentials.admin]
description = "Administrator password."
provider = "command"
argv = ["/bin/sh", "-c", "touch '{}' && printf secret"]
usages = ["sudo"]
[profiles.work.admin]
description = "Remote administrator."
kind = "sudo-target"
[profiles.work.admin.sudo]
transport = "ssh"
credential = "credential://admin"
auth_user = "deploy"
run_as = "root"
sudo_path = "/usr/bin/sudo"
[profiles.work.admin.sudo.ssh]
mode = "ssh-config"
host_alias = "production"
host_key_alias = "agentenv-production"
known_hosts_file = "/tmp/known-hosts"
helper_path = "/home/deploy/.local/libexec/agentenv-sudo-helper"
[profiles.work.admin.sudo.ssh.auth]
method = "publickey"
"#,
        marker.display()
    );
    fs::write(&config, source).expect("write config");
    let output = run_agentenv(
        &config,
        &[
            "--json",
            "sudo",
            "--with",
            "admin",
            "--plan",
            "--",
            "/usr/bin/id",
            "-u",
        ],
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let plan: serde_json::Value = serde_json::from_slice(&output.stdout).expect("plan JSON");
    assert_eq!(plan["transport"], "ssh");
    assert_eq!(plan["route_policy"], "unverified");
    assert_eq!(plan["cwd"], serde_json::Value::Null);
    assert!(plan["endpoint"]
        .as_str()
        .expect("endpoint")
        .contains("unresolved"));
    assert!(!marker.exists(), "offline plan must not run the provider");
}

#[test]
fn cli_check_does_not_resolve_and_rejects_command_specific_cwd() {
    let (sudo_directory, sudo_path) = fake_sudo("#!/bin/sh\nexit 0\n");
    let directory = tempfile::tempdir().expect("config tempdir");
    let marker = directory.path().join("provider-ran");
    let config = directory.path().join("agentenv.toml");
    let source = format!(
        r#"version = 1
default_profile = "work"
[profiles.work]
description = "Work."
[credentials.admin]
description = "Administrator password."
provider = "command"
argv = ["/bin/sh", "-c", "touch '{}' && printf secret"]
usages = ["sudo"]
[profiles.work.admin]
description = "Local administrator."
kind = "sudo-target"
[profiles.work.admin.sudo]
transport = "local"
credential = "credential://admin"
auth_user = "{}"
run_as = "root"
sudo_path = "{}"
"#,
        marker.display(),
        account_name(),
        sudo_path.display()
    );
    fs::write(&config, source).expect("write config");
    let output = run_agentenv(&config, &["--json", "sudo", "--with", "admin", "--check"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let result: serde_json::Value = serde_json::from_slice(&output.stdout).expect("check JSON");
    assert_eq!(result["sudo_authentication"], "not-attempted");
    assert!(!marker.exists(), "check must not run the provider");

    let rejected = run_agentenv(
        &config,
        &[
            "sudo",
            "--with",
            "admin",
            "--check",
            "--cwd",
            sudo_directory.path().to_str().expect("UTF-8 tempdir"),
        ],
    );
    assert_eq!(rejected.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&rejected.stderr).contains("does not accept --cwd"));
    assert!(!marker.exists(), "rejected check must not run the provider");
}

#[test]
fn nopasswd_preserves_binary_stdin_and_never_resolves() {
    let script = r#"#!/bin/sh
while [ "$#" -gt 0 ]; do
  case "$1" in
    -u|-p) shift 2 ;;
    -A|-k) shift ;;
    --) shift; break ;;
    *) exit 90 ;;
  esac
done
exec "$@"
"#;
    let (_directory, sudo_path) = fake_sudo(script);
    let input_dir = tempfile::tempdir().expect("input tempdir");
    let input_path = input_dir.path().join("input");
    let output_path = input_dir.path().join("output");
    let input = b"\0first\nsecond\xff\0";
    fs::write(&input_path, input).expect("write binary input");
    let io = ProcessIo {
        stdin: Stdio::from(fs::File::open(&input_path).expect("open input")),
        stdout: Stdio::from(fs::File::create(&output_path).expect("create output")),
        stderr: Stdio::null(),
    };
    let calls = Arc::new(AtomicUsize::new(0));
    let resolver_calls = Arc::clone(&calls);
    let (_cancel, cancellation) = sudo::cancellation_channel();
    let runtime = tokio::runtime::Runtime::new().expect("runtime");
    let outcome = runtime
        .block_on(sudo::execute_local(
            ExecutionRequest::new("/bin/cat", Vec::new(), None).expect("request"),
            options(sudo_path),
            io,
            cancellation,
            move || async move {
                resolver_calls.fetch_add(1, Ordering::SeqCst);
                CapturedSecret::new(b"unused".to_vec())
                    .into_secret()
                    .map_err(|_| agentenv::error::AppError::Credential("invalid".to_owned()))
            },
        ))
        .expect("nopasswd execution succeeds");
    assert_eq!(outcome.exit_code, Some(0));
    assert!(!outcome.password_delivered);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_eq!(fs::read(output_path).expect("read output"), input);
}

#[test]
fn valid_prompt_resolves_once_and_wrong_prompt_releases_no_secret() {
    let valid = r#"#!/bin/sh
prompt=""
while [ "$#" -gt 0 ]; do
  case "$1" in
    -p) prompt="$2"; shift 2 ;;
    -u) shift 2 ;;
    -A|-k) shift ;;
    --) shift; break ;;
    *) exit 90 ;;
  esac
done
auth=$(/usr/bin/id -un)
prefix=${prompt%\%p:}
password=$("$SUDO_ASKPASS" "${prefix}${auth}:") || exit 91
[ "$password" = "synthetic-secret" ] || exit 92
exec "$@"
"#;
    let (_directory, sudo_path) = fake_sudo(valid);
    let calls = Arc::new(AtomicUsize::new(0));
    let resolver_calls = Arc::clone(&calls);
    let (_cancel, cancellation) = sudo::cancellation_channel();
    let runtime = tokio::runtime::Runtime::new().expect("runtime");
    let outcome = runtime
        .block_on(sudo::execute_local(
            ExecutionRequest::new("/usr/bin/true", Vec::new(), None).expect("request"),
            options(sudo_path),
            ProcessIo {
                stdin: Stdio::null(),
                stdout: Stdio::null(),
                stderr: Stdio::null(),
            },
            cancellation,
            move || async move {
                resolver_calls.fetch_add(1, Ordering::SeqCst);
                CapturedSecret::new(b"synthetic-secret".to_vec())
                    .into_secret()
                    .map_err(|_| agentenv::error::AppError::Credential("invalid".to_owned()))
            },
        ))
        .expect("password execution succeeds");
    assert_eq!(outcome.exit_code, Some(0));
    assert!(outcome.password_delivered);
    assert_eq!(calls.load(Ordering::SeqCst), 1);

    let wrong = r#"#!/bin/sh
while [ "$#" -gt 0 ]; do
  case "$1" in
    -p) shift 2 ;;
    -u) shift 2 ;;
    -A|-k) shift ;;
    --) shift; break ;;
    *) exit 90 ;;
  esac
done
"$SUDO_ASKPASS" "untrusted prompt" >/dev/null 2>&1
exit 1
"#;
    let (_wrong_directory, wrong_sudo) = fake_sudo(wrong);
    let wrong_calls = Arc::new(AtomicUsize::new(0));
    let resolver_calls = Arc::clone(&wrong_calls);
    let (_cancel, cancellation) = sudo::cancellation_channel();
    let outcome = runtime
        .block_on(sudo::execute_local(
            ExecutionRequest::new("/usr/bin/true", Vec::new(), None).expect("request"),
            options(wrong_sudo),
            ProcessIo {
                stdin: Stdio::null(),
                stdout: Stdio::null(),
                stderr: Stdio::null(),
            },
            cancellation,
            move || async move {
                resolver_calls.fetch_add(1, Ordering::SeqCst);
                CapturedSecret::new(b"must-not-be-read".to_vec())
                    .into_secret()
                    .map_err(|_| agentenv::error::AppError::Credential("invalid".to_owned()))
            },
        ))
        .expect("the helper refuses before connecting and sudo reports its status");
    assert_eq!(outcome.exit_code, Some(1));
    assert_eq!(wrong_calls.load(Ordering::SeqCst), 0);
}

#[test]
fn a_second_password_challenge_fails_without_a_second_resolution() {
    let script = r#"#!/bin/sh
prompt=""
while [ "$#" -gt 0 ]; do
  case "$1" in
    -p) prompt="$2"; shift 2 ;;
    -u) shift 2 ;;
    -A|-k) shift ;;
    --) shift; break ;;
    *) exit 90 ;;
  esac
done
auth=$(/usr/bin/id -un)
prefix=${prompt%\%p:}
expanded="${prefix}${auth}:"
"$SUDO_ASKPASS" "$expanded" >/dev/null || exit 91
"$SUDO_ASKPASS" "$expanded" >/dev/null 2>&1 || exit 93
exit 94
"#;
    let (_directory, sudo_path) = fake_sudo(script);
    let calls = Arc::new(AtomicUsize::new(0));
    let resolver_calls = Arc::clone(&calls);
    let (_cancel, cancellation) = sudo::cancellation_channel();
    let runtime = tokio::runtime::Runtime::new().expect("runtime");
    let outcome = runtime
        .block_on(sudo::execute_local(
            ExecutionRequest::new("/usr/bin/true", Vec::new(), None).expect("request"),
            options(sudo_path),
            ProcessIo {
                stdin: Stdio::null(),
                stdout: Stdio::null(),
                stderr: Stdio::null(),
            },
            cancellation,
            move || async move {
                resolver_calls.fetch_add(1, Ordering::SeqCst);
                CapturedSecret::new(b"synthetic-secret".to_vec())
                    .into_secret()
                    .map_err(|_| agentenv::error::AppError::Credential("invalid".to_owned()))
            },
        ))
        .expect("second challenge terminates without hanging");
    assert_eq!(outcome.exit_code, Some(93));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[test]
fn prequeued_cancellation_starts_neither_helper_sudo_nor_provider() {
    let directory = tempfile::tempdir().expect("marker tempdir");
    let helper_marker = directory.path().join("helper-started");
    let sudo_marker = directory.path().join("sudo-started");
    let helper = directory.path().join("helper");
    let sudo_path = directory.path().join("sudo");
    fs::write(
        &helper,
        format!(
            "#!/bin/sh\n: > '{}'\nprintf 'agentenv-sudo-helper {PROTOCOL_VERSION} {}\\n'\n",
            helper_marker.display(),
            env!("CARGO_PKG_VERSION")
        ),
    )
    .expect("write helper");
    fs::write(
        &sudo_path,
        format!("#!/bin/sh\n: > '{}'\nexit 0\n", sudo_marker.display()),
    )
    .expect("write sudo");
    fs::set_permissions(&helper, fs::Permissions::from_mode(0o700)).expect("chmod helper");
    fs::set_permissions(&sudo_path, fs::Permissions::from_mode(0o700)).expect("chmod sudo");

    let mut options = options(sudo_path);
    options.helper_path = helper;
    let calls = Arc::new(AtomicUsize::new(0));
    let resolver_calls = Arc::clone(&calls);
    let (cancel, cancellation) = sudo::cancellation_channel();
    cancel.send(libc::SIGTERM).expect("queue cancellation");
    let runtime = tokio::runtime::Runtime::new().expect("runtime");
    let outcome = runtime
        .block_on(sudo::execute_local(
            ExecutionRequest::new("/usr/bin/true", Vec::new(), None).expect("request"),
            options,
            ProcessIo {
                stdin: Stdio::null(),
                stdout: Stdio::null(),
                stderr: Stdio::null(),
            },
            cancellation,
            move || async move {
                resolver_calls.fetch_add(1, Ordering::SeqCst);
                CapturedSecret::new(b"must-not-be-read".to_vec())
                    .into_secret()
                    .map_err(|_| agentenv::error::AppError::Credential("invalid".to_owned()))
            },
        ))
        .expect("queued cancellation is an observed outcome");
    assert_eq!(outcome.exit_code, None);
    assert_eq!(outcome.signal, Some(libc::SIGTERM));
    assert!(!outcome.password_delivered);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert!(!helper_marker.exists(), "helper identity must not start");
    assert!(!sudo_marker.exists(), "sudo must not start");
}

#[test]
fn cancellation_during_helper_identity_reaps_helper_without_starting_sudo() {
    let directory = tempfile::tempdir().expect("marker tempdir");
    let helper_pid = directory.path().join("helper-pid");
    let sudo_marker = directory.path().join("sudo-started");
    let helper = directory.path().join("helper");
    let sudo_path = directory.path().join("sudo");
    fs::write(
        &helper,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$$\" > '{}'\nexec /bin/sleep 30\n",
            helper_pid.display()
        ),
    )
    .expect("write helper");
    fs::write(
        &sudo_path,
        format!("#!/bin/sh\n: > '{}'\nexit 0\n", sudo_marker.display()),
    )
    .expect("write sudo");
    fs::set_permissions(&helper, fs::Permissions::from_mode(0o700)).expect("chmod helper");
    fs::set_permissions(&sudo_path, fs::Permissions::from_mode(0o700)).expect("chmod sudo");

    let mut options = options(sudo_path);
    options.helper_path = helper;
    options.setup_timeout = Duration::from_secs(10);
    let calls = Arc::new(AtomicUsize::new(0));
    let resolver_calls = Arc::clone(&calls);
    let (cancel, cancellation) = sudo::cancellation_channel();
    let runtime = tokio::runtime::Runtime::new().expect("runtime");
    let (outcome, ()) = runtime.block_on(async {
        tokio::join!(
            sudo::execute_local(
                ExecutionRequest::new("/usr/bin/true", Vec::new(), None).expect("request"),
                options,
                ProcessIo {
                    stdin: Stdio::null(),
                    stdout: Stdio::null(),
                    stderr: Stdio::null(),
                },
                cancellation,
                move || async move {
                    resolver_calls.fetch_add(1, Ordering::SeqCst);
                    CapturedSecret::new(b"must-not-be-read".to_vec())
                        .into_secret()
                        .map_err(|_| agentenv::error::AppError::Credential("invalid".to_owned()))
                },
            ),
            async {
                for _ in 0..200 {
                    if helper_pid.exists() {
                        cancel.send(libc::SIGTERM).expect("cancel helper check");
                        return;
                    }
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
                panic!("helper identity process did not start");
            }
        )
    });
    let outcome = outcome.expect("helper cancellation is an observed outcome");
    assert_eq!(outcome.exit_code, None);
    assert_eq!(outcome.signal, Some(libc::SIGTERM));
    assert!(!outcome.password_delivered);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert!(!sudo_marker.exists(), "sudo must not start");
    let pid: i32 = fs::read_to_string(&helper_pid)
        .expect("read helper pid")
        .trim()
        .parse()
        .expect("parse helper pid");
    assert_eq!(unsafe { libc::kill(pid, 0) }, -1, "helper must be reaped");
    assert_eq!(
        std::io::Error::last_os_error().raw_os_error(),
        Some(libc::ESRCH),
        "helper pid must no longer exist"
    );
}

/// Runs a fake sudo until it writes `started`, then requests SIGTERM.
fn cancel_after_start(
    source: &str,
    started: &std::path::Path,
) -> Result<sudo::ExecutionOutcome, agentenv::error::AppError> {
    let (_directory, sudo_path) = fake_sudo(source);
    let (cancel, cancellation) = sudo::cancellation_channel();
    let runtime = tokio::runtime::Runtime::new().expect("runtime");
    let (outcome, ()) = runtime.block_on(async {
        tokio::join!(
            sudo::execute_local(
                ExecutionRequest::new("/usr/bin/true", Vec::new(), None).expect("request"),
                options(sudo_path),
                ProcessIo {
                    stdin: Stdio::null(),
                    stdout: Stdio::null(),
                    stderr: Stdio::null(),
                },
                cancellation,
                || async {
                    Err::<agentenv::credential::Secret, _>(agentenv::error::AppError::Credential(
                        "not requested".to_owned(),
                    ))
                },
            ),
            async {
                for _ in 0..500 {
                    if started.exists() {
                        cancel.send(libc::SIGTERM).expect("cancel sudo");
                        return;
                    }
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
                panic!("fake sudo did not start");
            }
        )
    });
    outcome
}

#[test]
fn cancellation_reports_the_status_sudo_actually_returned() {
    let directory = tempfile::tempdir().expect("marker tempdir");
    let started = directory.path().join("started");
    let source = format!(
        "#!/bin/sh\ntrap 'exit 0' TERM\n: > '{}'\nwhile :; do /bin/sleep 0.05; done\n",
        started.display()
    );
    let outcome = cancel_after_start(&source, &started).expect("sudo exit is observed");
    assert_eq!(outcome.exit_code, Some(0));
    assert_eq!(outcome.signal, None);
}

#[test]
fn sudo_that_survives_the_forwarded_signal_is_completion_unconfirmed() {
    let directory = tempfile::tempdir().expect("marker tempdir");
    let started = directory.path().join("started");
    let source = format!(
        "#!/bin/sh\ntrap '' TERM\n: > '{}'\nwhile :; do /bin/sleep 0.05; done\n",
        started.display()
    );
    let outcome = cancel_after_start(&source, &started);
    assert!(
        matches!(
            outcome,
            Err(agentenv::error::AppError::SudoCompletionUnconfirmed(_))
        ),
        "escalation must not be reported as a confirmed termination"
    );
}

mod deployment {
    //! `--deploy-helper` usage boundaries and an end-to-end run in which a
    //! fake `ssh` delegates `-G` policy evaluation to the real client and
    //! executes every remote command locally through `/bin/sh -c`, exactly
    //! as sshd hands the command to a login shell.

    use super::*;

    fn ssh_config(
        directory: &std::path::Path,
        helper_path: &std::path::Path,
    ) -> std::path::PathBuf {
        let config = directory.join("agentenv.toml");
        let marker = directory.join("provider-ran");
        fs::write(
            &config,
            format!(
                r#"version = 1
default_profile = "work"
[profiles.work]
description = "Work."
[credentials.admin]
description = "Administrator password."
provider = "command"
argv = ["/bin/sh", "-c", "touch '{}' && printf secret"]
usages = ["sudo"]
[profiles.work.local_admin]
description = "Local administrator."
kind = "sudo-target"
[profiles.work.local_admin.sudo]
transport = "local"
credential = "credential://admin"
auth_user = "{account}"
run_as = "root"
sudo_path = "/usr/bin/sudo"
[profiles.work.admin]
description = "Loopback SSH target."
kind = "sudo-target"
[profiles.work.admin.sudo]
transport = "ssh"
credential = "credential://admin"
auth_user = "{account}"
run_as = "root"
sudo_path = "/usr/bin/sudo"
[profiles.work.admin.sudo.ssh]
mode = "explicit"
hostname = "127.0.0.1"
user = "{account}"
port = 22
host_key_alias = "agentenv-fixture"
known_hosts_file = "{known_hosts}"
helper_path = "{helper}"
[profiles.work.admin.sudo.ssh.auth]
method = "publickey"
identity_files = ["{key}"]
use_agent = false
"#,
                marker.display(),
                account = account_name(),
                known_hosts = directory.join("known_hosts").display(),
                helper = helper_path.display(),
                key = directory.join("fixture_key").display(),
            ),
        )
        .expect("config written");
        fs::write(directory.join("fixture_key"), "unused fixture identity\n").expect("key written");
        fs::set_permissions(
            directory.join("fixture_key"),
            fs::Permissions::from_mode(0o600),
        )
        .expect("key mode");
        config
    }

    /// A stand-in for `ssh`: policy queries reach the real client, and the
    /// remote command runs locally under `/bin/sh -c`.
    fn fake_ssh(directory: &std::path::Path) -> std::path::PathBuf {
        let bin = directory.join("bin");
        fs::create_dir_all(&bin).expect("bin dir");
        let script = bin.join("ssh");
        fs::write(
            &script,
            // A file named ssh-exit beside bin makes the fake ssh fail like a
            // connection failure: no output, no remote command, that status.
            // ssh-exit-install does the same for the install session only,
            // like a connection lost after the upload began.
            "#!/bin/sh\nfor argument in \"$@\"; do [ \"$argument\" = \"-G\" ] && exec /usr/bin/ssh \"$@\"; done\nd=$(dirname \"$0\")\n[ -f \"$d/../ssh-exit\" ] && exit \"$(cat \"$d/../ssh-exit\")\"\nfor last in \"$@\"; do :; done\ncase \"$last\" in *agentenv-install*) [ -f \"$d/../ssh-exit-install\" ] && exit \"$(cat \"$d/../ssh-exit-install\")\";; esac\nexec /bin/sh -c \"$last\"\n",
        )
        .expect("fake ssh written");
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).expect("executable");
        bin
    }

    /// The real helper as an upload source. Linux debug builds carry more
    /// than 64 MiB of debug information, above the deployment bound, so the
    /// copy is stripped when the tool exists; the stripped helper still
    /// reports its identity and serves.
    fn upload_source(directory: &std::path::Path) -> Option<std::path::PathBuf> {
        let source = directory.join("source-helper");
        fs::copy(
            assert_cmd::cargo::cargo_bin("agentenv-sudo-helper"),
            &source,
        )
        .expect("helper copied");
        let limit = 64 * 1024 * 1024;
        if fs::metadata(&source).expect("metadata").len() > limit {
            let stripped = std::process::Command::new("strip")
                .arg(&source)
                .status()
                .map(|status| status.success())
                .unwrap_or(false);
            if !stripped || fs::metadata(&source).expect("metadata").len() > limit {
                eprintln!(
                    "skipping: the debug helper exceeds the upload bound and cannot be stripped"
                );
                return None;
            }
        }
        Some(source)
    }

    fn run_deploy(
        config: &std::path::Path,
        bin: &std::path::Path,
        arguments: &[&str],
    ) -> std::process::Output {
        let mut command = Command::cargo_bin("agentenv").expect("agentenv binary");
        command
            .env_clear()
            .env("AGENTENV_FILE", config)
            .env("AGENTENV_NO_PROJECT", "1")
            // The fake ssh comes first; the destination-side commands
            // (sh, uname, getconf, cat, wc, chmod, mv) come from the system.
            .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
            .env("HOME", config.parent().unwrap())
            .args(arguments)
            .output()
            .expect("run agentenv")
    }

    #[test]
    fn deploy_helper_usage_is_explicit_and_offline() {
        let directory = tempfile::tempdir().expect("tempdir");
        let helper_path = directory.path().join("libexec/agentenv-sudo-helper");
        let config = ssh_config(directory.path(), &helper_path);
        let bin = fake_ssh(directory.path());
        for (arguments, status, needle) in [
            (
                vec![
                    "sudo",
                    "--with",
                    "admin",
                    "--deploy-helper",
                    "--",
                    "/usr/bin/id",
                ],
                1,
                "does not accept a command",
            ),
            (
                vec![
                    "sudo",
                    "--with",
                    "admin",
                    "--deploy-helper",
                    "--cwd",
                    "/tmp",
                ],
                1,
                "does not accept --cwd",
            ),
            (
                vec!["sudo", "--with", "local_admin", "--deploy-helper"],
                1,
                "requires an SSH target",
            ),
            (
                vec!["sudo", "--with", "admin", "--deploy-helper", "--check"],
                1,
                "cannot be used with",
            ),
            (
                vec!["sudo", "--with", "admin", "--deploy-helper", "--plan"],
                1,
                "cannot be used with",
            ),
            (
                vec!["sudo", "--with", "admin", "--from", "/tmp/x"],
                1,
                "--deploy-helper",
            ),
            (
                vec!["sudo", "--with", "admin", "--force"],
                1,
                "--deploy-helper",
            ),
        ] {
            let output = run_deploy(&config, &bin, &arguments);
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert_eq!(
                output.status.code(),
                Some(status),
                "{arguments:?}: {stderr}"
            );
            assert!(stderr.contains(needle), "{arguments:?}: {stderr}");
        }
        assert!(!helper_path.exists(), "usage errors never open a session");
        assert!(!directory.path().join("provider-ran").exists());
    }

    #[test]
    fn deploy_helper_installs_upgrades_and_reports_through_the_route() {
        if !std::path::Path::new("/usr/bin/ssh").is_file() {
            eprintln!("skipping: /usr/bin/ssh is required for policy evaluation");
            return;
        }
        let directory = tempfile::tempdir().expect("tempdir");
        let helper_path = directory.path().join("libexec/agentenv-sudo-helper");
        let config = ssh_config(directory.path(), &helper_path);
        let bin = fake_ssh(directory.path());
        let Some(real_helper) = upload_source(directory.path()) else {
            return;
        };
        let real = real_helper.to_str().expect("UTF-8 path");
        let identity = format!(
            "agentenv-sudo-helper {PROTOCOL_VERSION} {}",
            env!("CARGO_PKG_VERSION")
        );

        // A wrong source is refused on the destination and nothing is installed.
        let probe = assert_cmd::cargo::cargo_bin("test-probe");
        let output = run_deploy(
            &config,
            &bin,
            &[
                "sudo",
                "--with",
                "admin",
                "--deploy-helper",
                "--from",
                probe.to_str().unwrap(),
            ],
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!(output.status.code(), Some(9), "{stderr}");
        assert!(
            stderr.contains("helper-deploy-identity-mismatch"),
            "{stderr}"
        );
        assert!(!helper_path.exists());
        assert!(
            fs::read_dir(directory.path().join("libexec"))
                .map(|entries| entries.count() == 0)
                .unwrap_or(true),
            "no temporary upload remains"
        );

        // First installation.
        let output = run_deploy(
            &config,
            &bin,
            &[
                "--json",
                "sudo",
                "--with",
                "admin",
                "--deploy-helper",
                "--from",
                real,
            ],
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!(output.status.code(), Some(0), "{stderr}");
        let report: serde_json::Value =
            serde_json::from_slice(&output.stdout).expect("JSON report");
        assert_eq!(report["status"], "deployed");
        assert_eq!(report["transport"], "ssh");
        assert_eq!(report["helper_path"], helper_path.to_str().unwrap());
        assert_eq!(report["previous"], serde_json::Value::Null);
        assert_eq!(report["installed"], identity);
        assert_eq!(report["source"]["kind"], "file");
        assert_eq!(report["source"]["name"], real);
        assert_eq!(report["helper"]["build"], env!("CARGO_PKG_VERSION"));
        assert_eq!(report["helper"]["auth_user"], account_name());
        assert_eq!(report["sudo_authentication"], "not-attempted");
        assert!(report["destination"]["target"].as_str().unwrap().contains(
            if cfg!(target_os = "macos") {
                "apple-darwin"
            } else {
                "linux"
            }
        ));
        assert_eq!(
            fs::metadata(&helper_path).unwrap().permissions().mode() & 0o7777,
            0o755
        );
        assert_eq!(
            fs::read(&helper_path).unwrap(),
            fs::read(&real_helper).unwrap()
        );
        assert!(
            !directory.path().join("provider-ran").exists(),
            "deployment never resolves the sudo credential"
        );

        // Rerun is a no-op; --force reinstalls over the same identity.
        let output = run_deploy(
            &config,
            &bin,
            &[
                "--json",
                "sudo",
                "--with",
                "admin",
                "--deploy-helper",
                "--from",
                real,
            ],
        );
        let report: serde_json::Value =
            serde_json::from_slice(&output.stdout).expect("JSON report");
        assert_eq!(output.status.code(), Some(0));
        assert_eq!(report["status"], "up-to-date");
        assert_eq!(report["source"], serde_json::Value::Null);
        assert_eq!(report["previous"], identity);
        let output = run_deploy(
            &config,
            &bin,
            &[
                "sudo",
                "--with",
                "admin",
                "--deploy-helper",
                "--from",
                real,
                "--force",
            ],
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert_eq!(
            output.status.code(),
            Some(0),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(stdout.starts_with("Installed "), "{stdout}");
        assert!(
            stdout.contains(&format!("previously {identity}")),
            "{stdout}"
        );
        assert!(stdout.contains("SSH helper is ready as"), "{stdout}");

        // The ordinary check works through the installed helper, and a
        // mismatching helper names the remediation.
        let output = run_deploy(
            &config,
            &bin,
            &["--json", "sudo", "--with", "admin", "--check"],
        );
        assert_eq!(
            output.status.code(),
            Some(0),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        fs::write(&helper_path, format!("#!/bin/sh\ncase \"$1\" in --identity) echo 'agentenv-sudo-helper {PROTOCOL_VERSION} 0.0.1';; *) exit 9;; esac\n")).unwrap();
        let output = run_deploy(&config, &bin, &["sudo", "--with", "admin", "--check"]);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!(output.status.code(), Some(9), "{stderr}");
        assert!(
            stderr.contains("run: agentenv --profile=work sudo --with=admin --deploy-helper"),
            "{stderr}"
        );
        let output = run_deploy(
            &config,
            &bin,
            &[
                "--json",
                "sudo",
                "--with",
                "admin",
                "--deploy-helper",
                "--from",
                real,
            ],
        );
        let report: serde_json::Value =
            serde_json::from_slice(&output.stdout).expect("JSON report");
        assert_eq!(
            output.status.code(),
            Some(0),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(report["status"], "deployed");
        assert_eq!(
            report["previous"],
            format!("agentenv-sudo-helper {PROTOCOL_VERSION} 0.0.1")
        );
    }

    #[test]
    fn deploy_helper_separates_connection_failures_from_helper_failures() {
        if !std::path::Path::new("/usr/bin/ssh").is_file() {
            eprintln!("skipping: /usr/bin/ssh is required for policy evaluation");
            return;
        }
        let directory = tempfile::tempdir().expect("tempdir");
        let helper_path = directory.path().join("libexec/agentenv-sudo-helper");
        let config = ssh_config(directory.path(), &helper_path);
        let bin = fake_ssh(directory.path());
        let Some(real_helper) = upload_source(directory.path()) else {
            return;
        };
        let real = real_helper.to_str().expect("UTF-8 path");

        // ssh exiting 255 before any output is a connection or login
        // failure: no deployment remediation, and deployment itself does not
        // report it as an unparseable preflight.
        fs::write(directory.path().join("ssh-exit"), "255\n").unwrap();
        let output = run_deploy(&config, &bin, &["sudo", "--with", "admin", "--check"]);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!(output.status.code(), Some(9), "{stderr}");
        assert!(stderr.contains("ssh-connect-failed"), "{stderr}");
        assert!(!stderr.contains("--deploy-helper"), "{stderr}");
        let output = run_deploy(
            &config,
            &bin,
            &["sudo", "--with", "admin", "--deploy-helper", "--from", real],
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!(output.status.code(), Some(9), "{stderr}");
        assert!(stderr.contains("ssh-connect-failed"), "{stderr}");
        assert!(!stderr.contains("helper-deploy-"), "{stderr}");
        fs::remove_file(directory.path().join("ssh-exit")).unwrap();

        // The same status during the install session means the connection
        // ended after the upload began: a deployment failure that states
        // the destination may hold either helper and rerunning is safe.
        fs::write(directory.path().join("ssh-exit-install"), "255\n").unwrap();
        let output = run_deploy(
            &config,
            &bin,
            &["sudo", "--with", "admin", "--deploy-helper", "--from", real],
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!(output.status.code(), Some(9), "{stderr}");
        assert!(stderr.contains("helper-deploy-session-failed"), "{stderr}");
        assert!(stderr.contains("rerunning is safe"), "{stderr}");
        assert!(!stderr.contains("ssh-connect-failed"), "{stderr}");
        assert!(!helper_path.exists());
        fs::remove_file(directory.path().join("ssh-exit-install")).unwrap();

        // A remote command that ends without a Ready frame after ssh itself
        // succeeded is a helper problem and names the remediation.
        let output = run_deploy(&config, &bin, &["sudo", "--with", "admin", "--check"]);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!(output.status.code(), Some(9), "{stderr}");
        assert!(stderr.contains("helper-handshake-missing"), "{stderr}");
        assert!(
            stderr.contains("run: agentenv --profile=work sudo --with=admin --deploy-helper"),
            "{stderr}"
        );
    }

    #[test]
    fn deploy_helper_classifies_install_failures_that_precede_the_upload() {
        if !std::path::Path::new("/usr/bin/ssh").is_file() {
            eprintln!("skipping: /usr/bin/ssh is required for policy evaluation");
            return;
        }
        let directory = tempfile::tempdir().expect("tempdir");
        // The parent of helper_path is a regular file, so the install command
        // fails before it reads the upload; an upload larger than any pipe
        // buffer then cannot be written completely.
        let blocker = directory.path().join("blocker");
        fs::write(&blocker, "not a directory\n").unwrap();
        let helper_path = blocker.join("agentenv-sudo-helper");
        let config = ssh_config(directory.path(), &helper_path);
        let bin = fake_ssh(directory.path());
        let large = directory.path().join("large-helper");
        fs::write(&large, vec![0x2a; 4 * 1024 * 1024]).unwrap();
        let output = run_deploy(
            &config,
            &bin,
            &[
                "sudo",
                "--with",
                "admin",
                "--deploy-helper",
                "--from",
                large.to_str().unwrap(),
            ],
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!(output.status.code(), Some(9), "{stderr}");
        assert!(stderr.contains("helper-deploy-replace-failed"), "{stderr}");
        assert!(!stderr.contains("Broken pipe"), "{stderr}");
        assert_eq!(fs::read_to_string(&blocker).unwrap(), "not a directory\n");
    }

    #[test]
    fn check_warns_about_a_helper_path_outside_the_deployment_grammar() {
        if !std::path::Path::new("/usr/bin/ssh").is_file() {
            eprintln!("skipping: /usr/bin/ssh is required for policy evaluation");
            return;
        }
        let directory = tempfile::tempdir().expect("tempdir");
        // '@' is fine for the shell that execution uses but outside the
        // deployment grammar, so the check succeeds and warns. The file name
        // stays agentenv-sudo-helper, which the serving helper requires.
        let helper_path = directory.path().join("libexec@deploy/agentenv-sudo-helper");
        fs::create_dir_all(helper_path.parent().unwrap()).unwrap();
        fs::copy(
            assert_cmd::cargo::cargo_bin("agentenv-sudo-helper"),
            &helper_path,
        )
        .unwrap();
        let config = ssh_config(directory.path(), &helper_path);
        let bin = fake_ssh(directory.path());
        let output = run_deploy(
            &config,
            &bin,
            &["--json", "sudo", "--with", "admin", "--check"],
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!(output.status.code(), Some(0), "{stderr}");
        let report: serde_json::Value =
            serde_json::from_slice(&output.stdout).expect("JSON report");
        assert_eq!(report["status"], "ready");
        assert!(
            stderr.contains("outside the deployment grammar (unsupported character)"),
            "{stderr}"
        );
        let output = run_deploy(
            &config,
            &bin,
            &["sudo", "--with", "admin", "--deploy-helper", "--force"],
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!(output.status.code(), Some(9), "{stderr}");
        assert!(
            stderr.contains("helper-deploy-invalid-helper-path"),
            "{stderr}"
        );
    }
}
