#![cfg(target_os = "macos")]

#[path = "helpers/macos_keychain.rs"]
mod keychain_fixture;

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;

use agentenv::macos::signing;
use core_foundation::base::TCFType;
use security_framework::item::{ItemClass, ItemSearchOptions, Limit, Reference, SearchResult};
use security_framework::key::{Algorithm, SecKey};
use security_framework::os::macos::keychain::SecKeychain;
use security_framework_sys::base::SecKeychainRef;

const IDENTIFIER: &str = "io.github.ii999.agentenv.signing-test";
const LABEL: &str = "agentenv local code signing";

extern "C" {
    fn SecKeychainLock(keychain: SecKeychainRef) -> i32;
}

struct ScratchKeychain {
    keychain: SecKeychain,
    _directory: tempfile::TempDir,
}

impl Drop for ScratchKeychain {
    fn drop(&mut self) {
        keychain_fixture::delete(&self.keychain);
    }
}

fn keys(keychain: &SecKeychain) -> Vec<SecKey> {
    ItemSearchOptions::new()
        .keychains(std::slice::from_ref(keychain))
        .class(ItemClass::key())
        .label(LABEL)
        .load_refs(true)
        .limit(Limit::All)
        .search()
        .unwrap()
        .into_iter()
        .filter_map(|item| match item {
            SearchResult::Ref(Reference::Key(key)) => Some(key),
            _ => None,
        })
        .collect()
}

