//! Traditional Keychain APIs missing from the Security.framework wrappers.

use std::ffi::CString;
use std::os::unix::ffi::OsStringExt;
use std::path::PathBuf;
use std::ptr;

use core_foundation::array::{CFArray, CFArrayRef};
use core_foundation::base::{CFType, CFTypeRef, TCFType};
use core_foundation::boolean::CFBoolean;
use core_foundation::data::CFData;
use core_foundation::dictionary::CFDictionary;
use core_foundation::string::{CFString, CFStringRef};
use security_framework::certificate::SecCertificate;
use security_framework::identity::SecIdentity;
use security_framework::item::{
    AddRef, ItemAddOptions, ItemAddValue, ItemClass, ItemSearchOptions, Limit, Location, Reference,
    SearchResult,
};
use security_framework::key::SecKey;
use security_framework::os::macos::access::SecAccess;
use security_framework::os::macos::code_signing::{SecRequirement, SecStaticCode};
use security_framework::os::macos::identity::SecIdentityExt;
use security_framework::os::macos::keychain::SecKeychain;
use security_framework_sys::base::{
    SecAccessRef, SecKeyRef, SecKeychainAttribute, SecKeychainAttributeList, SecKeychainRef,
};
use security_framework_sys::code_signing::{SecRequirementRef, SecStaticCodeRef};
use security_framework_sys::item::*;
use security_framework_sys::keychain_item::{
    SecItemCopyMatching, SecKeychainItemModifyAttributesAndData,
};

use super::{SigningError, KEY_LABEL, KEY_TAG};

extern "C" {
    static kSecAttrIsSensitive: CFStringRef;
    static kSecAttrIsExtractable: CFStringRef;
    static kSecAttrApplicationTag: CFStringRef;
    static kSecACLAuthorizationSign: CFStringRef;
    fn SecTrustedApplicationCreateFromPath(
        path: *const libc::c_char,
        application: *mut CFTypeRef,
    ) -> i32;
    fn SecAccessCreate(
        description: CFStringRef,
        applications: CFArrayRef,
        access: *mut SecAccessRef,
    ) -> i32;
    fn SecAccessCopyMatchingACLList(access: SecAccessRef, authorization: CFTypeRef) -> CFArrayRef;
    fn SecACLUpdateAuthorizations(acl: CFTypeRef, authorizations: CFArrayRef) -> i32;
    fn SecKeychainGetStatus(keychain: SecKeychainRef, status: *mut u32) -> i32;
    fn SecKeychainGetPath(
        keychain: SecKeychainRef,
        length: *mut u32,
        path: *mut libc::c_char,
    ) -> i32;
    fn SecCodeCopyDesignatedRequirement(
        code: SecStaticCodeRef,
        flags: u32,
        requirement: *mut SecRequirementRef,
    ) -> i32;
    fn SecKeyCreatePair(
        keychain: SecKeychainRef,
        algorithm: u32,
        bits: u32,
        context: u64,
        public_usage: u32,
        public_attributes: u32,
        private_usage: u32,
        private_attributes: u32,
        access: SecAccessRef,
        public_key: *mut SecKeyRef,
        private_key: *mut SecKeyRef,
    ) -> i32;
}

fn status(operation: &str, code: i32) -> Result<(), SigningError> {
    if code == 0 {
        return Ok(());
    }
    let detail = security_framework::base::Error::from_code(code);
    Err(SigningError(format!("{operation}: {detail} (OSStatus {code}). Unlock the selected Keychain and retry if authorization is required")))
}

fn constant(value: CFStringRef) -> CFType {
    // Security.framework constants remain alive for the process lifetime.
    unsafe { CFString::wrap_under_get_rule(value) }.as_CFType()
}

