//! Deterministic contracts of explicit remote helper deployment: path
//! grammar, remote command templates, strict preflight parsing, the decision
//! table, and the session order driven through a scripted runner.

use std::path::Path;

use agentenv::sudo::deploy::{
    decide, deploy, destination_target, expected_identity, install_command, parse_identity,
    parse_preflight, preflight_command, release_asset_name, validate_helper_path, Decision,
    Destination, Existing, HelperBytes, HelperSource, Preflight, Reason, SessionOutput,
    SessionRunner, Source, Status, HELPER_UPLOAD_LIMIT, SESSION_OUTPUT_LIMIT,
};
use agentenv::sudo::PROTOCOL_VERSION;

const HELPER: &str = "/home/deploy/.local/libexec/agentenv-sudo-helper";

fn identity() -> String {
    format!(
        "agentenv-sudo-helper {PROTOCOL_VERSION} {}",
        env!("CARGO_PKG_VERSION")
    )
}

fn preflight_output(system: &str, machine: &str, glibc: &str, state: &str) -> Vec<u8> {
    format!("{system}\n{machine}\n{glibc}\n{state}\n").into_bytes()
}

fn linux_absent() -> Preflight {
    Preflight {
        system: "Linux".into(),
        machine: "x86_64".into(),
        glibc: Some((2, 36)),
        existing: Existing::Absent,
    }
}

#[test]
fn helper_path_grammar_accepts_plain_absolute_paths_only() {
    for accepted in [
        HELPER,
        "/opt/agentenv/agentenv-sudo-helper",
        "/srv/tools+extra/v1.2/agentenv-sudo-helper",
    ] {
        assert_eq!(
            validate_helper_path(Path::new(accepted)).expect(accepted),
            accepted
        );
    }
    for rejected in [
        "/opt/agentenv/helper_bin",
        "/opt/agentenv/agentenv-sudo-helper.exe",
        "/opt/agentenv/Agentenv-sudo-helper",
        "relative/helper",
        "/path with space/helper",
        "/path/with'quote",
        "/path/with\"quote",
        "/path/$HOME/helper",
        "/path/;rm/helper",
        "/path/with!bang",
        "/trailing/",
        "/double//slash",
        "/dot/./component",
        "/parent/../component",
        "/unicode/héLper",
        "/tab\there",
        "/home/~deploy/helper",
        "/back`tick`/helper",
        "/back\\slash/helper",
        "/new\nline/helper",
        "/glob*/helper",
        "/glob?/helper",
        "/nul\0/helper",
    ] {
        let error = validate_helper_path(Path::new(rejected)).expect_err(rejected);
        assert!(
            error
                .to_string()
                .contains("helper-deploy-invalid-helper-path"),
            "{rejected}: {error}"
        );
    }
    let long = format!("/{}/agentenv-sudo-helper", "a".repeat(1003));
    assert!(validate_helper_path(Path::new(&long)).is_err());
    let limit = format!("/{}/agentenv-sudo-helper", "a".repeat(1002));
    assert!(
        validate_helper_path(Path::new(&limit)).is_ok(),
        "exactly 1024 bytes is accepted"
    );
}

#[test]
fn remote_commands_are_fixed_templates_around_the_quoted_path() {
    let expected = identity();
    for (command, name, tail) in [
        (
            preflight_command(HELPER),
            "agentenv-preflight",
            String::new(),
        ),
        (
            install_command(HELPER, &expected, 4096),
            "agentenv-install",
            format!(" '{expected}' '4096'"),
        ),
    ] {
        let prefix = "sh -c '";
        let suffix = format!("' {name} '{HELPER}'{tail}");
        assert!(command.starts_with(prefix), "{command}");
        assert!(command.ends_with(&suffix), "{command}");
        let script = &command[prefix.len()..command.len() - suffix.len()];
        assert!(!script.contains(['\'', '\\', '!']), "{script}");
        assert!(script.starts_with("p=$1;"), "{script}");
    }
    let install = install_command(HELPER, &expected, 4096);
    assert!(install.contains("umask 077"));
    assert!(install.contains("set -C; cat > \"$t\""));
    assert!(install.contains("chmod 0755 \"$t\""));
    assert!(
        install.contains("[ \"$c\" = \"$n\" ]"),
        "size is checked on the destination"
    );
    assert!(
        install.contains("[ \"$i\" = \"$e\" ]"),
        "identity is checked on the destination"
    );
    let identity_check = install.find("[ \"$i\" = \"$e\" ]").unwrap();
    let rename = install.find("mv -f \"$t\" \"$p\"").unwrap();
    assert!(
        identity_check < rename,
        "identity is verified before the rename"
    );
    let ending = format!("echo \"$i\"' agentenv-install '{HELPER}' '{expected}' '4096'");
    assert!(install.ends_with(&ending), "{install}");
    let preflight = preflight_command(HELPER);
    assert!(preflight.contains("uname -s; uname -m; getconf GNU_LIBC_VERSION"));
    assert!(!preflight.contains("cat >"), "the preflight never writes");
}

