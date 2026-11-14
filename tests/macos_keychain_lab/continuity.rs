//! Explicit login-Keychain proof; ordinary tests use disposable Keychains.

use std::ffi::c_void;
use std::os::unix::fs::PermissionsExt;
use std::process::{Command, Stdio};
use std::ptr;

use core_foundation::array::{CFArray, CFArrayRef};
use core_foundation::base::{CFType, TCFType};
use core_foundation::string::{CFString, CFStringRef};
use security_framework::os::macos::keychain::SecKeychain;
use security_framework_sys::keychain::SecKeychainFindGenericPassword;

use super::*;

extern "C" {
    fn SecKeychainItemCopyAccess(item: *mut c_void, access: *mut *mut c_void) -> i32;
    fn SecAccessCopyACLList(access: *mut c_void, acls: *mut CFArrayRef) -> i32;
    fn SecACLCopyContents(
        acl: *const c_void,
        applications: *mut CFArrayRef,
        description: *mut CFStringRef,
        selector: *mut u16,
    ) -> i32;
}

fn login_command(install: &Install, server: &Server, config: &Path, args: &[&str]) -> Command {
    let mut command = install.command(server, args);
    command
        .env_remove("AGENTENV_TEST_NATIVE_KEYCHAIN")
        .env("HOME", std::env::var_os("HOME").expect("real login home"))
        .env("XDG_DATA_HOME", install.home.join(".local/share"))
        .env("AGENTENV_FILE", config);
    command
}

fn login_run(install: &Install, server: &Server, config: &Path, args: &[&str]) -> Run {
    Run::from_output(
        login_command(install, server, config, args)
            .output()
            .unwrap(),
        args,
    )
}

fn helper_root(install: &Install) -> PathBuf {
    install.home.join(".local/share/agentenv/macos-keychain")
}

fn restore(path: &Path, bytes: &[u8], mode: u32) {
    let staged = path.with_extension("fixture-restore");
    fs::write(&staged, bytes).unwrap();
    fs::set_permissions(&staged, fs::Permissions::from_mode(mode)).unwrap();
    fs::rename(staged, path).unwrap();
}

struct LoginItem<'a> {
    install: &'a Install,
    server: &'a Server,
    config: PathBuf,
    service: String,
    helper: Vec<u8>,
    receipt: Vec<u8>,
}

impl Drop for LoginItem<'_> {
    fn drop(&mut self) {
        // A negative helper-upgrade control must never prevent cleanup by the
        // exact executable that originally owned the synthetic login item.
        restore(
            &helper_root(self.install).join("agentenv-keychain-helper"),
            &self.helper,
            0o700,
        );
        restore(
            &helper_root(self.install).join("helper.toml"),
            &self.receipt,
            0o600,
        );
        let run = login_run(
            self.install,
            self.server,
            &self.config,
            &[
                "keychain",
                "delete-fixture",
                "--service",
                &self.service,
                "--account",
                "fixture",
            ],
        );
        if run.code != Some(0) {
            eprintln!(
                "Synthetic login-Keychain cleanup failed for {}: {}",
                self.service, run.stderr
            );
            if !std::thread::panicking() {
                assert_exit(&run, 0, "the synthetic login-Keychain item is removed");
            }
        }
    }
}