pub(super) fn require_unlocked(keychain: &SecKeychain) -> Result<(), SigningError> {
    let mut flags = 0;
    status("Could not inspect the selected Keychain", unsafe {
        SecKeychainGetStatus(keychain.as_concrete_TypeRef(), &mut flags)
    })?;
    if flags & 1 == 0 {
        return Err(SigningError(
            "The selected Keychain is locked; unlock it with macOS and retry local signing".into(),
        ));
    }
    Ok(())
}

pub(super) fn keychain_path(keychain: &SecKeychain) -> Result<PathBuf, SigningError> {
    let mut bytes = vec![0_u8; libc::PATH_MAX as usize + 1];
    let mut length = bytes.len() as u32;
    status("Could not locate the selected Keychain", unsafe {
        SecKeychainGetPath(
            keychain.as_concrete_TypeRef(),
            &mut length,
            bytes.as_mut_ptr().cast(),
        )
    })?;
    bytes.truncate(length as usize);
    if bytes.last() == Some(&0) {
        bytes.pop();
    }
    Ok(std::ffi::OsString::from_vec(bytes).into())
}

pub(super) fn find_key(keychain: &SecKeychain) -> Result<Option<SecKey>, SigningError> {
    let list = CFArray::from_CFTypes(std::slice::from_ref(keychain));
    let query = unsafe {
        CFDictionary::from_CFType_pairs(&[
            (constant(kSecClass), constant(kSecClassKey)),
            (
                constant(kSecAttrKeyClass),
                constant(kSecAttrKeyClassPrivate),
            ),
            (
                constant(kSecAttrApplicationTag),
                CFData::from_buffer(KEY_TAG.as_bytes()).as_CFType(),
            ),
            (constant(kSecMatchSearchList), list.as_CFType()),
            (constant(kSecMatchLimit), constant(kSecMatchLimitAll)),
            (constant(kSecReturnRef), CFBoolean::true_value().as_CFType()),
        ])
    };
    let mut result = ptr::null();
    let code = unsafe { SecItemCopyMatching(query.as_concrete_TypeRef(), &mut result) };
    if code == -25300 {
        return Ok(None);
    }
    status("Could not find the local signing key", code)?;
    // kSecMatchLimitAll with kSecReturnRef returns an array of key references.
    let keys: CFArray<CFType> = unsafe { CFArray::wrap_under_create_rule(result.cast()) };
    if keys.len() != 1 {
        return Err(SigningError("Multiple local signing private keys exist in the selected Keychain; restore the unique enrolled identity before continuing".into()));
    }
    let key = keys
        .get(0)
        .and_then(|value| value.downcast::<SecKey>())
        .ok_or_else(|| {
            SigningError("The selected Keychain returned an invalid signing-key reference".into())
        })?;
    validate_key_attributes(&key)?;
    Ok(Some(key))
}

