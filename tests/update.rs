//! Integration coverage for `agentenv update` against a local release server.
//!
//! Each test copies the built binary into its own directory and points it at
//! a loopback HTTP server that serves a fabricated release, so the binary the
//! rest of the suite runs is never replaced. The released "binary" is the
//! `test-probe` fixture, which answers `--version` with the version the test
//! announces through `TEST_PROBE_VERSION`.

mod helpers;

use std::fs::{self, File};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::thread;

use assert_cmd::cargo::cargo_bin;
use sha2::{Digest, Sha256};
use tempfile::TempDir;

use helpers::{assert_exit, assert_mentions, Run};

const TARGET: &str = env!("AGENTENV_TARGET");
const CURRENT: &str = env!("CARGO_PKG_VERSION");
const NEXT: &str = "9.9.9";

#[cfg(windows)]
const ARCHIVE_EXTENSION: &str = "zip";
#[cfg(not(windows))]
const ARCHIVE_EXTENSION: &str = "tar.gz";
#[cfg(windows)]
const BINARY_NAME: &str = "agentenv.exe";
#[cfg(not(windows))]
const BINARY_NAME: &str = "agentenv";

/// A release staged for one tag: the archive under `download/<tag>/` and a
/// `SHA256SUMS` that also answers `latest/download/`.
struct Server {
    root: TempDir,
    base_url: String,
}

impl Server {
    fn start() -> Self {
        let root = TempDir::new().expect("a server root");
        let listener = TcpListener::bind("127.0.0.1:0").expect("a loopback listener");
        let port = listener.local_addr().expect("a bound address").port();
        let files = root.path().to_path_buf();
        thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                serve(stream, &files);
            }
        });
        Self {
            root,
            base_url: format!("http://127.0.0.1:{port}/releases"),
        }
    }

    /// Publishes `tag` with the probe as its binary and `skill_text` as the
    /// packaged `SKILL.md`; `digest_override` corrupts the checksum entry.
    fn publish(
        &self,
        tag: &str,
        skill_text: Option<&str>,
        digest_override: Option<&str>,
        current_bundle: bool,
        omit: Option<&str>,
    ) {
        let stem = format!("agentenv-{tag}-{TARGET}");
        let staging = self.root.path().join("staging").join(&stem);
        fs::create_dir_all(&staging).expect("staging dir");
        let main = if current_bundle {
            cargo_bin("agentenv")
        } else {
            cargo_bin("test-probe")
        };
        fs::copy(main, staging.join(BINARY_NAME)).expect("main copied");
        for name in ["agentenv-sudo-helper", "agentenv-ssh-askpass"] {
            let packaged = format!("{name}{}", std::env::consts::EXE_SUFFIX);
            if omit == Some(name) {
                continue;
            }
            fs::copy(cargo_bin(name), staging.join(packaged)).expect("companion copied");
        }
        if let Some(text) = skill_text {
            let skill = staging.join("skills").join("agentenv");
            fs::create_dir_all(&skill).expect("skill dir");
            fs::write(skill.join("SKILL.md"), text).expect("skill written");
        }
        let asset = format!("{stem}.{ARCHIVE_EXTENSION}");
        let download = self.root.path().join("download").join(tag);
        fs::create_dir_all(&download).expect("download dir");
        let archive = download.join(&asset);
        pack(&staging, &stem, &archive);
        let digest = match digest_override {
            Some(digest) => digest.to_owned(),
            None => {
                let bytes = fs::read(&archive).expect("archive read");
                Sha256::digest(&bytes)
                    .iter()
                    .map(|byte| format!("{byte:02x}"))
                    .collect()
            }
        };
        let sums = format!("{digest}  {asset}\n");
        fs::write(download.join("SHA256SUMS"), &sums).expect("sums written");
        let latest = self.root.path().join("latest").join("download");
        fs::create_dir_all(&latest).expect("latest dir");
        fs::write(latest.join("SHA256SUMS"), &sums).expect("latest sums written");
    }
}

