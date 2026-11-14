//! Disposable Keychains registered for the system code-signing identity search.

use std::fs::{File, OpenOptions};
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::process::Command;
use std::ptr;

use core_foundation::array::{CFArray, CFArrayRef};
use core_foundation::base::TCFType;
use security_framework::os::macos::keychain::{CreateOptions, SecKeychain, SecPreferencesDomain};
use security_framework_sys::base::SecKeychainRef;

extern "C" {
    fn SecKeychainCopyDomainSearchList(domain: SecPreferencesDomain, list: *mut CFArrayRef) -> i32;
    fn SecKeychainSetDomainSearchList(domain: SecPreferencesDomain, list: CFArrayRef) -> i32;
    fn SecKeychainDelete(keychain: SecKeychainRef) -> i32;
}

fn registry_lock() -> File {
    // Separate integration-test executables share the user's Keychain search
    // list. A file lock also serializes their native creation and deletion.
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(std::env::temp_dir().join("agentenv-test-keychain-list.lock"))
        .expect("open the fixture Keychain registry lock");
    file.lock().expect("lock the fixture Keychain registry");
    file
}

fn search_list() -> Vec<SecKeychain> {
    let mut raw = ptr::null();
    assert_eq!(
        unsafe { SecKeychainCopyDomainSearchList(SecPreferencesDomain::User, &mut raw) },
        0,
        "read the user Keychain search list"
    );
    let list: CFArray<SecKeychain> = unsafe { CFArray::wrap_under_create_rule(raw) };
    list.iter()
        .map(|keychain| SecKeychain::clone(&keychain))
        .collect()
}

fn set_search_list(keychains: &[SecKeychain]) -> i32 {
    let list = CFArray::from_CFTypes(keychains);
    unsafe {
        SecKeychainSetDomainSearchList(SecPreferencesDomain::User, list.as_concrete_TypeRef())
    }
}

pub fn create(path: &Path, password: &str, isolated_home: Option<&Path>) -> SecKeychain {
    let lock = registry_lock();
    let keychain = CreateOptions::new()
        .password(password)
        .create(path)
        .expect("create the disposable Keychain");
    // codesign requires the identity's Keychain on the user's search list,
    // including when --keychain explicitly selects that same Keychain.
    let mut keychains = search_list();
    if !keychains.contains(&keychain) {
        keychains.push(keychain.clone());
        let status = set_search_list(&keychains);
        if status != 0 {
            let cleanup = unsafe { SecKeychainDelete(keychain.as_concrete_TypeRef()) };
            panic!("register the disposable Keychain: OSStatus {status}; cleanup: {cleanup}");
        }
    }
    drop(lock);
    if let Some(home) = isolated_home {
        // A CLI's isolated HOME also isolates Security.framework preferences.
        // This newly created fixture home has no unrelated search-list entries.
        let registered = std::fs::create_dir_all(home.join("Library/Preferences")).and_then(|()| {
            Command::new("/usr/bin/security")
                .env("HOME", home)
                .args(["list-keychains", "-d", "user", "-s"])
                .arg(path)
                .output()
        });
        match registered {
            Ok(output) if output.status.success() => {}
            other => {
                delete(&keychain);
                panic!("register the Keychain in its isolated home: {other:?}");
            }
        }
    }
    keychain
}

pub fn delete(keychain: &SecKeychain) {
    let _lock = registry_lock();
    // Re-read the list so another live fixture or newly registered user
    // Keychain survives cleanup; never restore an earlier list snapshot.
    let mut keychains = search_list();
    keychains.retain(|entry| entry != keychain);
    let registration = set_search_list(&keychains);
    let deletion = unsafe { SecKeychainDelete(keychain.as_concrete_TypeRef()) };
    if registration != 0 || deletion != 0 {
        let message = format!(
            "remove the disposable Keychain: search-list OSStatus {registration}; deletion OSStatus {deletion}"
        );
        if std::thread::panicking() {
            eprintln!("{message}");
        } else {
            panic!("{message}");
        }
    }
}