pub(super) fn generate_key(keychain: &SecKeychain) -> Result<SecKey, SigningError> {
    let access = signing_access()?;
    // SecKeyCreateRandomKey ignores kSecAttrIsSensitive for traditional Keychains.
    // This public legacy API sets both immutable key attributes at generation.
    const RSA: u32 = 42;
    const RETURN_REF: u32 = 0x2000_0000;
    const PERMANENT: u32 = 1;
    const SENSITIVE: u32 = 8;
    const EXTRACTABLE: u32 = 0x20;
    const SIGN: u32 = 4;
    const VERIFY: u32 = 8;
    let mut public = ptr::null_mut();
    let mut private = ptr::null_mut();
    status(
        "Could not generate the nonextractable local signing key",
        unsafe {
            SecKeyCreatePair(
                keychain.as_concrete_TypeRef(),
                RSA,
                3072,
                0,
                VERIFY,
                RETURN_REF | EXTRACTABLE,
                SIGN,
                RETURN_REF | PERMANENT | SENSITIVE,
                access.as_concrete_TypeRef(),
                &mut public,
                &mut private,
            )
        },
    )?;
    let _public = unsafe { SecKey::wrap_under_create_rule(public) };
    let key = unsafe { SecKey::wrap_under_create_rule(private) };
    // SecItemUpdate conflates application tags and labels on file-based keys.
    // These public Keychain attribute tags update the intended fields directly.
    let mut attributes = [
        SecKeychainAttribute {
            tag: 1,
            length: KEY_LABEL.len() as u32,
            data: KEY_LABEL.as_ptr().cast_mut().cast(),
        },
        SecKeychainAttribute {
            tag: 7,
            length: KEY_TAG.len() as u32,
            data: KEY_TAG.as_ptr().cast_mut().cast(),
        },
    ];
    let list = SecKeychainAttributeList {
        count: attributes.len() as u32,
        attr: attributes.as_mut_ptr(),
    };
    let labeled = status("Could not label the local signing key", unsafe {
        SecKeychainItemModifyAttributesAndData(
            key.as_concrete_TypeRef().cast(),
            &list,
            0,
            ptr::null(),
        )
    })
    .and_then(|()| validate_key_attributes(&key));
    if let Err(error) = labeled {
        return match key.delete() {
            Ok(()) => Err(error),
            Err(cleanup) => Err(SigningError(format!(
                "{error}; the unissued signing key could not be removed: {cleanup}"
            ))),
        };
    }
    Ok(key)
}

pub(super) fn validate_key_attributes(key: &SecKey) -> Result<(), SigningError> {
    let attributes = key.attributes();
    let boolean = |attribute: CFStringRef| -> Option<bool> {
        let value = attributes.find(attribute.cast())?;
        let value = unsafe { CFType::wrap_under_get_rule(*value) };
        value.downcast::<CFBoolean>().map(bool::from)
    };
    let valid = unsafe {
        boolean(kSecAttrIsExtractable) == Some(false)
            && boolean(kSecAttrIsSensitive) == Some(true)
            && boolean(kSecAttrCanSign) == Some(true)
            && boolean(kSecAttrCanDecrypt) == Some(false)
            && boolean(kSecAttrCanDerive) == Some(false)
            && boolean(kSecAttrCanUnwrap) == Some(false)
    };
    if !valid {
        return Err(SigningError(format!("The local signing key does not have the required nonextractable, sensitive, signing-only attributes (extractable: {:?}, sensitive: {:?}, sign: {:?}, decrypt: {:?}, derive: {:?}, unwrap: {:?}); inspect the selected Keychain before continuing", unsafe { boolean(kSecAttrIsExtractable) }, unsafe { boolean(kSecAttrIsSensitive) }, unsafe { boolean(kSecAttrCanSign) }, unsafe { boolean(kSecAttrCanDecrypt) }, unsafe { boolean(kSecAttrCanDerive) }, unsafe { boolean(kSecAttrCanUnwrap) })));
    }
    Ok(())
}

fn signing_access() -> Result<SecAccess, SigningError> {
    let mut applications = Vec::new();
    // A null path names the running bootstrap, not every application.
    for path in [None, Some("/usr/bin/codesign")] {
        let path = path.map(|value| CString::new(value).expect("fixed path has no NUL"));
        let mut application = ptr::null();
        status("Could not identify a trusted signing application", unsafe {
            SecTrustedApplicationCreateFromPath(
                path.as_ref().map_or(ptr::null(), |value| value.as_ptr()),
                &mut application,
            )
        })?;
        applications.push(unsafe { CFType::wrap_under_create_rule(application) });
    }
    let applications = CFArray::from_CFTypes(&applications);
    let description = CFString::new(KEY_LABEL);
    let mut access = ptr::null_mut();
    status("Could not create signing-key access controls", unsafe {
        SecAccessCreate(
            description.as_concrete_TypeRef(),
            applications.as_concrete_TypeRef(),
            &mut access,
        )
    })?;
    let access = unsafe { SecAccess::wrap_under_create_rule(access) };
    // Limit the application's normal crypto-operation ACL to signing; retain
    // the system-created ACL administration and owner authorization entries.
    let matching = unsafe {
        SecAccessCopyMatchingACLList(
            access.as_concrete_TypeRef(),
            kSecACLAuthorizationSign.cast(),
        )
    };
    if matching.is_null() {
        return Err(SigningError(
            "macOS did not create a signing authorization for the local key".into(),
        ));
    }
    let matching: CFArray<CFType> = unsafe { CFArray::wrap_under_create_rule(matching) };
    if matching.is_empty() {
        return Err(SigningError(
            "macOS created an empty signing authorization list".into(),
        ));
    }
    let authorizations = CFArray::from_CFTypes(&[unsafe {
        CFString::wrap_under_get_rule(kSecACLAuthorizationSign)
    }]);
    for acl in matching.iter() {
        status("Could not restrict signing-key authorization", unsafe {
            SecACLUpdateAuthorizations(acl.as_CFTypeRef(), authorizations.as_concrete_TypeRef())
        })?;
    }
    Ok(access)
}

