//! Native Keychain operations executed by the persistent credential helper.

use std::path::PathBuf;
use std::ptr;

use core_foundation::base::TCFType;
use security_framework::base::Error;
use security_framework::os::macos::keychain::{SecKeychain, SecPreferencesDomain};
use security_framework::os::macos::keychain_item::SecKeychainItem;
use security_framework_sys::base::SecKeychainRef;
use security_framework_sys::keychain::{SecKeychainFindGenericPassword, SecKeychainSettings};
use serde::{Deserialize, Serialize};

use crate::credential::{
    CapturedSecret, KeychainFailure, KeychainOperation, KeychainReason, Secret,
};

const UNLOCKED: u32 = 1;
const GRAPHICAL: u32 = 0x10;
const REMOTE: u32 = 0x1000;
const ERR_USER_CANCELLED: i32 = -128;
const ERR_AUTH_FAILED: i32 = -25293;
const ERR_NO_ENTRY: i32 = -25300;
const ERR_INTERACTION_NOT_ALLOWED: i32 = -25308;
const ERR_INTERACTION_REQUIRED: i32 = -25315;

extern "C" {
    fn SessionGetInfo(session: u32, actual: *mut u32, attributes: *mut u32) -> i32;
    fn SecKeychainGetStatus(keychain: SecKeychainRef, status: *mut u32) -> i32;
    fn SecKeychainCopySettings(keychain: SecKeychainRef, settings: *mut SecKeychainSettings)
        -> i32;
    fn SecKeychainGetPath(
        keychain: SecKeychainRef,
        length: *mut u32,
        path: *mut libc::c_char,
    ) -> i32;
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionStatus {
    pub graphical: bool,
    pub remote: bool,
    pub interaction_allowed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Diagnostic {
    pub keychain: PathBuf,
    pub unlocked: bool,
    pub lock_settings: Option<LockSettings>,
    pub lock_settings_error: Option<i32>,
    pub session: SessionStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LockSettings {
    pub locks_on_sleep: bool,
    pub lock_after_seconds: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UnlockReport {
    pub was_locked: bool,
    pub state: Diagnostic,
}

pub(crate) fn open_keychain() -> Result<SecKeychain, KeychainFailure> {
    #[cfg(all(feature = "test-support", debug_assertions))]
    {
        if std::env::var_os("AGENTENV_TEST_KEYCHAIN_NO_UI").is_some() {
            // Deliberately retain the process-wide setting for the helper's
            // lifetime, including any implicit prompt inside a Security API.
            unsafe {
                let status =
                    security_framework_sys::keychain::SecKeychainSetUserInteractionAllowed(0);
                if status != 0 {
                    return Err(bare_failure(status, KeychainOperation::Status));
                }
            }
        }
        if let Some(path) = std::env::var_os("AGENTENV_TEST_NATIVE_KEYCHAIN") {
            return SecKeychain::open(path)
                .map_err(|error| bare_failure(error.code(), KeychainOperation::Status));
        }
    }
    SecKeychain::default_for_domain(SecPreferencesDomain::User)
        .map_err(|error| bare_failure(error.code(), KeychainOperation::Status))
}

pub(crate) fn session_status() -> Result<SessionStatus, KeychainFailure> {
    let mut attributes = 0;
    let status = unsafe { SessionGetInfo(u32::MAX, ptr::null_mut(), &mut attributes) };
    if status != 0 {
        return Err(bare_failure(status, KeychainOperation::Status));
    }
    let interaction_allowed = SecKeychain::user_interaction_allowed()
        .map_err(|error| bare_failure(error.code(), KeychainOperation::Status))?;
    Ok(SessionStatus {
        graphical: attributes & GRAPHICAL != 0,
        remote: attributes & REMOTE != 0,
        interaction_allowed,
    })
}

fn unlocked(keychain: &SecKeychain) -> Result<bool, Error> {
    let mut flags = 0;
    let status = unsafe { SecKeychainGetStatus(keychain.as_concrete_TypeRef(), &mut flags) };
    if status == 0 {
        Ok(flags & UNLOCKED != 0)
    } else {
        Err(Error::from_code(status))
    }
}

pub(crate) fn inspect(keychain: &SecKeychain) -> Result<Diagnostic, KeychainFailure> {
    let session = session_status()?;
    let _no_ui = if session.interaction_allowed {
        Some(
            SecKeychain::disable_user_interaction()
                .map_err(|error| classify(keychain, error.code(), KeychainOperation::Status))?,
        )
    } else {
        None
    };
    let is_unlocked = unlocked(keychain)
        .map_err(|error| classify(keychain, error.code(), KeychainOperation::Status))?;
    let mut settings = SecKeychainSettings {
        version: 1,
        lockOnSleep: 0,
        useLockInterval: 0,
        lockInterval: 0,
    };
    // CopySettings itself can require authentication on a locked Keychain.
    // Its absence must not hide an already observed lock state or trigger UI.
    let settings_status = is_unlocked
        .then(|| unsafe { SecKeychainCopySettings(keychain.as_concrete_TypeRef(), &mut settings) });
    let mut path = vec![0_u8; libc::PATH_MAX as usize + 1];
    let mut length = path.len() as u32;
    let status = unsafe {
        SecKeychainGetPath(
            keychain.as_concrete_TypeRef(),
            &mut length,
            path.as_mut_ptr().cast(),
        )
    };
    if status != 0 {
        return Err(classify(keychain, status, KeychainOperation::Status));
    }
    path.truncate(length as usize);
    use std::os::unix::ffi::OsStringExt;
    Ok(Diagnostic {
        keychain: std::ffi::OsString::from_vec(path).into(),
        unlocked: is_unlocked,
        lock_settings: (settings_status == Some(0)).then_some(LockSettings {
            locks_on_sleep: settings.lockOnSleep != 0,
            lock_after_seconds: (settings.useLockInterval != 0).then_some(settings.lockInterval),
        }),
        lock_settings_error: settings_status.filter(|status| *status != 0),
        session,
    })
}

pub(crate) fn unlock(keychain: &mut SecKeychain) -> Result<UnlockReport, KeychainFailure> {
    let was_locked = !unlocked(keychain)
        .map_err(|error| classify(keychain, error.code(), KeychainOperation::Unlock))?;
    if was_locked {
        unlock_for_operation(keychain, KeychainOperation::Unlock)?;
    }
    Ok(UnlockReport {
        was_locked,
        state: inspect(keychain)?,
    })
}

fn unlock_for_operation(
    keychain: &mut SecKeychain,
    operation: KeychainOperation,
) -> Result<(), KeychainFailure> {
    let session = session_status()?;
    if !session.graphical || !session.interaction_allowed {
        let mut error = classify(keychain, ERR_INTERACTION_NOT_ALLOWED, operation);
        error.reason = KeychainReason::InteractionUnavailable;
        return Err(error);
    }
    // The OS owns the password window. No login password enters this process.
    keychain.unlock(None).map_err(|error| {
        let mut failure = classify(keychain, error.code(), KeychainOperation::Unlock);
        if failure.reason == KeychainReason::Unavailable {
            failure.reason = KeychainReason::UnlockFailed;
        }
        failure
    })?;
    if !unlocked(keychain).map_err(|error| classify(keychain, error.code(), operation))? {
        let mut error = classify(keychain, ERR_AUTH_FAILED, operation);
        error.reason = KeychainReason::Locked;
        return Err(error);
    }
    Ok(())
}

/// Authenticated native operations may race a system lock after the initial
/// observation. Unlock at most once, then resume only an authentication-failed
/// operation. An unlocked Keychain's ACL denial is not a lock condition.
fn with_unlock<T>(
    keychain: &mut SecKeychain,
    operation: KeychainOperation,
    mut perform: impl FnMut(&SecKeychain) -> Result<T, Error>,
) -> Result<T, KeychainFailure> {
    let initially_locked =
        !unlocked(keychain).map_err(|error| classify(keychain, error.code(), operation))?;
    if initially_locked {
        unlock_for_operation(keychain, operation)?;
    }
    match perform(keychain) {
        Ok(value) => Ok(value),
        Err(error)
            if !initially_locked
                && matches!(
                    error.code(),
                    ERR_AUTH_FAILED | ERR_INTERACTION_REQUIRED | ERR_INTERACTION_NOT_ALLOWED
                )
                && matches!(unlocked(keychain), Ok(false)) =>
        {
            unlock_for_operation(keychain, operation)?;
            perform(keychain).map_err(|error| classify(keychain, error.code(), operation))
        }
        Err(error) => Err(classify(keychain, error.code(), operation)),
    }
}

pub(crate) fn read(
    keychain: &mut SecKeychain,
    service: &str,
    account: &str,
) -> Result<Secret, KeychainFailure> {
    validate_selectors(service, account, KeychainOperation::Read)?;
    let bytes = with_unlock(keychain, KeychainOperation::Read, |keychain| {
        let (password, _) = keychain.find_generic_password(service, account)?;
        Ok(CapturedSecret::new(password.to_owned()))
    })?;
    bytes
        .into_secret()
        .map_err(|_| KeychainFailure::new(KeychainReason::InvalidValue, KeychainOperation::Read))
}

pub(crate) fn store(
    keychain: &mut SecKeychain,
    service: &str,
    account: &str,
    secret: &Secret,
) -> Result<(), KeychainFailure> {
    validate_selectors(service, account, KeychainOperation::Store)?;
    if u32::try_from(secret.as_str().len()).is_err() {
        return Err(KeychainFailure::new(
            KeychainReason::InvalidValue,
            KeychainOperation::Store,
        ));
    }
    with_unlock(keychain, KeychainOperation::Store, |keychain| {
        // Query the item reference without reading its old secret. Only an
        // actual missing item permits creation; other failures remain failures.
        let mut raw_item = ptr::null_mut();
        let status = unsafe {
            SecKeychainFindGenericPassword(
                keychain.as_CFTypeRef(),
                service.len() as u32,
                service.as_ptr().cast(),
                account.len() as u32,
                account.as_ptr().cast(),
                ptr::null_mut(),
                ptr::null_mut(),
                &mut raw_item,
            )
        };
        match status {
            0 => {
                let mut item = unsafe { SecKeychainItem::wrap_under_create_rule(raw_item) };
                item.set_password(secret.as_str().as_bytes())
            }
            ERR_NO_ENTRY => {
                keychain.add_generic_password(service, account, secret.as_str().as_bytes())
            }
            other => Err(Error::from_code(other)),
        }
    })
}

fn validate_selectors(
    service: &str,
    account: &str,
    operation: KeychainOperation,
) -> Result<(), KeychainFailure> {
    if [service, account]
        .iter()
        .any(|value| value.is_empty() || u32::try_from(value.len()).is_err())
    {
        return Err(KeychainFailure::new(
            KeychainReason::InvalidValue,
            operation,
        ));
    }
    Ok(())
}

#[cfg(all(feature = "test-support", debug_assertions))]
pub(crate) fn delete_fixture(
    keychain: &mut SecKeychain,
    service: &str,
    account: &str,
) -> Result<(), KeychainFailure> {
    if !service.starts_with("agentenv-test-") {
        return Err(KeychainFailure::new(
            KeychainReason::AccessDenied,
            KeychainOperation::Store,
        ));
    }
    with_unlock(keychain, KeychainOperation::Store, |keychain| {
        let (_, item) = keychain.find_generic_password(service, account)?;
        let code = unsafe {
            security_framework_sys::keychain_item::SecKeychainItemDelete(item.as_concrete_TypeRef())
        };
        if code == 0 {
            Ok(())
        } else {
            Err(Error::from_code(code))
        }
    })
}

fn bare_failure(code: i32, operation: KeychainOperation) -> KeychainFailure {
    let reason = match code {
        -25307 | -25294 => KeychainReason::KeychainNotFound,
        ERR_NO_ENTRY => KeychainReason::Missing,
        ERR_USER_CANCELLED => KeychainReason::Cancelled,
        ERR_AUTH_FAILED => KeychainReason::AccessDenied,
        ERR_INTERACTION_REQUIRED | ERR_INTERACTION_NOT_ALLOWED => {
            KeychainReason::InteractionUnavailable
        }
        _ => KeychainReason::Unavailable,
    };
    KeychainFailure {
        os_status: Some(code),
        ..KeychainFailure::new(reason, operation)
    }
}

fn classify(keychain: &SecKeychain, code: i32, operation: KeychainOperation) -> KeychainFailure {
    let mut failure = bare_failure(code, operation);
    failure.locked = unlocked(keychain).ok().map(|value| !value);
    if let Ok(session) = session_status() {
        failure.graphical_session = Some(session.graphical);
        failure.interaction_allowed = Some(session.interaction_allowed);
    }
    failure
}
