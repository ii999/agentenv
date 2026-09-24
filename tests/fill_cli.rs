//! `credential fill` and `credential fill --capabilities`: argument contract,
//! purpose gate, exit codes, result shape, and value confinement, driven
//! through the debug-only scripted test backend.
#![cfg(feature = "test-keychain")]

mod helpers;

use std::fs;
use std::path::PathBuf;

use helpers::{
    assert_exit, assert_mentions, assert_omits, command_with_project_discovery, run_ac,
    staged_config, Run, SENTINEL_NESTED, SENTINEL_PLAIN,
};
use tempfile::TempDir;

const CONFIG: &str = r#"version = 1

[credentials.api_token]
description = "API token."
provider = "env"
name = "API_TOKEN"
inject_as = "API_TOKEN"

[credentials.root_pw]
description = "Root password."
provider = "keychain"
service = "agentenv.sudo"
account = "root"
usages = ["sudo"]

[credentials.vault_token]
description = "Vault token."
provider = "keychain"
service = "agentenv"
account = "vault"
inject_as = "VAULT_TOKEN"
"#;

struct Fixture {
    _dir: TempDir,
    config: PathBuf,
    sink: PathBuf,
    store: PathBuf,
}

impl Fixture {
    fn new(config: &str) -> Self {
        let (dir, path) = staged_config(config);
        let sink = dir.path().join("delivered.txt");
        let store = dir.path().join("test-keychain.json");
        Self {
            _dir: dir,
            config: path,
            sink,
            store,
        }
    }

    fn sink_str(&self) -> &str {
        self.sink.to_str().unwrap()
    }

    fn store_str(&self) -> &str {
        self.store.to_str().unwrap()
    }

    fn delivered(&self) -> Option<String> {
        fs::read_to_string(&self.sink).ok()
    }

    /// Runs a fill through the scripted backend with `behavior`, the sink
    /// wired, and `extra` environment.
    fn fill(&self, behavior: &str, extra: &[(&str, &str)], args: &[&str]) -> Run {
        let sink = self.sink_str().to_owned();
        let store = self.store_str().to_owned();
        let mut envs = vec![
            ("AGENTENV_FILL_TEST_BEHAVIOR", behavior),
            ("AGENTENV_FILL_TEST_SINK", sink.as_str()),
            ("AGENTENV_TEST_KEYCHAIN", store.as_str()),
        ];
        envs.extend_from_slice(extra);
        let mut full = vec!["--json", "credential", "fill"];
        full.extend_from_slice(args);
        full.extend_from_slice(&["--backend", "test"]);
        run_ac(&self.config, &envs, &full)
    }
}