pub(super) fn public_bytes(public_key: &SecKey) -> Result<Vec<u8>, SigningError> {
    public_key
        .external_representation()
        .map(|data| data.to_vec())
        .ok_or_else(|| SigningError("Could not encode the signing public key".into()))
}

pub(super) fn certificate_present(
    keychain: &SecKeychain,
    expected: &SecCertificate,
) -> Result<bool, SigningError> {
    let matches = ItemSearchOptions::new()
        .keychains(std::slice::from_ref(keychain))
        .class(ItemClass::certificate())
        .label(KEY_LABEL)
        .load_refs(true)
        .limit(Limit::All)
        .search();
    let matches = match matches {
        Ok(matches) => matches,
        Err(error) if error.code() == -25300 => return Ok(false),
        Err(error) => {
            return Err(SigningError::context(
                "Could not inspect the local signing certificate",
                error,
            ))
        }
    };
    if matches.len() != 1 {
        return Err(SigningError("Multiple local signing certificates exist in the selected Keychain; restore the unique enrolled certificate".into()));
    }
    match &matches[0] {
        SearchResult::Ref(Reference::Certificate(certificate)) if certificate.to_der() == expected.to_der() => Ok(true),
        _ => Err(SigningError("The Keychain's local signing certificate differs from the saved certificate; restore the matching Keychain and signing.toml".into())),
    }
}

pub(super) fn add_certificate(
    keychain: &SecKeychain,
    certificate: &SecCertificate,
) -> Result<(), SigningError> {
    ItemAddOptions::new(ItemAddValue::Ref(AddRef::Certificate(certificate.clone())))
        .set_location(Location::FileKeychain(keychain.clone()))
        .set_label(KEY_LABEL)
        .add()
        .map_err(|error| {
            SigningError::context(
                "Could not restore the local signing certificate to the selected Keychain",
                error,
            )
        })
}

pub(super) fn resolve_identity(
    keychain: &SecKeychain,
    certificate: &SecCertificate,
) -> Result<(), SigningError> {
    SecIdentity::with_certificate(std::slice::from_ref(keychain), certificate)
        .map(|_| ())
        .map_err(|error| {
            SigningError::context(
                "Could not pair the local certificate with its private key",
                error,
            )
        })
}

pub(super) fn verify_designated_requirement(
    code: &SecStaticCode,
    expected: &SecRequirement,
) -> Result<(), SigningError> {
    let mut requirement = ptr::null_mut();
    status(
        "Could not inspect the executable's designated requirement",
        unsafe {
            SecCodeCopyDesignatedRequirement(code.as_concrete_TypeRef(), 0, &mut requirement)
        },
    )?;
    let requirement = unsafe { SecRequirement::wrap_under_create_rule(requirement) };
    if requirement != *expected {
        return Err(SigningError("The executable's designated requirement does not pin the expected local certificate and identifier".into()));
    }
    Ok(())
}