#[test]
fn enroll_reuse_recover_and_reject_identity_loss() {
    let _interaction = SecKeychain::disable_user_interaction().unwrap();
    let directory = tempfile::tempdir().unwrap();
    let keychain_path = directory.path().join("signing.keychain");
    let fixture = ScratchKeychain {
        keychain: keychain_fixture::create(&keychain_path, "disposable-test-only", None),
        _directory: directory,
    };
    let state = fixture._directory.path().join("state");
    assert!(signing::load(&state, &fixture.keychain).unwrap().is_none());
    let identity = signing::ensure(&state, &fixture.keychain).unwrap();
    assert_eq!(
        fs::metadata(state.join("signing.toml"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    assert_eq!(keys(&fixture.keychain).len(), 1);
    let private_key = keys(&fixture.keychain).pop().unwrap();
    // The native API must refuse private export; no returned data is printed or saved.
    assert!(private_key.external_representation().is_none());

    let first = fixture._directory.path().join("first");
    fs::copy("/usr/bin/true", &first).unwrap();
    identity.sign(&first, IDENTIFIER).unwrap();
    identity.verify(&first, IDENTIFIER).unwrap();
    assert!(identity.verify(&first, "another.identifier").is_err());
    assert!(identity
        .sign(&std::env::current_exe().unwrap(), IDENTIFIER)
        .is_err());

    let reused = signing::ensure(&state, &fixture.keychain).unwrap();
    assert_eq!(identity.certificate_der(), reused.certificate_der());
    let second = fixture._directory.path().join("second");
    fs::copy("/usr/bin/false", &second).unwrap();
    let bootstrap = fixture._directory.path().join("different-bootstrap");
    fs::copy(std::env::current_exe().unwrap(), &bootstrap).unwrap();
    let status = Command::new("/usr/bin/codesign")
        .args([
            "--force",
            "--sign",
            "-",
            "--identifier",
            "agentenv.test.second-bootstrap",
        ])
        .arg(&bootstrap)
        .status()
        .unwrap();
    assert!(status.success());
    let result = Command::new(&bootstrap)
        .args([
            "--exact",
            "reuse_from_an_independent_bootstrap",
            "--nocapture",
        ])
        .env("AGENTENV_SIGNING_TEST_KEYCHAIN", &keychain_path)
        .env("AGENTENV_SIGNING_TEST_STATE", &state)
        .env("AGENTENV_SIGNING_TEST_EXECUTABLE", &second)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    identity.verify(&second, IDENTIFIER).unwrap();

    ItemSearchOptions::new()
        .keychains(std::slice::from_ref(&fixture.keychain))
        .class(ItemClass::certificate())
        .label(LABEL)
        .delete()
        .unwrap();
    assert!(signing::load(&state, &fixture.keychain).is_err());
    let restored = signing::ensure(&state, &fixture.keychain).unwrap();
    assert_eq!(identity.certificate_der(), restored.certificate_der());
    assert!(signing::load(&state, &fixture.keychain).unwrap().is_some());

    // Recover an interruption after permanent key creation but before DER publication.
    let receipt_path = state.join("signing.toml");
    let complete_receipt = fs::read_to_string(&receipt_path).unwrap();
    let mut pending: toml::Table = toml::from_str(&complete_receipt).unwrap();
    pending.remove("certificate_der");
    pending.remove("certificate_sha1");
    fs::write(&receipt_path, toml::to_string(&pending).unwrap()).unwrap();
    let resumed = signing::ensure(&state, &fixture.keychain).unwrap();
    assert_eq!(identity.certificate_der(), resumed.certificate_der());

    let corrupt = fixture._directory.path().join("corrupt");
    fs::copy(&first, &corrupt).unwrap();
    let mut bytes = fs::read(&corrupt).unwrap();
    bytes[4096] ^= 1;
    fs::write(&corrupt, bytes).unwrap();
    assert!(identity.verify(&corrupt, IDENTIFIER).is_err());

    // A valid signature with a broad designated requirement is also rejected.
    let status = Command::new("/usr/bin/codesign")
        .args([
            "--force",
            "--sign",
            identity.certificate_sha1(),
            "--keychain",
        ])
        .arg(&keychain_path)
        .args([
            "--identifier",
            IDENTIFIER,
            "--timestamp=none",
            "--requirements",
            "=designated => always",
        ])
        .arg(&first)
        .status()
        .unwrap();
    assert!(status.success());
    assert!(identity.verify(&first, IDENTIFIER).is_err());

    let foreign_directory = tempfile::tempdir().unwrap();
    let foreign = ScratchKeychain {
        keychain: keychain_fixture::create(
            &foreign_directory.path().join("foreign.keychain"),
            "disposable-foreign-test-only",
            None,
        ),
        _directory: foreign_directory,
    };
    let foreign_identity =
        signing::ensure(&foreign._directory.path().join("state"), &foreign.keychain).unwrap();
    foreign_identity.sign(&first, IDENTIFIER).unwrap();
    assert!(identity.verify(&first, IDENTIFIER).is_err());
    fs::write(
        &receipt_path,
        fs::read(foreign._directory.path().join("state/signing.toml")).unwrap(),
    )
    .unwrap();
    let mismatch = signing::ensure(&state, &fixture.keychain)
        .err()
        .unwrap()
        .to_string();
    assert!(mismatch.contains("do not match"), "{mismatch}");
    fs::write(&receipt_path, &complete_receipt).unwrap();

    assert_eq!(
        unsafe { SecKeychainLock(fixture.keychain.as_concrete_TypeRef()) },
        0
    );
    signing::verify_signed(&second, IDENTIFIER, identity.certificate_sha1()).unwrap();
    let error = signing::ensure(&state, &fixture.keychain)
        .err()
        .unwrap()
        .to_string();
    assert!(error.contains("locked"), "{error}");
    let mut keychain = fixture.keychain.clone();
    keychain.unlock(Some("disposable-test-only")).unwrap();
    private_key.delete().unwrap();
    let error = signing::ensure(&state, &fixture.keychain)
        .err()
        .unwrap()
        .to_string();
    assert!(error.contains("private key is missing"), "{error}");
}

#[test]
fn reuse_from_an_independent_bootstrap() {
    let Some(path) = std::env::var_os("AGENTENV_SIGNING_TEST_KEYCHAIN") else {
        return;
    };
    let _interaction = SecKeychain::disable_user_interaction().unwrap();
    let keychain = SecKeychain::open(path).unwrap();
    let state = std::env::var_os("AGENTENV_SIGNING_TEST_STATE").unwrap();
    let executable = std::env::var_os("AGENTENV_SIGNING_TEST_EXECUTABLE").unwrap();
    let loaded = signing::load(Path::new(&state), &keychain)
        .unwrap()
        .unwrap();
    let identity = signing::ensure(Path::new(&state), &keychain).unwrap();
    assert_eq!(loaded.certificate_der(), identity.certificate_der());
    let private_key = keys(&keychain).pop().unwrap();
    assert!(private_key
        .create_signature(
            Algorithm::RSASignatureMessagePKCS1v15SHA256,
            b"untrusted bootstrap"
        )
        .is_err());
    identity.sign(Path::new(&executable), IDENTIFIER).unwrap();
}