#[test]
fn preflight_parsing_is_strict_about_shape_and_lenient_about_foreign_identity_output() {
    let parsed = parse_preflight(&preflight_output(
        "Linux",
        "aarch64",
        "glibc 2.36",
        "absent",
    ))
    .expect("linux absent");
    assert_eq!(
        parsed,
        Preflight {
            system: "Linux".into(),
            machine: "aarch64".into(),
            glibc: Some((2, 36)),
            existing: Existing::Absent,
        }
    );
    let helper = identity();
    let parsed = parse_preflight(&preflight_output(
        "Darwin",
        "arm64",
        "none",
        &format!("helper {helper}"),
    ))
    .expect("macos helper");
    assert_eq!(parsed.glibc, None);
    assert_eq!(parsed.existing, Existing::Helper(helper.clone()));
    let older = "agentenv-sudo-helper 1 0.1.0";
    assert_eq!(
        parse_preflight(&preflight_output(
            "Linux",
            "x86_64",
            "glibc 2.28",
            &format!("helper {older}")
        ))
        .expect("older helper")
        .existing,
        Existing::Helper(older.into())
    );
    // A forged keyword behind the prefix and an empty or trailing helper
    // answer are occupied paths: only the destination script emits the two
    // keywords, and everything after `helper ` is the executable's output.
    for forged in [
        "helper absent",
        "helper ",
        "helper agentenv-sudo-helper 1 0.1.0\nextra",
    ] {
        assert_eq!(
            parse_preflight(&preflight_output("Linux", "x86_64", "glibc 2.28", forged))
                .expect(forged)
                .existing,
            Existing::Occupied,
            "{forged}"
        );
    }
    assert_eq!(
        parse_preflight(&preflight_output("Linux", "x86_64", "glibc 2.28", "absent"))
            .expect("absent")
            .existing,
        Existing::Absent
    );
    assert_eq!(
        parse_preflight(&preflight_output(
            "Linux",
            "x86_64",
            "glibc 2.28",
            "occupied"
        ))
        .expect("occupied")
        .existing,
        Existing::Occupied
    );
    // A foreign executable answering --identity with its own text is an
    // occupied path, not a parse failure; the script prefixes it with
    // `helper `. Text after a bare keyword did not come from the script.
    assert_eq!(
        parse_preflight(&preflight_output(
            "Linux",
            "x86_64",
            "glibc 2.28",
            "helper usage: something --flag\nmore text"
        ))
        .expect("foreign output")
        .existing,
        Existing::Occupied
    );
    // A bare identity, text after a bare keyword, or any other fourth line
    // did not come from the script and is a parse failure.
    for trailing in [
        helper.as_str(),
        "absent\nmore",
        "occupied\nmore",
        "usage: something --flag\nmore text",
    ] {
        let error = parse_preflight(&preflight_output("Linux", "x86_64", "glibc 2.28", trailing))
            .expect_err(trailing);
        assert!(
            error
                .to_string()
                .contains("helper-deploy-preflight-unparseable"),
            "{trailing}: {error}"
        );
    }
    for (name, output) in [
        (
            "banner",
            b"Welcome to prod!\nLinux\nx86_64\nglibc 2.36\nabsent\n".to_vec(),
        ),
        ("truncated", b"Linux\nx86_64\nglibc 2.36\n".to_vec()),
        (
            "no final newline",
            b"Linux\nx86_64\nglibc 2.36\nabsent".to_vec(),
        ),
        ("empty", Vec::new()),
        (
            "bad glibc",
            preflight_output("Linux", "x86_64", "glibc two", "absent"),
        ),
        (
            "bad system",
            preflight_output("Linux kernel", "x86_64", "glibc 2.36", "absent"),
        ),
        (
            "control bytes",
            b"Linux\nx86_64\x07\nglibc 2.36\nabsent\n".to_vec(),
        ),
        ("not utf-8", b"Linux\n\xff\nglibc 2.36\nabsent\n".to_vec()),
        ("too long", vec![b'a'; SESSION_OUTPUT_LIMIT + 1]),
    ] {
        let error = parse_preflight(&output).expect_err(name);
        assert!(
            error
                .to_string()
                .contains("helper-deploy-preflight-unparseable"),
            "{name}: {error}"
        );
    }
}

#[test]
fn destination_targets_cover_the_published_helpers_only() {
    assert_eq!(
        destination_target("Linux", "x86_64"),
        Some(("linux", "x86_64-unknown-linux-gnu"))
    );
    assert_eq!(
        destination_target("Linux", "aarch64"),
        Some(("linux", "aarch64-unknown-linux-gnu"))
    );
    assert_eq!(
        destination_target("Darwin", "arm64"),
        Some(("macos", "aarch64-apple-darwin"))
    );
    assert_eq!(
        destination_target("Darwin", "x86_64"),
        Some(("macos", "x86_64-apple-darwin"))
    );
    for (system, machine) in [
        ("Linux", "armv7l"),
        ("Linux", "riscv64"),
        ("FreeBSD", "amd64"),
        ("Windows_NT", "x86_64"),
    ] {
        assert_eq!(
            destination_target(system, machine),
            None,
            "{system} {machine}"
        );
    }
}