fn partitioned_item(keychain: &SecKeychain, service: &str) -> bool {
    unsafe {
        let mut item = ptr::null_mut();
        assert_eq!(
            SecKeychainFindGenericPassword(
                keychain.as_CFTypeRef(),
                service.len() as u32,
                service.as_ptr().cast(),
                7,
                c"fixture".as_ptr(),
                ptr::null_mut(),
                ptr::null_mut(),
                &mut item,
            ),
            0,
            "synthetic item metadata is readable without secret data"
        );
        let item = CFType::wrap_under_create_rule(item.cast());
        let mut access = ptr::null_mut();
        assert_eq!(
            SecKeychainItemCopyAccess(item.as_CFTypeRef().cast_mut(), &mut access),
            0
        );
        let access = CFType::wrap_under_create_rule(access.cast());
        let mut acls = ptr::null();
        assert_eq!(
            SecAccessCopyACLList(access.as_CFTypeRef().cast_mut(), &mut acls),
            0
        );
        let acls: CFArray<CFType> = CFArray::wrap_under_create_rule(acls);
        for acl in acls.iter() {
            let mut applications = ptr::null();
            let mut description = ptr::null();
            let mut selector = 0;
            assert_eq!(
                SecACLCopyContents(
                    acl.as_CFTypeRef(),
                    &mut applications,
                    &mut description,
                    &mut selector
                ),
                0
            );
            if !applications.is_null() {
                drop(CFArray::<CFType>::wrap_under_create_rule(applications));
            }
            if description.is_null() {
                continue;
            }
            let text = CFString::wrap_under_create_rule(description).to_string();
            if text.len() % 2 != 0 || !text.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                continue;
            }
            let decoded = (0..text.len())
                .step_by(2)
                .map(|index| u8::from_str_radix(&text[index..index + 2], 16).unwrap())
                .collect::<Vec<_>>();
            let decoded = String::from_utf8_lossy(&decoded);
            if decoded.contains("<key>Partitions</key>") && decoded.contains("cdhash:") {
                return true;
            }
        }
        false
    }
}

fn revise_executable(path: &Path, rpath: &str) {
    let edited = Command::new("/usr/bin/install_name_tool")
        .args(["-add_rpath", rpath])
        .arg(path)
        .output()
        .unwrap();
    assert!(
        edited.status.success(),
        "install_name_tool: {}",
        String::from_utf8_lossy(&edited.stderr)
    );
    let sealed = Command::new("/usr/bin/codesign")
        .args(["--force", "--sign", "-"])
        .arg(path)
        .output()
        .unwrap();
    assert!(
        sealed.status.success(),
        "codesign: {}",
        String::from_utf8_lossy(&sealed.stderr)
    );
}

fn publish_rebuilt_main(server: &Server, tag: &str) {
    let stem = format!("agentenv-{tag}-{TARGET}");
    let staging = server.root.path().join("staging").join(&stem);
    revise_executable(&staging.join(BINARY_NAME), "/agentenv-test-rebuilt-main");
    let name = format!("{stem}.{ARCHIVE_EXTENSION}");
    let archive = server.root.path().join("download").join(tag).join(&name);
    pack(&staging, &stem, &archive);
    let digest: String = Sha256::digest(fs::read(&archive).unwrap())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let sums = format!("{digest}  {name}\n");
    fs::write(
        server
            .root
            .path()
            .join("download")
            .join(tag)
            .join("SHA256SUMS"),
        &sums,
    )
    .unwrap();
    fs::write(server.root.path().join("latest/download/SHA256SUMS"), sums).unwrap();
}

fn run_installer(install: &Install, server: &Server, tag: &str) -> Run {
    let shim = install._dir.path().join("download-tools");
    fs::create_dir(&shim).unwrap();
    let curl = shim.join("curl");
    let source = r#"#!/bin/bash
set -euo pipefail
args=()
for arg in "$@"; do
    case "$arg" in
        https://github.com/ii999/agentenv/releases/download/*)
            args+=("__BASE__/download/${arg#https://github.com/ii999/agentenv/releases/download/}") ;;
        *) args+=("$arg") ;;
    esac
done
exec /usr/bin/curl "${args[@]}"
"#.replace("__BASE__", &server.base_url);
    fs::write(&curl, source).unwrap();
    fs::set_permissions(&curl, fs::Permissions::from_mode(0o700)).unwrap();
    fs::remove_file(&install.binary).unwrap();
    let output = Command::new("/bin/bash")
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("install.sh"))
        .args(["--no-skill", "--version", tag, "--dir"])
        .arg(install.binary.parent().unwrap())
        .env_clear()
        .current_dir(install._dir.path())
        .env("HOME", &install.home)
        .env(
            "PATH",
            format!("{}:/usr/bin:/bin:/usr/sbin:/sbin", shim.display()),
        )
        .env("AGENTENV_TEST_NATIVE_KEYCHAIN", &install.keychain_path)
        .env("AGENTENV_TEST_KEYCHAIN_NO_UI", "1")
        .output()
        .unwrap();
    Run::from_output(output, &["install.sh"])
}