impl Server {
    /// Publishes one standalone asset for `tag` and lists it in that tag's
    /// `SHA256SUMS`; `digest_override` corrupts its entry.
    fn publish_asset(&self, tag: &str, name: &str, bytes: &[u8], digest_override: Option<&str>) {
        let download = self.root.path().join("download").join(tag);
        fs::create_dir_all(&download).expect("download dir");
        fs::write(download.join(name), bytes).expect("asset written");
        let digest = digest_override.map_or_else(
            || {
                Sha256::digest(bytes)
                    .iter()
                    .map(|byte| format!("{byte:02x}"))
                    .collect::<String>()
            },
            str::to_owned,
        );
        let sums = download.join("SHA256SUMS");
        let mut existing = fs::read_to_string(&sums).unwrap_or_default();
        existing.push_str(&format!("{digest}  {name}\n"));
        fs::write(sums, existing).expect("sums written");
    }
}

fn serve(mut stream: TcpStream, files: &Path) {
    let mut reader = BufReader::new(stream.try_clone().expect("stream clone"));
    let mut request_line = String::new();
    if reader.read_line(&mut request_line).is_err() {
        return;
    }
    let mut line = String::new();
    while reader.read_line(&mut line).is_ok() && line != "\r\n" && !line.is_empty() {
        line.clear();
    }
    let path = request_line
        .split_whitespace()
        .nth(1)
        .and_then(|path| path.strip_prefix("/releases/"))
        .map(|path| files.join(path));
    let response = match path.and_then(|path| fs::read(path).ok()) {
        Some(body) => {
            let mut response = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            )
            .into_bytes();
            response.extend(body);
            response
        }
        None => {
            b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec()
        }
    };
    let _ = stream.write_all(&response);
    let _ = stream.flush();
}

#[cfg(not(windows))]
fn pack(staging: &Path, stem: &str, archive: &Path) {
    let file = File::create(archive).expect("archive created");
    let encoder = flate2::write::GzEncoder::new(file, flate2::Compression::fast());
    let mut tar = tar::Builder::new(encoder);
    tar.append_dir_all(stem, staging)
        .expect("archive populated");
    tar.into_inner()
        .expect("tar finished")
        .finish()
        .expect("gzip finished");
}

#[cfg(windows)]
fn pack(staging: &Path, stem: &str, archive: &Path) {
    use zip::write::SimpleFileOptions;

    let file = File::create(archive).expect("archive created");
    let mut zip = zip::ZipWriter::new(file);
    let options = SimpleFileOptions::default();
    fn add(zip: &mut zip::ZipWriter<File>, dir: &Path, prefix: &str, options: SimpleFileOptions) {
        for entry in fs::read_dir(dir).expect("dir read") {
            let entry = entry.expect("entry");
            let name = format!("{prefix}/{}", entry.file_name().to_string_lossy());
            if entry.file_type().expect("file type").is_dir() {
                zip.add_directory(&name, options).expect("dir added");
                add(zip, &entry.path(), &name, options);
            } else {
                zip.start_file(&name, options).expect("file started");
                let mut source = File::open(entry.path()).expect("file opened");
                std::io::copy(&mut source, zip).expect("file copied");
            }
        }
    }
    add(&mut zip, staging, stem, options);
    zip.finish().expect("zip finished");
}

/// An isolated install: a private copy of the binary and a private home.
struct Install {
    _dir: TempDir,
    binary: PathBuf,
    home: PathBuf,
}

impl Install {
    fn new(bin_segments: &[&str]) -> Self {
        let dir = TempDir::new().expect("an install dir");
        let mut bin_dir = dir.path().to_path_buf();
        bin_dir.extend(bin_segments);
        fs::create_dir_all(&bin_dir).expect("bin dir");
        let binary = bin_dir.join(BINARY_NAME);
        fs::copy(cargo_bin("agentenv"), &binary).expect("binary copied");
        let home = dir.path().join("home");
        fs::create_dir_all(&home).expect("home dir");
        Self {
            _dir: dir,
            binary,
            home,
        }
    }