#[test]
fn decision_table_follows_the_design() {
    let expected = identity();
    let mut preflight = linux_absent();
    assert!(matches!(
        decide(&preflight, &expected, false).expect("absent installs"),
        Decision::Install { previous: None, ref destination }
            if destination.target == "x86_64-unknown-linux-gnu" && destination.glibc.as_deref() == Some("2.36")
    ));
    preflight.existing = Existing::Helper(expected.clone());
    assert!(matches!(
        decide(&preflight, &expected, false).expect("matching helper"),
        Decision::UpToDate { ref identity, .. } if *identity == expected
    ));
    assert!(matches!(
        decide(&preflight, &expected, true).expect("forced reinstall"),
        Decision::Install { previous: Some(ref previous), .. } if *previous == expected
    ));
    preflight.existing = Existing::Helper("agentenv-sudo-helper 1 0.1.0".into());
    assert!(matches!(
        decide(&preflight, &expected, false).expect("older helper upgrades"),
        Decision::Install { previous: Some(ref previous), .. } if previous == "agentenv-sudo-helper 1 0.1.0"
    ));
    preflight.existing = Existing::Occupied;
    let error = decide(&preflight, &expected, true).expect_err("occupied refuses even when forced");
    assert!(
        error.to_string().contains("helper-deploy-path-occupied"),
        "{error}"
    );
    let mut old = linux_absent();
    old.glibc = Some((2, 27));
    let error = decide(&old, &expected, false).expect_err("old glibc");
    assert!(
        error.to_string().contains("helper-deploy-glibc-too-old"),
        "{error}"
    );
    old.glibc = Some((2, 28));
    assert!(
        decide(&old, &expected, false).is_ok(),
        "the floor itself is accepted"
    );
    let mut musl = linux_absent();
    musl.glibc = None;
    let error = decide(&musl, &expected, false).expect_err("musl");
    assert!(
        error
            .to_string()
            .contains("helper-deploy-destination-unsupported"),
        "{error}"
    );
    let mut unsupported = linux_absent();
    unsupported.machine = "riscv64".into();
    let error = decide(&unsupported, &expected, false).expect_err("unsupported machine");
    assert!(
        error
            .to_string()
            .contains("helper-deploy-destination-unsupported"),
        "{error}"
    );
    for (glibc, accepted) in [
        ((3, 0), true),
        ((2, 100), true),
        ((2, 28), true),
        ((2, 27), false),
        ((1, 99), false),
    ] {
        let mut linux = linux_absent();
        linux.glibc = Some(glibc);
        assert_eq!(
            decide(&linux, &expected, false).is_ok(),
            accepted,
            "{glibc:?}"
        );
    }
    let intel = Preflight {
        system: "Darwin".into(),
        machine: "x86_64".into(),
        glibc: None,
        existing: Existing::Absent,
    };
    assert!(matches!(
        decide(&intel, &expected, false).expect("intel macos"),
        Decision::Install { ref destination, .. } if destination.target == "x86_64-apple-darwin"
    ));
    let macos = Preflight {
        system: "Darwin".into(),
        machine: "arm64".into(),
        glibc: None,
        existing: Existing::Absent,
    };
    assert!(matches!(
        decide(&macos, &expected, false).expect("macos needs no glibc"),
        Decision::Install { ref destination, .. } if destination.platform == "macos" && destination.glibc.is_none()
    ));
}

/// A scripted destination: each entry answers the next remote command.
struct Scripted {
    answers: Vec<SessionOutput>,
    commands: Vec<(String, Vec<u8>, usize)>,
}

impl Scripted {
    fn new(answers: Vec<SessionOutput>) -> Self {
        Self {
            answers,
            commands: Vec::new(),
        }
    }
}

impl SessionRunner for Scripted {
    async fn run(
        &mut self,
        remote_command: &str,
        stdin: Vec<u8>,
        stdout_limit: usize,
    ) -> Result<SessionOutput, agentenv::error::AppError> {
        self.commands
            .push((remote_command.to_owned(), stdin, stdout_limit));
        Ok(self.answers.remove(0))
    }
}

struct Bytes(Vec<u8>, std::sync::Arc<std::sync::atomic::AtomicBool>);

impl HelperSource for Bytes {
    async fn bytes(
        self,
        destination: &Destination,
    ) -> Result<HelperBytes, agentenv::error::AppError> {
        self.1.store(true, std::sync::atomic::Ordering::SeqCst);
        Ok(HelperBytes {
            kind: "file",
            name: format!("fixture-for-{}", destination.target),
            bytes: self.0,
        })
    }
}

fn ok(stdout: &[u8]) -> SessionOutput {
    SessionOutput {
        status: Some(0),
        stdout: stdout.to_vec(),
    }
}

fn consulted() -> std::sync::Arc<std::sync::atomic::AtomicBool> {
    std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false))
}

#[tokio::test]
async fn deployment_runs_preflight_then_install_with_the_source_bytes() {
    let expected = identity();
    let mut runner = Scripted::new(vec![
        ok(&preflight_output(
            "Linux",
            "aarch64",
            "glibc 2.36",
            "helper agentenv-sudo-helper 1 0.1.0",
        )),
        ok(format!("{expected}\n").as_bytes()),
    ]);
    let flag = consulted();
    let report = deploy(
        &mut runner,
        Path::new(HELPER),
        false,
        Bytes(b"ELF-bytes".to_vec(), flag.clone()),
    )
    .await
    .expect("deploys");
    assert_eq!(report.status.label(), "deployed");
    assert_eq!(report.helper_path, HELPER);
    assert_eq!(report.destination.target, "aarch64-unknown-linux-gnu");
    assert_eq!(report.destination.platform, "linux");
    assert_eq!(
        report.previous.as_deref(),
        Some("agentenv-sudo-helper 1 0.1.0")
    );
    assert_eq!(report.installed, expected);
    assert_eq!(
        report
            .source()
            .map(|source| (source.kind, source.name.as_str())),
        Some(("file", "fixture-for-aarch64-unknown-linux-gnu"))
    );
    assert!(flag.load(std::sync::atomic::Ordering::SeqCst));
    assert_eq!(runner.commands.len(), 2);
    assert_eq!(runner.commands[0].0, preflight_command(HELPER));
    assert!(
        runner.commands[0].1.is_empty(),
        "the preflight sends no stdin"
    );
    assert_eq!(runner.commands[0].2, SESSION_OUTPUT_LIMIT);
    assert_eq!(runner.commands[1].0, install_command(HELPER, &expected, 9));
    assert_eq!(runner.commands[1].1, b"ELF-bytes");
}