#[test]
#[ignore = "uses one synthetic item in an unlocked partitioned login Keychain; run explicitly on macOS"]
fn installer_and_update_preserve_login_keychain_authorization() {
    let _no_ui = SecKeychain::disable_user_interaction().unwrap();
    let login = SecKeychain::default().unwrap();
    let server = Server::start();
    let tag = format!("v{CURRENT}");
    server.publish(&tag, None, None, true, None);
    let install = Install::new(&["bin"]);
    assert_exit(
        &run_installer(&install, &server, &tag),
        0,
        "the real macOS installer signs and installs the local bundle",
    );
    let service = format!(
        "agentenv-test-update-{}",
        install._dir.path().file_name().unwrap().to_string_lossy()
    );
    let config = install._dir.path().join("login-fixture.toml");
    fs::write(
        &config,
        format!(
            r#"version = 1
[credentials.fixture]
description = "Synthetic login-Keychain update fixture."
provider = "keychain"
service = "{service}"
account = "fixture"
inject_as = "AGENTENV_SYNTHETIC_TOKEN"
"#
        ),
    )
    .unwrap();
    fs::set_permissions(&config, fs::Permissions::from_mode(0o600)).unwrap();
    let item = LoginItem {
        install: &install,
        server: &server,
        config,
        service,
        helper: fs::read(helper_root(&install).join("agentenv-keychain-helper")).unwrap(),
        receipt: fs::read(helper_root(&install).join("helper.toml")).unwrap(),
    };
    let args = ["credential", "set", "fixture"];
    let mut command = login_command(&install, &server, &item.config, &args);
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"synthetic-login-update-value\n")
        .unwrap();
    assert_exit(
        &Run::from_output(child.wait_with_output().unwrap(), &args),
        0,
        "the installed helper creates a synthetic login item",
    );
    assert_exit(
        &login_run(
            &install,
            &server,
            &item.config,
            &["credential", "check", "fixture"],
        ),
        0,
        "initial read establishes the helper's partition authorization",
    );
    assert!(
        partitioned_item(&login, &item.service),
        "the fixture must exercise modern login-Keychain cdhash partitioning"
    );
    assert!(
        login
            .find_generic_password(&item.service, "fixture")
            .is_err(),
        "an unrelated executable is refused without authorization UI"
    );
    let main_before = fs::read(&install.binary).unwrap();
    publish_rebuilt_main(&server, &tag);
    assert_exit(
        &install.run(&server, &["update", "--force", "--no-skill"]),
        0,
        "the ordinary update path installs a content-different rebuild",
    );
    assert_ne!(
        Sha256::digest(&main_before),
        Sha256::digest(fs::read(&install.binary).unwrap()),
        "the installed main executable changes"
    );
    assert_eq!(
        Sha256::digest(&item.helper),
        Sha256::digest(fs::read(helper_root(&install).join("agentenv-keychain-helper")).unwrap()),
        "ordinary update preserves the exact signed helper"
    );
    assert_eq!(
        item.receipt,
        fs::read(helper_root(&install).join("helper.toml")).unwrap(),
        "ordinary update preserves the helper's receipt"
    );
    assert_exit(
        &login_run(
            &install,
            &server,
            &item.config,
            &["credential", "check", "fixture"],
        ),
        0,
        "updated CLI reads the existing login credential with UI disabled",
    );

    let candidate = install._dir.path().join("changed-keychain-helper");
    fs::copy(cargo_bin("agentenv-keychain-helper"), &candidate).unwrap();
    revise_executable(&candidate, "/agentenv-test-changed-helper");
    let mut replacement = install.command(
        &server,
        &["keychain", "setup", "--replace-helper", "--from"],
    );
    replacement.arg(&candidate);
    assert_exit(
        &Run::from_output(replacement.output().unwrap(), &["keychain", "setup"]),
        0,
        "explicit helper replacement uses the same local identity",
    );
    let changed = login_run(
        &install,
        &server,
        &item.config,
        &["credential", "check", "fixture"],
    );
    assert_exit(
        &changed,
        4,
        "changing the helper itself correctly requires new authorization",
    );
    assert_mentions(
        &changed,
        "OSStatus -25293",
        "the helper-upgrade boundary remains explicit",
    );
    drop(item);
}