fn store_keychain_value(fixture: &Fixture, name: &str, value: &str) {
    let mut command = command_with_project_discovery(&fixture.config);
    command
        .env("AGENTENV_NO_PROJECT", "1")
        .env("AGENTENV_TEST_KEYCHAIN", fixture.store_str())
        .args(["credential", "set", name])
        .write_stdin(format!("{value}\n"));
    let output = command.output().expect("credential set runs");
    assert!(
        output.status.success(),
        "credential set failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn assert_fill_json(run: &Run, backend: &str, effect: &str) {
    assert_eq!(
        run.stdout,
        format!("{{\"version\":1,\"backend\":\"{backend}\",\"effect\":\"{effect}\"}}\n"),
        "success JSON is exactly version, backend and effect"
    );
    assert!(
        run.stderr.is_empty(),
        "success writes nothing to stderr: {}",
        run.stderr
    );
}

#[test]
fn capabilities_reports_backends_without_a_credential() {
    let fixture = Fixture::new(CONFIG);
    let run = run_ac(
        &fixture.config,
        &[],
        &["--json", "credential", "fill", "--capabilities"],
    );
    assert_exit(&run, 0, "capabilities succeeds");
    let document: serde_json::Value =
        serde_json::from_str(run.stdout.trim()).expect("capabilities is JSON");
    assert_eq!(document["version"], 1);
    for backend in ["cdp", "playwright", "desktop"] {
        assert!(
            document["backends"][backend]["compiled"].is_boolean(),
            "{backend} compiled flag"
        );
        assert!(
            document["backends"][backend]["available"].is_boolean(),
            "{backend} available flag"
        );
    }
    assert_eq!(document["resolver"]["confidential"], cfg!(unix));

    let text = run_ac(
        &fixture.config,
        &[],
        &["credential", "fill", "--capabilities"],
    );
    assert_exit(&text, 0, "text capabilities succeeds");
    assert_mentions(&text, "cdp:", "text lists the cdp backend");

    let with_name = run_ac(
        &fixture.config,
        &[],
        &["credential", "fill", "api_token", "--capabilities"],
    );
    assert_exit(&with_name, 1, "capabilities rejects a credential name");
}

#[test]
fn argument_contract_is_enforced_before_configuration_use() {
    let fixture = Fixture::new(CONFIG);
    let cases: &[(&[&str], &str)] = &[
        (&["credential", "fill"], "requires a credential name"),
        (&["credential", "fill", "api_token"], "requires --backend"),
        (
            &["credential", "fill", "api_token", "--backend", "cdp"],
            "requires --endpoint",
        ),
        (
            &[
                "credential",
                "fill",
                "api_token",
                "--backend",
                "cdp",
                "--endpoint",
                "http://127.0.0.1:9222",
                "--page-url",
                "http://x/",
                "--selector",
                "#p",
                "--expect-pid",
                "1",
            ],
            "--expect-pid does not apply",
        ),
        (
            &[
                "credential",
                "fill",
                "api_token",
                "--backend",
                "playwright",
                "--endpoint",
                "ws://127.0.0.1:1/",
                "--page-url",
                "http://x/",
                "--selector",
                "#p",
            ],
            "requires --browser",
        ),
        (
            &["credential", "fill", "api_token", "--backend", "desktop"],
            "requires --expect-pid",
        ),
        (
            &[
                "credential",
                "fill",
                "api_token",
                "--backend",
                "desktop",
                "--expect-pid",
                "1",
                "--selector",
                "#p",
            ],
            "--selector does not apply",
        ),
    ];
    for (args, expected) in cases {
        let run = run_ac(&fixture.config, &[], args);
        assert_exit(&run, 1, &format!("{args:?} is a usage error"));
        assert_mentions(&run, expected, &format!("{args:?} names the problem"));
        assert!(run.stdout.is_empty(), "usage errors leave stdout empty");
    }
    for timeout in ["0", "300001"] {
        let run = run_ac(
            &fixture.config,
            &[],
            &[
                "credential",
                "fill",
                "api_token",
                "--backend",
                "desktop",
                "--expect-pid",
                "1",
                "--timeout-ms",
                timeout,
            ],
        );
        assert_exit(&run, 1, "timeout outside 1..=300000 is a usage error");
    }
    let rejected: &[(&[&str], &str)] = &[
        (
            &[
                "credential",
                "fill",
                "api_token",
                "--backend",
                "desktop",
                "--expect-pid",
                "1",
                "--page-match",
                "exact",
            ],
            "--page-match does not apply",
        ),
        (
            &["credential", "fill", "--capabilities", "--timeout-ms", "5"],
            "--capabilities takes no",
        ),
        (
            &[
                "credential",
                "fill",
                "--capabilities",
                "--page-match",
                "exact",
            ],
            "--capabilities takes no",
        ),
        (
            &[
                "credential",
                "fill",
                "api_token",
                "--backend",
                "cdp",
                "--endpoint",
                "http://127.0.0.1:9222",
                "--page-url",
                "about:blank",
                "--selector",
                "#p",
            ],
            "--page-url must be an absolute URL",
        ),
    ];
    for (args, expected) in rejected {
        let run = run_ac(&fixture.config, &[], args);
        assert_exit(&run, 1, &format!("{args:?} is a usage error"));
        assert_mentions(&run, expected, &format!("{args:?} names the problem"));
    }
}

#[test]
fn unknown_and_authentication_credentials_fail_before_any_backend_work() {
    let fixture = Fixture::new(CONFIG);
    let unknown = fixture.fill("filled", &[], &["nope"]);
    assert_exit(&unknown, 3, "unknown credential");
    assert!(unknown.stdout.is_empty());

    let authentication = fixture.fill("filled", &[], &["root_pw"]);
    assert_exit(
        &authentication,
        2,
        "authentication credentials cannot be filled",
    );
    assert_mentions(
        &authentication,
        "credentials.root_pw.usages",
        "the violation names the usages field",
    );
    assert_mentions(&authentication, "sudo", "the permitted usage is listed");
    assert!(authentication.stdout.is_empty());
    assert!(fixture.delivered().is_none(), "no delivery happened");
}

#[test]
fn cdp_reports_connection_failures_and_other_backends_are_unavailable() {
    let fixture = Fixture::new(CONFIG);
    // Bind and release a loopback port so nothing is listening on it.
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let closed = format!("http://127.0.0.1:{port}");
    let cases = [
        ("cdp", closed.as_str(), "connect-failed"),
        ("cdp", "http://192.0.2.1:9222", "connect-failed"),
        ("playwright", "ws://127.0.0.1:1/", "backend-unavailable"),
    ];
    for (backend, endpoint, reason) in cases {
        let mut args = vec![
            "--json",
            "credential",
            "fill",
            "api_token",
            "--backend",
            backend,
            "--endpoint",
            endpoint,
            "--page-url",
            "http://portal.example/login",
            "--selector",
            "#password",
        ];
        if backend == "playwright" {
            args.extend_from_slice(&["--browser", "chromium"]);
        }
        let run = run_ac(&fixture.config, &[("API_TOKEN", SENTINEL_PLAIN)], &args);
        assert_exit(&run, 8, &format!("{backend} {endpoint}"));
        assert!(
            run.stdout.is_empty(),
            "failed JSON invocations leave stdout empty"
        );
        assert!(
            run.stderr
                .starts_with(&format!("credential-fill: {reason}:")),
            "{backend} {endpoint}: {}",
            run.stderr
        );
    }
}

#[test]
fn env_credential_is_delivered_exactly_and_only_the_effect_is_reported() {
    let fixture = Fixture::new(CONFIG);
    let value = format!("  {SENTINEL_PLAIN} 🔑 \"quoted\" <b>&amp;</b>  ");
    let run = fixture.fill("filled", &[("API_TOKEN", value.as_str())], &["api_token"]);
    assert_exit(&run, 0, "fill succeeds");
    assert_fill_json(&run, "test", "field-filled");
    assert_eq!(
        fixture.delivered().as_deref(),
        Some(value.as_str()),
        "the destination received the exact value"
    );

    let text = run_ac(
        &fixture.config,
        &[
            ("API_TOKEN", SENTINEL_PLAIN),
            ("AGENTENV_FILL_TEST_BEHAVIOR", "input-sent"),
        ],
        &["credential", "fill", "api_token", "--backend", "test"],
    );
    assert_exit(&text, 0, "text fill succeeds");
    assert_mentions(&text, "input-sent", "the text output names the effect");
    assert_omits(&text, "field-filled", "the effect is the backend's");
}

#[test]
fn keychain_credential_goes_through_the_resolver_fill_stage() {
    let fixture = Fixture::new(CONFIG);
    store_keychain_value(&fixture, "vault_token", SENTINEL_NESTED);
    let debug = [("RUST_LOG", "trace"), ("RUST_BACKTRACE", "1")];
    let run = fixture.fill("filled", &debug, &["vault_token"]);
    if cfg!(unix) {
        assert_exit(&run, 0, "keychain fill succeeds through the resolver");
        assert_fill_json(&run, "test", "field-filled");
        assert_eq!(fixture.delivered().as_deref(), Some(SENTINEL_NESTED));
    } else {
        assert_exit(&run, 8, "the confidential resolver is unavailable off Unix");
        assert!(run
            .stderr
            .starts_with("credential-fill: backend-unavailable:"));
    }

    let unset_fixture = Fixture::new(CONFIG);
    let missing = unset_fixture.fill("filled", &debug, &["vault_token"]);
    if cfg!(unix) {
        assert_exit(&missing, 4, "a missing keychain item is a credential error");
    } else {
        // Off Unix the keychain is never consulted: the command refuses
        // before lookup because the confidential resolver is unavailable.
        assert_exit(
            &missing,
            8,
            "keychain credentials are refused before lookup off Unix",
        );
    }
    assert!(missing.stdout.is_empty());
    assert!(unset_fixture.delivered().is_none());
}

#[test]
fn unsupported_values_are_rejected_before_delivery() {
    let fixture = Fixture::new(CONFIG);
    let multi_line = format!("{SENTINEL_PLAIN}\nsecond line");
    let run = fixture.fill(
        "filled",
        &[("API_TOKEN", multi_line.as_str())],
        &["api_token"],
    );
    assert_exit(&run, 8, "multi-line values are unsupported");
    assert!(
        run.stderr
            .starts_with("credential-fill: value-unsupported:"),
        "{}",
        run.stderr
    );
    assert_omits(&run, "second line", "no fragment of the value is echoed");
    assert!(fixture.delivered().is_none());

    let oversize = "a".repeat(8193);
    let run = fixture.fill(
        "filled",
        &[("API_TOKEN", oversize.as_str())],
        &["api_token"],
    );
    assert_exit(&run, 8, "oversize values are unsupported");
    assert!(fixture.delivered().is_none());

    let unset = fixture.fill("filled", &[], &["api_token"]);
    assert_exit(&unset, 4, "an unset env credential is a credential error");
    assert!(fixture.delivered().is_none());
}

#[test]
fn preflight_failures_cost_no_lookup_and_use_code_8() {
    let fixture = Fixture::new(CONFIG);
    for (behavior, reason) in [
        ("prepare-fail:target-absent", "target-absent"),
        ("prepare-fail:recording-conflict", "recording-conflict"),
        ("prepare-fail:permission", "permission"),
    ] {
        // The credential is deliberately unset: a lookup would fail with 4.
        let run = fixture.fill(
            behavior,
            &[("RUST_LOG", "trace"), ("RUST_BACKTRACE", "1")],
            &["api_token"],
        );
        assert_exit(&run, 8, behavior);
        assert!(
            run.stderr
                .starts_with(&format!("credential-fill: {reason}:")),
            "{}",
            run.stderr
        );
        assert!(run.stdout.is_empty());
    }
    assert!(fixture.delivered().is_none());
}

#[test]
fn expiry_and_delivery_failures_distinguish_code_8_from_code_11() {
    let fixture = Fixture::new(CONFIG);
    let env = [("API_TOKEN", SENTINEL_PLAIN)];

    let before = fixture.fill("prepare-hang", &env, &["api_token", "--timeout-ms", "300"]);
    assert_exit(&before, 8, "expiry before delivery");
    assert!(
        before.stderr.starts_with("credential-fill: timeout:"),
        "{}",
        before.stderr
    );
    assert_omits(&before, "may have changed", "no mutation is claimed");
    assert!(fixture.delivered().is_none());

    let revalidating = fixture.fill(
        "revalidate-hang",
        &env,
        &["api_token", "--timeout-ms", "300"],
    );
    assert_exit(&revalidating, 8, "expiry before the mutation gate");
    assert!(
        revalidating.stderr.starts_with("credential-fill: timeout:"),
        "{}",
        revalidating.stderr
    );
    assert_omits(&revalidating, "may have changed", "no mutation is claimed");
    assert!(fixture.delivered().is_none());

    let during = fixture.fill("deliver-hang", &env, &["api_token", "--timeout-ms", "300"]);
    assert_exit(&during, 11, "expiry during delivery");
    assert!(
        during.stderr.starts_with("credential-fill: timeout:"),
        "{}",
        during.stderr
    );
    assert_mentions(&during, "may have changed", "uncertain mutation is stated");

    let changed = fixture.fill("deliver-fail", &env, &["api_token"]);
    assert_exit(&changed, 8, "a detected target change before insertion");
    assert!(
        changed
            .stderr
            .starts_with("credential-fill: target-changed:"),
        "{}",
        changed.stderr
    );

    let uncertain = fixture.fill("deliver-fail-uncertain", &env, &["api_token"]);
    assert_exit(&uncertain, 11, "an interrupted delivery");
    assert!(
        uncertain
            .stderr
            .starts_with("credential-fill: delivery-failed:"),
        "{}",
        uncertain.stderr
    );
    assert_mentions(
        &uncertain,
        "may have changed",
        "uncertain mutation is stated",
    );

    let leaked = fixture.fill("release-fail", &env, &["api_token"]);
    assert_exit(&leaked, 11, "unconfirmed cleanup after delivery");
    assert!(
        leaked
            .stderr
            .starts_with("credential-fill: cleanup-unconfirmed:"),
        "{}",
        leaked.stderr
    );
    assert_eq!(
        fixture.delivered().as_deref(),
        Some(SENTINEL_PLAIN),
        "the value was delivered before cleanup failed"
    );
    assert!(
        leaked.stdout.is_empty(),
        "an uncertain result is not a success document"
    );

    let unset = Fixture::new(CONFIG);
    let run = unset.fill("release-fail", &[], &["api_token"]);
    assert_exit(
        &run,
        4,
        "a credential error keeps its code when cleanup also fails",
    );
    assert_mentions(
        &run,
        "cleanup unconfirmed",
        "the cleanup failure is still reported",
    );
    assert!(unset.delivered().is_none());
}

#[cfg(unix)]
#[test]
fn command_credential_goes_through_the_resolver_fill_stage() {
    use std::os::unix::fs::PermissionsExt;

    let dir = TempDir::new().unwrap();
    let script = dir.path().join("provider.sh");
    fs::write(
        &script,
        format!("#!/bin/sh\nprintf 'SENTINEL-STDERR' >&2\nprintf '%s' '{SENTINEL_PLAIN}'\n"),
    )
    .unwrap();
    fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
    let config = format!(
        "version = 1\n\n[credentials.cmd_token]\ndescription = \"Command token.\"\nprovider = \"command\"\nargv = [{}]\ninject_as = \"CMD_TOKEN\"\n",
        serde_json::to_string(script.to_str().unwrap()).unwrap()
    );
    let fixture = Fixture::new(&config);
    let debug = [("RUST_LOG", "trace"), ("RUST_BACKTRACE", "1")];
    let run = fixture.fill("filled", &debug, &["cmd_token"]);
    assert_exit(&run, 0, "command fill succeeds");
    assert_fill_json(&run, "test", "field-filled");
    assert_omits(&run, "SENTINEL-STDERR", "provider stderr is not forwarded");
    assert_eq!(fixture.delivered().as_deref(), Some(SENTINEL_PLAIN));

    fs::write(
        &script,
        format!("#!/bin/sh\nprintf '{SENTINEL_PLAIN}' >&2\nexit 1\n"),
    )
    .unwrap();
    let failing = Fixture::new(&config);
    let run = failing.fill("filled", &debug, &["cmd_token"]);
    assert_exit(&run, 4, "a failing provider is a credential error");
    assert!(run.stdout.is_empty());
    assert!(failing.delivered().is_none());

    fs::write(&script, "#!/bin/sh\nprintf ''\n").unwrap();
    let empty = Fixture::new(&config);
    let run = empty.fill("filled", &debug, &["cmd_token"]);
    assert_exit(
        &run,
        4,
        "empty output is a credential error, as for env values",
    );
    assert!(empty.delivered().is_none());

    fs::write(&script, "#!/bin/sh\nsleep 5\n").unwrap();
    let slow = Fixture::new(&config);
    let run = slow.fill("filled", &debug, &["cmd_token", "--timeout-ms", "300"]);
    assert_exit(&run, 8, "lookup expiry is a timeout with nothing changed");
    assert!(
        run.stderr.starts_with("credential-fill: timeout:"),
        "{}",
        run.stderr
    );
    assert_omits(&run, "may have changed", "no mutation is claimed");
    assert!(slow.delivered().is_none());

    fs::write(
        &script,
        format!("#!/bin/sh\nprintf '{SENTINEL_PLAIN}\\n'\n"),
    )
    .unwrap();
    let trailing = Fixture::new(&config);
    let run = trailing.fill("filled", &[], &["cmd_token"]);
    assert_exit(
        &run,
        0,
        "one trailing newline is a line ending, not part of the value",
    );
    assert_fill_json(&run, "test", "field-filled");
    assert_eq!(trailing.delivered().as_deref(), Some(SENTINEL_PLAIN));

    let unicode = format!("  pässwörd-日本語-🔑-{SENTINEL_PLAIN}  ");
    fs::write(&script, format!("#!/bin/sh\nprintf '%s\\n' '{unicode}'\n")).unwrap();
    let exact = Fixture::new(&config);
    let run = exact.fill("filled", &[], &["cmd_token"]);
    assert_exit(
        &run,
        0,
        "unicode and surrounding spaces are delivered exactly",
    );
    assert_eq!(exact.delivered().as_deref(), Some(unicode.as_str()));

    fs::write(
        &script,
        format!("#!/bin/sh\nprintf '{SENTINEL_PLAIN}\\n\\n'\n"),
    )
    .unwrap();
    let blank = Fixture::new(&config);
    let run = blank.fill("filled", &[], &["cmd_token"]);
    assert_exit(&run, 8, "a second line is still unsupported");
    assert!(
        run.stderr
            .starts_with("credential-fill: value-unsupported:"),
        "{}",
        run.stderr
    );
    assert!(blank.delivered().is_none());
}

#[cfg(unix)]
#[test]
fn a_signal_cancels_the_operation_and_reaps_the_resolver() {
    use std::os::unix::fs::PermissionsExt;
    use std::process::{Command, Stdio};

    let dir = TempDir::new().unwrap();
    let script = dir.path().join("provider.sh");
    let pid_file = dir.path().join("resolver-pid");
    fs::write(
        &script,
        format!(
            "#!/bin/sh\nprintf '%s %s' \"$PPID\" \"$$\" > '{0}.tmp' && mv '{0}.tmp' '{0}'\nsleep 30\nprintf '{SENTINEL_PLAIN}'\n",
            pid_file.display()
        ),
    )
    .unwrap();
    fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
    let config = format!(
        "version = 1\n\n[credentials.cmd_token]\ndescription = \"Command token.\"\nprovider = \"command\"\nargv = [{}]\ninject_as = \"CMD_TOKEN\"\n",
        serde_json::to_string(script.to_str().unwrap()).unwrap()
    );
    let fixture = Fixture::new(&config);
    let mut command = Command::new(env!("CARGO_BIN_EXE_agentenv"));
    command
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("AGENTENV_FILE", &fixture.config)
        .env("AGENTENV_NO_PROJECT", "1")
        .env("AGENTENV_TEST_KEYCHAIN", fixture.store_str())
        .env("AGENTENV_FILL_TEST_BEHAVIOR", "filled")
        .env("AGENTENV_FILL_TEST_SINK", fixture.sink_str())
        .args([
            "--json",
            "credential",
            "fill",
            "cmd_token",
            "--backend",
            "test",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let child = command.spawn().expect("fill starts");
    let started = std::time::Instant::now();
    let pids: Vec<i32> = loop {
        let parsed: Vec<i32> = fs::read_to_string(&pid_file)
            .unwrap_or_default()
            .split_whitespace()
            .filter_map(|pid| pid.parse().ok())
            .collect();
        if parsed.len() == 2 {
            break parsed;
        }
        assert!(
            started.elapsed() < std::time::Duration::from_secs(10),
            "resolver never started"
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    };
    let (resolver_pid, provider_pid) = (pids[0], pids[1]);
    assert_eq!(unsafe { libc::kill(child.id() as i32, libc::SIGTERM) }, 0);
    let output = child.wait_with_output().expect("fill exits");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(8), "{stderr}");
    assert!(
        stderr.starts_with("credential-fill: cancelled:"),
        "{stderr}"
    );
    assert!(output.stdout.is_empty());
    assert!(!stderr.contains(SENTINEL_PLAIN));
    assert!(fixture.delivered().is_none(), "nothing was delivered");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let resolver_alive = unsafe { libc::kill(resolver_pid, 0) } == 0;
        let provider_alive = unsafe { libc::kill(provider_pid, 0) } == 0;
        if !resolver_alive && !provider_alive {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the resolver or its provider outlived the cancelled fill"
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

#[test]
fn existing_credential_commands_keep_their_contract() {
    let fixture = Fixture::new(CONFIG);
    let check = run_ac(
        &fixture.config,
        &[("API_TOKEN", SENTINEL_PLAIN)],
        &["credential", "check", "api_token"],
    );
    assert_exit(&check, 0, "credential check still works");
    let list = run_ac(&fixture.config, &[], &["--json", "credential", "list"]);
    assert_exit(&list, 0, "credential list still works");
    assert_mentions(&list, "root_pw", "authentication credentials are listed");
}