#[tokio::test]
async fn up_to_date_destination_skips_the_source_and_the_install_session() {
    let expected = identity();
    let mut runner = Scripted::new(vec![ok(&preflight_output(
        "Darwin",
        "arm64",
        "none",
        &format!("helper {expected}"),
    ))]);
    let flag = consulted();
    let report = deploy(
        &mut runner,
        Path::new(HELPER),
        false,
        Bytes(vec![1], flag.clone()),
    )
    .await
    .expect("up to date");
    assert_eq!(report.status, Status::UpToDate);
    assert_eq!(report.source(), None);
    assert_eq!(report.previous.as_deref(), Some(expected.as_str()));
    assert_eq!(report.installed, expected);
    assert_eq!(runner.commands.len(), 1);
    assert!(!flag.load(std::sync::atomic::Ordering::SeqCst));
}

#[tokio::test]
async fn refusals_before_install_never_consult_the_source() {
    for (name, answer, reason) in [
        (
            "occupied",
            ok(&preflight_output(
                "Linux",
                "x86_64",
                "glibc 2.36",
                "occupied",
            )),
            Reason::PathOccupied,
        ),
        (
            "banner",
            ok(b"Welcome\nLinux\nx86_64\nglibc 2.36\nabsent\n"),
            Reason::PreflightUnparseable,
        ),
        (
            "nonzero preflight",
            SessionOutput {
                status: Some(127),
                stdout: Vec::new(),
            },
            Reason::PreflightUnparseable,
        ),
        (
            "preflight without an exit status",
            SessionOutput {
                status: None,
                stdout: Vec::new(),
            },
            Reason::SessionFailed,
        ),
        (
            "musl",
            ok(&preflight_output("Linux", "x86_64", "none", "absent")),
            Reason::DestinationUnsupported,
        ),
    ] {
        let mut runner = Scripted::new(vec![answer]);
        let flag = consulted();
        let error = deploy(
            &mut runner,
            Path::new(HELPER),
            false,
            Bytes(vec![1], flag.clone()),
        )
        .await
        .expect_err(name);
        assert!(
            error
                .to_string()
                .contains(&format!("helper-deploy-{}", reason.code())),
            "{name}: {error}"
        );
        assert_eq!(runner.commands.len(), 1, "{name}");
        assert!(!flag.load(std::sync::atomic::Ordering::SeqCst), "{name}");
    }
    let mut runner = Scripted::new(vec![]);
    let error = deploy(
        &mut runner,
        Path::new("/bad path/helper"),
        false,
        Bytes(vec![1], consulted()),
    )
    .await
    .expect_err("invalid path");
    assert!(error
        .to_string()
        .contains("helper-deploy-invalid-helper-path"));
    assert!(
        runner.commands.is_empty(),
        "an invalid path never opens a session"
    );
}

#[tokio::test]
async fn install_statuses_map_to_distinct_reasons_and_bad_bytes_are_refused() {
    let expected = identity();
    for (status, stdout, reason) in [
        (
            Some(0),
            "agentenv-sudo-helper 1 0.0.1\n".to_owned(),
            Reason::IdentityMismatch,
        ),
        (Some(0), String::new(), Reason::IdentityMismatch),
        (Some(21), String::new(), Reason::ReplaceFailed),
        (Some(22), String::new(), Reason::ReplaceFailed),
        (Some(23), String::new(), Reason::PathOccupied),
        (Some(24), String::new(), Reason::UploadFailed),
        (Some(25), String::new(), Reason::UploadFailed),
        (Some(26), String::new(), Reason::IdentityMismatch),
        (Some(27), String::new(), Reason::ReplaceFailed),
        (Some(255), String::new(), Reason::SessionFailed),
        (Some(1), String::new(), Reason::UploadFailed),
        (None, format!("{expected}\n"), Reason::SessionFailed),
    ] {
        let mut runner = Scripted::new(vec![
            ok(&preflight_output("Linux", "x86_64", "glibc 2.36", "absent")),
            SessionOutput {
                status,
                stdout: stdout.into_bytes(),
            },
        ]);
        let error = deploy(
            &mut runner,
            Path::new(HELPER),
            false,
            Bytes(vec![1], consulted()),
        )
        .await
        .expect_err("install failure");
        assert!(
            error
                .to_string()
                .contains(&format!("helper-deploy-{}", reason.code())),
            "{status:?}: {error}"
        );
    }
    for bytes in [Vec::new(), vec![0; HELPER_UPLOAD_LIMIT + 1]] {
        let mut runner = Scripted::new(vec![ok(&preflight_output(
            "Linux",
            "x86_64",
            "glibc 2.36",
            "absent",
        ))]);
        let error = deploy(
            &mut runner,
            Path::new(HELPER),
            false,
            Bytes(bytes, consulted()),
        )
        .await
        .expect_err("bad bytes");
        assert!(
            error
                .to_string()
                .contains("helper-deploy-source-unavailable"),
            "{error}"
        );
        assert_eq!(
            runner.commands.len(),
            1,
            "bad bytes never open the install session"
        );
    }
}