    fn skill(&self, root: &[&str]) -> PathBuf {
        let mut path = self.home.clone();
        path.extend(root);
        path.join("agentenv").join("SKILL.md")
    }

    fn seed_skill(&self, root: &[&str], text: &str) {
        let skill = self.skill(root);
        fs::create_dir_all(skill.parent().expect("skill dir")).expect("skill dir created");
        fs::write(skill, text).expect("skill seeded");
    }

    fn run(&self, server: &Server, args: &[&str]) -> Run {
        let mut command = std::process::Command::new(&self.binary);
        command.env_clear();
        for name in ["PATH", "SYSTEMROOT"] {
            if let Some(value) = std::env::var_os(name) {
                command.env(name, value);
            }
        }
        let home_name = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
        command
            .current_dir(&self.home)
            .env(home_name, &self.home)
            .env("APPDATA", self.home.join("AppData").join("Roaming"))
            .env("AGENTENV_RELEASE_BASE_URL", &server.base_url)
            .env("TEST_PROBE_VERSION", NEXT)
            .args(args);
        let output = command.output().expect("agentenv runs");
        Run {
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            code: output.status.code(),
        }
    }

    fn binary_is_original(&self) -> bool {
        let original = fs::read(cargo_bin("agentenv")).expect("original read");
        fs::read(&self.binary).expect("installed read") == original
    }
}

fn read(path: &Path) -> String {
    let mut text = String::new();
    File::open(path)
        .expect("file opens")
        .read_to_string(&mut text)
        .expect("file reads");
    text
}

#[test]
fn check_reports_a_newer_release_without_writing() {
    let server = Server::start();
    server.publish(&format!("v{NEXT}"), Some("new skill"), None, false, None);
    let install = Install::new(&["bin"]);

    let run = install.run(&server, &["update", "--check"]);
    assert_exit(&run, 0, "check succeeds");
    assert_mentions(&run, CURRENT, "the installed version is reported");
    assert_mentions(&run, NEXT, "the available version is reported");
    assert!(
        install.binary_is_original(),
        "check leaves the binary alone"
    );

    let run = install.run(&server, &["--json", "update", "--check"]);
    assert_exit(&run, 0, "json check succeeds");
    let value: serde_json::Value = serde_json::from_str(&run.stdout).expect("json output");
    assert_eq!(value["update_available"], true);
    assert_eq!(value["available"], NEXT);
}