#[test]
fn expected_identity_matches_the_helper_binary_format_and_the_safe_grammar() {
    assert_eq!(expected_identity(), identity());
    assert_eq!(
        parse_identity(&expected_identity()),
        Some(identity().as_str()),
        "the interpolated identity stays inside the quoting-safe grammar"
    );
    assert_eq!(
        parse_identity("agentenv-sudo-helper 1 0.3.0-rc.1+build.5"),
        Some("agentenv-sudo-helper 1 0.3.0-rc.1+build.5")
    );
    for rejected in [
        "agentenv-sudo-helper 1",
        "agentenv-sudo-helper x 0.2.0",
        "agentenv-sudo-helper 1 0.2.0 extra",
        "agentenv-sudo-helper 1 0.2.0'",
        "agentenv-sudo-helper 1 0.2.0!",
        "agentenv 1 0.2.0",
    ] {
        assert_eq!(parse_identity(rejected), None, "{rejected}");
    }
    let (tag, name) = release_asset_name("aarch64-unknown-linux-gnu");
    assert_eq!(tag, format!("v{}", env!("CARGO_PKG_VERSION")));
    assert_eq!(
        name,
        format!("agentenv-sudo-helper-{tag}-aarch64-unknown-linux-gnu")
    );
}

#[test]
fn glibc_lines_accept_only_dotted_digit_versions() {
    for (line, expected) in [
        ("glibc 2.28", Some((2, 28))),
        ("glibc 2.40.9000", Some((2, 40))),
        ("none", None),
    ] {
        assert_eq!(
            parse_preflight(&preflight_output("Linux", "x86_64", line, "absent"))
                .expect(line)
                .glibc,
            expected,
            "{line}"
        );
    }
    for line in [
        "glibc +2.28",
        "glibc 2.28.x",
        "glibc 2",
        "glibc 2.",
        "glibc  2.28",
        "GLIBC 2.28",
        "glibc 2.28.1.1",
        "musl 1.2",
    ] {
        let error =
            parse_preflight(&preflight_output("Linux", "x86_64", line, "absent")).expect_err(line);
        assert!(
            error.to_string().contains("preflight-unparseable"),
            "{line}: {error}"
        );
    }
    for output in [
        b"Linux\r\nx86_64\r\nglibc 2.36\r\nabsent\r\n".to_vec(),
        b"motd\nLinux\nx86_64\nnone\nabsent\n".to_vec(),
    ] {
        let result = parse_preflight(&output);
        assert!(
            !matches!(
                result,
                Ok(Preflight {
                    existing: Existing::Absent | Existing::Helper(_),
                    ..
                })
            ),
            "a banner or CRLF never yields an installable state: {result:?}"
        );
    }
}

fn destination_for(target: &'static str) -> Destination {
    Destination {
        platform: if target.contains("linux") {
            "linux"
        } else {
            "macos"
        },
        machine: "x86_64".into(),
        target,
        glibc: None,
    }
}

const UNREACHABLE_RELEASES: &str = "http://127.0.0.1:9/releases";

#[tokio::test]
async fn file_source_requires_a_nonempty_regular_file() {
    let root = tempfile::tempdir().expect("tempdir");
    let good = root.path().join("helper.bin");
    std::fs::write(&good, b"bytes").unwrap();
    let bytes = Source::File(good.clone())
        .bytes(&destination_for("x86_64-unknown-linux-gnu"))
        .await
        .expect("regular file");
    assert_eq!(
        (bytes.kind, bytes.bytes.as_slice()),
        ("file", &b"bytes"[..])
    );
    assert_eq!(bytes.name, good.display().to_string());
    let empty = root.path().join("empty");
    std::fs::write(&empty, b"").unwrap();
    let mut rejected = vec![
        ("empty", empty),
        ("directory", root.path().to_path_buf()),
        ("missing", root.path().join("missing")),
    ];
    #[cfg(unix)]
    {
        // A FIFO with no writer would block a naive open forever.
        let fifo = root.path().join("fifo");
        let made = std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .map(|status| status.success())
            .unwrap_or(false);
        if made {
            rejected.push(("fifo", fifo));
        }
    }
    for (name, path) in rejected {
        let error = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            Source::File(path).bytes(&destination_for("x86_64-unknown-linux-gnu")),
        )
        .await
        .expect("classification never blocks")
        .expect_err(name);
        assert!(
            error
                .to_string()
                .contains("helper-deploy-source-unavailable"),
            "{name}: {error}"
        );
        assert!(error.to_string().contains("--from"), "{name}: {error}");
    }
}

#[cfg(unix)]
#[tokio::test]
async fn automatic_source_prefers_a_matching_bundle_and_otherwise_needs_the_release() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().expect("tempdir");
    let bundle = root.path().join("agentenv-sudo-helper");
    let script = format!("#!/bin/sh\necho '{}'\n", identity());
    std::fs::write(&bundle, &script).unwrap();
    std::fs::set_permissions(&bundle, std::fs::Permissions::from_mode(0o755)).unwrap();
    let local = agentenv::update::TARGET;
    let bytes = Source::Automatic {
        bundle: bundle.clone(),
        release_base_url: UNREACHABLE_RELEASES.into(),
    }
    .bytes(&destination_for(local))
    .await
    .expect("matching bundle");
    assert_eq!(bytes.kind, "bundle");
    assert_eq!(bytes.bytes, script.as_bytes());

    // A different destination target never uses the local companion.
    let other = if local == "aarch64-apple-darwin" {
        "x86_64-unknown-linux-gnu"
    } else {
        "aarch64-apple-darwin"
    };
    let error = Source::Automatic {
        bundle: bundle.clone(),
        release_base_url: UNREACHABLE_RELEASES.into(),
    }
    .bytes(&destination_for(other))
    .await
    .expect_err("release unreachable");
    assert!(
        error
            .to_string()
            .contains("helper-deploy-source-unavailable"),
        "{error}"
    );
    assert!(error.to_string().contains("--from"), "{error}");

    // A bundle with another identity, or none at all, falls through too.
    std::fs::write(&bundle, "#!/bin/sh\necho 'agentenv-sudo-helper 1 0.0.1'\n").unwrap();
    let error = Source::Automatic {
        bundle: bundle.clone(),
        release_base_url: UNREACHABLE_RELEASES.into(),
    }
    .bytes(&destination_for(local))
    .await
    .expect_err("stale bundle falls through to the release");
    assert!(
        error
            .to_string()
            .contains("helper-deploy-source-unavailable"),
        "{error}"
    );
    let error = Source::Automatic {
        bundle: root.path().join("missing"),
        release_base_url: UNREACHABLE_RELEASES.into(),
    }
    .bytes(&destination_for(local))
    .await
    .expect_err("missing bundle falls through to the release");
    assert!(
        error
            .to_string()
            .contains("helper-deploy-source-unavailable"),
        "{error}"
    );
}

/// Runs the rendered remote commands the way sshd does: the login shell
/// receives the whole remote command as one `-c` string.
#[cfg(unix)]
mod destination {
    use super::*;
    use std::fs;
    use std::io::Write;
    use std::os::unix::fs::PermissionsExt;
    use std::process::{Command, Stdio};

    struct Run {
        status: Option<i32>,
        stdout: String,
    }

    fn login_shell(shell: &str, remote_command: &str, stdin: &[u8]) -> Run {
        let mut child = Command::new(shell)
            .arg("-c")
            .arg(remote_command)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("login shell spawns");
        child
            .stdin
            .take()
            .expect("piped stdin")
            .write_all(stdin)
            .expect("stdin written");
        let output = child.wait_with_output().expect("login shell exits");
        Run {
            status: output.status.code(),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        }
    }