#[test]
fn update_replaces_the_binary_and_installed_skills_only() {
    let server = Server::start();
    server.publish(&format!("v{CURRENT}"), Some("new skill"), None, true, None);
    let install = Install::new(&["bin"]);
    install.seed_skill(&[".agents", "skills"], "old skill");

    let run = install.run(&server, &["update", "--force"]);
    assert_exit(&run, 0, "update succeeds");
    assert_mentions(&run, CURRENT, "the installed version is reported");
    let replaced = install.run(&server, &["--version"]);
    assert_eq!(replaced.stdout.trim(), format!("agentenv {CURRENT}"));
    for name in ["agentenv-sudo-helper", "agentenv-ssh-askpass"] {
        assert!(
            install
                .binary
                .parent()
                .expect("binary directory")
                .join(format!("{name}{}", std::env::consts::EXE_SUFFIX))
                .is_file(),
            "the complete companion bundle is installed"
        );
    }
    assert_eq!(read(&install.skill(&[".agents", "skills"])), "new skill");
    assert!(
        !install.skill(&[".claude", "skills"]).exists(),
        "a skill root that was never installed stays absent"
    );
    let leftovers: Vec<_> = fs::read_dir(install.home.join(".agents").join("skills"))
        .expect("skills root")
        .map(|entry| {
            entry
                .expect("entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    assert_eq!(leftovers, vec!["agentenv"], "no staging directories remain");
}

#[test]
fn update_names_the_ssh_sudo_targets_whose_helpers_need_redeployment() {
    let server = Server::start();
    server.publish(&format!("v{CURRENT}"), Some("skill"), None, true, None);
    let install = Install::new(&["bin"]);
    // The default configuration location the executable resolves for HOME
    // on Unix and for APPDATA on Windows.
    let config_dir = if cfg!(windows) {
        install
            .home
            .join("AppData")
            .join("Roaming")
            .join("agentenv")
    } else {
        install.home.join(".config").join("agentenv")
    };
    fs::create_dir_all(&config_dir).expect("config dir");
    let config_file = config_dir.join("config.toml");
    fs::write(
        &config_file,
        r#"version = 1
default_profile = "work"
[profiles.work]
description = "Work."
[credentials.admin]
description = "Administrator password."
provider = "command"
argv = ["/bin/sh", "-c", "printf secret"]
usages = ["sudo"]
[profiles.work.local_admin]
description = "Local administrator."
kind = "sudo-target"
[profiles.work.local_admin.sudo]
transport = "local"
credential = "credential://admin"
auth_user = "me"
run_as = "root"
sudo_path = "/usr/bin/sudo"
[profiles.work.prod_admin]
description = "Production host."
kind = "sudo-target"
[profiles.work.prod_admin.sudo]
transport = "ssh"
credential = "credential://admin"
auth_user = "deploy"
run_as = "root"
sudo_path = "/usr/bin/sudo"
[profiles.work.prod_admin.sudo.ssh]
mode = "explicit"
hostname = "prod.example.internal"
user = "deploy"
port = 22
host_key_alias = "prod"
known_hosts_file = "/home/me/.ssh/known_hosts"
helper_path = "/home/deploy/.local/libexec/agentenv-sudo-helper"
[profiles.work.prod_admin.sudo.ssh.auth]
method = "publickey"
identity_files = []
use_agent = true
"#,
    )
    .expect("config written");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&config_file, fs::Permissions::from_mode(0o600)).expect("config mode");
    }

    let run = install.run(&server, &["--json", "update", "--force"]);
    assert_exit(&run, 0, "update succeeds");
    let report: serde_json::Value = serde_json::from_str(run.stdout.trim()).expect("JSON report");
    assert_eq!(
        report["helper_redeployments"],
        serde_json::json!([{
            "profile": "work",
            "entry": "prod_admin",
            "command": "agentenv --profile=work sudo --with=prod_admin --deploy-helper",
        }]),
        "only SSH targets are listed, and no remote action is taken"
    );
    let run = install.run(&server, &["update", "--force"]);
    assert_exit(&run, 0, "update succeeds");
    assert_mentions(
        &run,
        "agentenv --profile=work sudo --with=prod_admin --deploy-helper",
        "the text report names the deployment command",
    );
    assert!(!run.stdout.contains("local_admin"), "{}", run.stdout);
}

#[test]
fn update_reports_an_already_current_install() {
    let server = Server::start();
    server.publish(&format!("v{CURRENT}"), None, None, true, None);
    let install = Install::new(&["bin"]);

    let run = install.run(&server, &["update"]);
    assert_exit(&run, 0, "an up-to-date install is not an error");
    assert_mentions(&run, "already", "the report says nothing changed");
    assert!(install.binary_is_original(), "the binary is untouched");
}

#[test]
fn update_refuses_a_silent_downgrade_but_honours_an_explicit_tag() {
    let server = Server::start();
    server.publish("v0.0.1", None, None, false, None);
    let install = Install::new(&["bin"]);

    let run = install.run(&server, &["update"]);
    assert_exit(&run, 7, "an older latest release is refused");
    assert_mentions(
        &run,
        "--version v0.0.1",
        "the refusal names the explicit path",
    );
    assert!(install.binary_is_original(), "the binary is untouched");

    // The published binary reports NEXT rather than 0.0.1, so an explicit
    // downgrade reaches the post-download verification and stops there.
    let run = install.run(&server, &["update", "--version", "v0.0.1"]);
    assert_exit(&run, 7, "a binary that misreports its version is rejected");
    assert_mentions(&run, "agentenv 0.0.1", "the expected version is named");
    assert!(install.binary_is_original(), "the binary is untouched");
}

#[test]
fn update_rejects_a_checksum_mismatch() {
    let server = Server::start();
    server.publish(
        &format!("v{NEXT}"),
        None,
        Some(&"0".repeat(64)),
        false,
        None,
    );
    let install = Install::new(&["bin"]);

    let run = install.run(&server, &["update"]);
    assert_exit(&run, 7, "a bad checksum fails the update");
    assert_mentions(&run, "checksum", "the failure names the check");
    assert!(install.binary_is_original(), "the binary is untouched");
}

#[test]
fn update_rejects_a_missing_companion_without_replacing_main() {
    let server = Server::start();
    server.publish(
        &format!("v{NEXT}"),
        None,
        None,
        false,
        Some("agentenv-ssh-askpass"),
    );
    let install = Install::new(&["bin"]);

    let run = install.run(&server, &["update"]);
    assert_exit(&run, 7, "an incomplete bundle is rejected");
    assert_mentions(&run, "complete release bundle", "the repair is named");
    assert!(install.binary_is_original(), "the main binary is untouched");
}

#[test]
fn update_rejects_a_mismatched_companion_without_replacing_main() {
    let server = Server::start();
    server.publish(&format!("v{NEXT}"), None, None, false, None);
    let install = Install::new(&["bin"]);

    let run = install.run(&server, &["update"]);
    assert_exit(&run, 7, "a mismatched companion is rejected");
    assert_mentions(&run, "mismatched identity", "the repair is named");
    assert!(install.binary_is_original(), "the main binary is untouched");
}

#[test]
fn update_refuses_a_cargo_managed_binary() {
    let server = Server::start();
    server.publish(&format!("v{NEXT}"), None, None, false, None);
    let install = Install::new(&[".cargo", "bin"]);

    let run = install.run(&server, &["update"]);
    assert_exit(&run, 7, "a cargo-managed binary is refused");
    assert_mentions(&run, "cargo install", "the refusal names the cargo path");
    assert!(install.binary_is_original(), "the binary is untouched");
}

mod standalone_assets {
    use super::*;
    use agentenv::sudo::deploy::{Destination, HelperSource, Source};
    use agentenv::update::{AssetLookup, Client, DownloadFailure};

    #[test]
    fn named_asset_is_found_verified_and_read_back() {
        let server = Server::start();
        let name = "agentenv-sudo-helper-v1.2.3-aarch64-unknown-linux-gnu";
        server.publish_asset("v1.2.3", "agentenv-v1.2.3-other.tar.gz", b"other", None);
        server.publish_asset("v1.2.3", name, b"helper-bytes", None);
        let client = Client::new(&server.base_url);
        let asset = client.fetch_asset("v1.2.3", name).expect("asset listed");
        assert_eq!(asset.name, name);
        assert_eq!(asset.digest.len(), 64);
        let dir = TempDir::new().expect("download dir");
        let path = client
            .download_asset("v1.2.3", &asset, dir.path(), 1024)
            .expect("download verifies");
        assert_eq!(fs::read(path).expect("downloaded bytes"), b"helper-bytes");
    }

    #[test]
    fn missing_asset_and_corrupt_digest_are_distinct_failures() {
        let server = Server::start();
        let name = "agentenv-sudo-helper-v1.2.3-x86_64-unknown-linux-gnu";
        server.publish_asset("v1.2.3", name, b"helper-bytes", Some(&"0".repeat(64)));
        let client = Client::new(&server.base_url);
        match client.fetch_asset(
            "v1.2.3",
            "agentenv-sudo-helper-v1.2.3-riscv64gc-unknown-linux-gnu",
        ) {
            Err(AssetLookup::NoAsset(detail)) => {
                assert!(detail.contains("ships no asset named"), "{detail}")
            }
            other => panic!("expected NoAsset, got {other:?}"),
        }
        match client.fetch_asset("v9.9.9", name) {
            Err(AssetLookup::NoRelease(detail)) => assert!(detail.contains("v9.9.9"), "{detail}"),
            other => panic!("expected NoRelease, got {other:?}"),
        }
        let asset = client
            .fetch_asset("v1.2.3", name)
            .expect("listed with a bad digest");
        let dir = TempDir::new().expect("download dir");
        match client.download_asset("v1.2.3", &asset, dir.path(), 1024) {
            Err(DownloadFailure::Checksum { name: failed, .. }) => assert_eq!(failed, name),
            other => panic!("expected a checksum failure, got {other:?}"),
        }
        let malformed = format!("zz  {name}-malformed\n");
        let sums = server.root.path().join("download/v1.2.3/SHA256SUMS");
        let mut text = fs::read_to_string(&sums).unwrap();
        text.push_str(&malformed);
        fs::write(&sums, text).unwrap();
        match client.fetch_asset("v1.2.3", &format!("{name}-malformed")) {
            Err(AssetLookup::Malformed(detail)) => {
                assert!(detail.contains("malformed digest"), "{detail}")
            }
            other => panic!("expected Malformed, got {other:?}"),
        }
        server.publish_asset("v1.2.3", name, b"helper-bytes", None);
        match client.fetch_asset("v1.2.3", name) {
            Err(AssetLookup::Malformed(detail)) => {
                assert!(detail.contains("more than once"), "{detail}")
            }
            other => panic!("expected a duplicate refusal, got {other:?}"),
        }
    }

    fn linux_arm() -> Destination {
        Destination {
            platform: "linux",
            machine: "aarch64".into(),
            target: "aarch64-unknown-linux-gnu",
            glibc: Some("2.36".into()),
        }
    }

    fn automatic(server: &Server) -> Source {
        Source::Automatic {
            bundle: server.root.path().join("no-bundle"),
            release_base_url: server.base_url.clone(),
        }
    }

    #[tokio::test]
    async fn automatic_source_downloads_and_verifies_the_helper_for_this_version() {
        let server = Server::start();
        let tag = format!("v{CURRENT}");
        let name = format!("agentenv-sudo-helper-{tag}-aarch64-unknown-linux-gnu");
        // The real release layout: the archive entry first, then the
        // standalone helpers, all in one SHA256SUMS.
        server.publish(&tag, None, None, true, None);
        server.publish_asset(
            &tag,
            &format!("agentenv-sudo-helper-{tag}-x86_64-unknown-linux-gnu"),
            b"x86",
            None,
        );
        server.publish_asset(&tag, &name, b"aarch64-helper", None);
        let bytes = automatic(&server)
            .bytes(&linux_arm())
            .await
            .expect("release source");
        assert_eq!(bytes.kind, "release");
        assert_eq!(bytes.name, name);
        assert_eq!(bytes.bytes, b"aarch64-helper");
        // The archive lookup used by `agentenv update` still parses that file.
        Client::new(&server.base_url)
            .fetch(Some(&tag))
            .expect("archive still resolves");
    }

    #[tokio::test]
    async fn automatic_source_reports_unreleased_versions_and_checksum_mismatches() {
        let server = Server::start();
        let tag = format!("v{CURRENT}");
        let name = format!("agentenv-sudo-helper-{tag}-aarch64-unknown-linux-gnu");
        let error = automatic(&server)
            .bytes(&linux_arm())
            .await
            .expect_err("nothing published");
        let text = error.to_string();
        assert!(text.contains("helper-deploy-source-unavailable"), "{text}");
        assert!(
            text.contains("is not published") && text.contains("--from"),
            "{text}"
        );
        assert!(!text.contains("check the release tag"), "{text}");
        server.publish(&tag, None, None, true, None);
        let error = automatic(&server)
            .bytes(&linux_arm())
            .await
            .expect_err("no helper asset");
        let text = error.to_string();
        assert!(
            text.contains("ships no helper for aarch64-unknown-linux-gnu")
                && text.contains("--from"),
            "{text}"
        );
        server.publish_asset(&tag, &name, b"aarch64-helper", Some(&"f".repeat(64)));
        let error = automatic(&server)
            .bytes(&linux_arm())
            .await
            .expect_err("bad digest");
        assert!(
            error
                .to_string()
                .contains("helper-deploy-checksum-mismatch"),
            "{error}"
        );
    }
}