    fn shells() -> Vec<&'static str> {
        [
            "/bin/sh",
            "/bin/bash",
            "/bin/zsh",
            "/bin/dash",
            "/bin/csh",
            "/bin/tcsh",
            "/bin/ksh",
        ]
        .into_iter()
        .filter(|shell| Path::new(shell).is_file())
        .collect()
    }

    fn helper_script(identity: &str) -> Vec<u8> {
        format!("#!/bin/sh\ncase \"$1\" in --identity) echo '{identity}';; *) exit 3;; esac\n")
            .into_bytes()
    }

    fn preflight(shell: &str, path: &str) -> Run {
        login_shell(shell, &preflight_command(path), b"")
    }

    fn install(shell: &str, path: &str, expected: &str, bytes: &[u8], declared: usize) -> Run {
        login_shell(shell, &install_command(path, expected, declared), bytes)
    }

    fn mode(path: &Path) -> u32 {
        fs::metadata(path).expect("metadata").permissions().mode() & 0o7777
    }

    /// Temporary upload files beside `path`, whatever pid suffix they carry.
    fn temporaries(path: &Path) -> Vec<String> {
        let prefix = format!(
            "{}.agentenv-new",
            path.file_name().unwrap().to_str().unwrap()
        );
        let Ok(entries) = fs::read_dir(path.parent().unwrap()) else {
            return Vec::new();
        };
        let mut names: Vec<String> = entries
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|name| name.starts_with(&prefix))
            .collect();
        names.sort();
        names
    }

    #[test]
    fn install_and_preflight_behave_identically_under_every_login_shell() {
        let expected = identity();
        for shell in shells() {
            let root = tempfile::tempdir().expect("tempdir");
            let path = root.path().join("lib+exec/v1.0/agentenv-sudo-helper");
            let path_text = path.to_str().expect("UTF-8 tempdir");

            let run = preflight(shell, path_text);
            assert_eq!(run.status, Some(0), "{shell}");
            assert!(
                run.stdout.ends_with("\nabsent\n"),
                "{shell}: {}",
                run.stdout
            );
            let parsed = parse_preflight(run.stdout.as_bytes()).expect("real preflight parses");
            assert_eq!(parsed.existing, Existing::Absent);

            let script = helper_script(&expected);
            let run = install(shell, path_text, &expected, &script, script.len());
            assert_eq!(run.status, Some(0), "{shell}");
            assert_eq!(run.stdout, format!("{expected}\n"), "{shell}");
            assert_eq!(mode(&path), 0o755, "{shell}");
            assert_eq!(mode(path.parent().unwrap()), 0o700, "{shell}");
            assert!(
                temporaries(&path).is_empty(),
                "{shell}: temporary file removed"
            );
            assert_eq!(fs::read(&path).unwrap(), script);

            let run = preflight(shell, path_text);
            assert_eq!(run.status, Some(0), "{shell}");
            assert_eq!(
                parse_preflight(run.stdout.as_bytes()).unwrap().existing,
                Existing::Helper(expected.clone()),
                "{shell}"
            );
        }
    }

    #[test]
    fn wrong_identity_truncated_or_clobbered_uploads_never_replace_the_helper() {
        let expected = identity();
        let root = tempfile::tempdir().expect("tempdir");
        let path = root.path().join("agentenv-sudo-helper");
        let path_text = path.to_str().unwrap();
        let current = helper_script("agentenv-sudo-helper 1 0.0.1");
        fs::write(&path, &current).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();

        // Wrong identity is refused on the destination before the rename.
        let wrong = helper_script("agentenv-sudo-helper 1 9.9.9");
        let run = install("/bin/sh", path_text, &expected, &wrong, wrong.len());
        assert_eq!(run.status, Some(26));
        assert!(run.stdout.is_empty());
        assert_eq!(
            fs::read(&path).unwrap(),
            current,
            "previous helper untouched"
        );
        assert!(temporaries(&path).is_empty());

        // A truncated upload fails the size check.
        let good = helper_script(&expected);
        let run = install(
            "/bin/sh",
            path_text,
            &expected,
            &good[..good.len() - 4],
            good.len(),
        );
        assert_eq!(run.status, Some(24));
        assert_eq!(fs::read(&path).unwrap(), current);
        assert!(temporaries(&path).is_empty());

        // Something that does not run at all.
        let run = install("/bin/sh", path_text, &expected, b"not a program", 13);
        assert_eq!(run.status, Some(26));
        assert_eq!(fs::read(&path).unwrap(), current);
        assert!(temporaries(&path).is_empty());

        // Stale temporary files, with or without a pid suffix, are removed
        // first, then the upgrade succeeds.
        fs::write(
            root.path().join("agentenv-sudo-helper.agentenv-new"),
            b"stale",
        )
        .unwrap();
        fs::write(
            root.path().join("agentenv-sudo-helper.agentenv-new.4242"),
            b"stale",
        )
        .unwrap();
        let run = install("/bin/sh", path_text, &expected, &good, good.len());
        assert_eq!(run.status, Some(0), "{}", run.stdout);
        assert_eq!(fs::read(&path).unwrap(), good);
        assert!(temporaries(&path).is_empty());
    }

    #[test]
    fn occupied_paths_are_reported_and_refused_and_keywords_cannot_be_forged() {
        let expected = identity();
        let root = tempfile::tempdir().expect("tempdir");
        let good = helper_script(&expected);

        let forger = root.path().join("forger");
        fs::write(&forger, "#!/bin/sh\necho absent\n").unwrap();
        fs::set_permissions(&forger, fs::Permissions::from_mode(0o755)).unwrap();
        let run = preflight("/bin/sh", forger.to_str().unwrap());
        assert_eq!(
            parse_preflight(run.stdout.as_bytes()).unwrap().existing,
            Existing::Occupied
        );

        let silent = root.path().join("silent");
        fs::write(&silent, "#!/bin/sh\nexit 0\n").unwrap();
        fs::set_permissions(&silent, fs::Permissions::from_mode(0o755)).unwrap();
        let run = preflight("/bin/sh", silent.to_str().unwrap());
        assert_eq!(
            parse_preflight(run.stdout.as_bytes()).unwrap().existing,
            Existing::Occupied
        );

        let plain = root.path().join("plain");
        fs::write(&plain, b"data").unwrap();
        let run = preflight("/bin/sh", plain.to_str().unwrap());
        assert_eq!(
            parse_preflight(run.stdout.as_bytes()).unwrap().existing,
            Existing::Occupied
        );

        let link = root.path().join("link");
        std::os::unix::fs::symlink(&plain, &link).unwrap();
        let run = preflight("/bin/sh", link.to_str().unwrap());
        assert_eq!(
            parse_preflight(run.stdout.as_bytes()).unwrap().existing,
            Existing::Occupied
        );
        let run = install(
            "/bin/sh",
            link.to_str().unwrap(),
            &expected,
            &good,
            good.len(),
        );
        assert_eq!(run.status, Some(23));
        assert!(link.symlink_metadata().unwrap().file_type().is_symlink());
        assert_eq!(fs::read(&plain).unwrap(), b"data");

        let directory = root.path().join("dir");
        fs::create_dir(&directory).unwrap();
        let run = preflight("/bin/sh", directory.to_str().unwrap());
        assert_eq!(
            parse_preflight(run.stdout.as_bytes()).unwrap().existing,
            Existing::Occupied
        );
        let run = install(
            "/bin/sh",
            directory.to_str().unwrap(),
            &expected,
            &good,
            good.len(),
        );
        assert_eq!(run.status, Some(23));
        assert!(directory.is_dir());
        assert!(!root.path().join("dir.agentenv-new").exists());
    }

    /// A directory or a symlink to a directory appearing at the path while
    /// the upload is in flight would otherwise receive the verified file
    /// from `mv`; the check immediately before the rename refuses it.
    #[test]
    fn a_path_swapped_during_the_upload_is_refused_before_the_rename() {
        let expected = identity();
        let good = helper_script(&expected);
        for swap in ["directory", "symlink to a directory"] {
            let root = tempfile::tempdir().expect("tempdir");
            let path = root.path().join("agentenv-sudo-helper");
            let elsewhere = root.path().join("elsewhere");
            fs::create_dir(&elsewhere).unwrap();
            let mut child = Command::new("/bin/sh")
                .arg("-c")
                .arg(install_command(
                    path.to_str().unwrap(),
                    &expected,
                    good.len(),
                ))
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
                .expect("install starts");
            let mut stdin = child.stdin.take().expect("piped stdin");
            stdin.write_all(&good[..1]).unwrap();
            let started = std::time::Instant::now();
            while temporaries(&path).is_empty() {
                assert!(
                    started.elapsed() < std::time::Duration::from_secs(10),
                    "{swap}: the upload never started"
                );
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            if swap == "directory" {
                fs::create_dir(&path).unwrap();
            } else {
                std::os::unix::fs::symlink(&elsewhere, &path).unwrap();
            }
            stdin.write_all(&good[1..]).unwrap();
            drop(stdin);
            let output = child.wait_with_output().expect("install exits");
            assert_eq!(output.status.code(), Some(23), "{swap}");
            assert!(output.stdout.is_empty(), "{swap}");
            assert!(
                temporaries(&path).is_empty(),
                "{swap}: temporary file removed"
            );
            let inside = |directory: &Path| fs::read_dir(directory).unwrap().count();
            if swap == "directory" {
                assert!(path.is_dir(), "{swap}");
                assert_eq!(inside(&path), 0, "{swap}: nothing moved into it");
            } else {
                assert!(path.symlink_metadata().unwrap().file_type().is_symlink());
                assert_eq!(inside(&elsewhere), 0, "{swap}: nothing moved through it");
            }
        }
    }

    /// Two deployments at once: the later one removes the earlier one's
    /// temporary file as stale, so the earlier one fails and never moves the
    /// later one's upload; the path only ever receives a verified file. The
    /// first run's helper blocks inside `--identity` until released, while
    /// the second run is mid-upload with different bytes; a shared temporary
    /// name would let the first run rename that partial upload into place.
    #[test]
    fn a_concurrent_install_never_moves_the_other_runs_upload_into_place() {
        let expected = identity();
        let root = tempfile::tempdir().expect("tempdir");
        let path = root.path().join("agentenv-sudo-helper");
        let path_text = path.to_str().unwrap();
        let started = root.path().join("identity-started");
        let gate = root.path().join("identity-gate");
        let blocking = format!(
            "#!/bin/sh\ncase \"$1\" in --identity) : > '{}'; while [ ! -e '{}' ]; do sleep 0.02; done; echo '{expected}';; *) exit 3;; esac\n",
            started.display(),
            gate.display()
        )
        .into_bytes();
        let mut second_bytes = helper_script(&expected);
        second_bytes.extend_from_slice(b"# second run\n");
        let wait_for = |condition: &dyn Fn() -> bool, what: &str| {
            let begun = std::time::Instant::now();
            while !condition() {
                assert!(
                    begun.elapsed() < std::time::Duration::from_secs(10),
                    "{what} never happened"
                );
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
        };
        let spawn = |declared: usize| {
            Command::new("/bin/sh")
                .arg("-c")
                .arg(install_command(path_text, &expected, declared))
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
                .expect("install starts")
        };

        // The first run uploads completely and blocks in --identity.
        let mut first = spawn(blocking.len());
        let mut first_stdin = first.stdin.take().expect("piped stdin");
        first_stdin.write_all(&blocking).unwrap();
        drop(first_stdin);
        wait_for(&|| started.exists(), "the first run's identity check");

        // The second run removes stale temporaries and starts its upload.
        let mut second = spawn(second_bytes.len());
        let mut second_stdin = second.stdin.take().expect("piped stdin");
        second_stdin.write_all(&second_bytes[..1]).unwrap();
        wait_for(
            &|| {
                temporaries(&path).iter().any(|name| {
                    fs::metadata(path.parent().unwrap().join(name))
                        .map(|metadata| metadata.len() == 1)
                        .unwrap_or(false)
                })
            },
            "the second run's partial upload",
        );

        // Released, the first run cannot rename anything into place.
        fs::write(&gate, b"").unwrap();
        let first = first.wait_with_output().expect("first install exits");
        assert_eq!(
            first.status.code(),
            Some(27),
            "the first run's file is gone"
        );
        assert!(first.stdout.is_empty());
        assert!(!path.exists(), "no partial upload reached the path");

        // The second run completes normally.
        second_stdin.write_all(&second_bytes[1..]).unwrap();
        drop(second_stdin);
        let second = second.wait_with_output().expect("second install exits");
        assert_eq!(second.status.code(), Some(0), "{:?}", second.stdout);
        assert_eq!(fs::read(&path).unwrap(), second_bytes);
        assert_eq!(mode(&path), 0o755);
        assert!(temporaries(&path).is_empty());
    }
}
