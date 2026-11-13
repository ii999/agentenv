//! A persistent local code-signing identity backed by a nonextractable Keychain key.
//!
//! Callers hold the installation lock across enrollment and staged-file signing.
//! The certificate pins a local installation identity; it does not confer Apple
//! developer status or bypass the Keychain's executable partition checks.

mod native;

use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use core_foundation::url::CFURL;
use rcgen::{CertificateParams, CustomExtension, DistinguishedName, DnType, IsCa, KeyUsagePurpose};
use security_framework::certificate::SecCertificate;
use security_framework::key::{Algorithm, SecKey};
use security_framework::os::macos::code_signing::{Flags, SecRequirement, SecStaticCode};
use security_framework::os::macos::keychain::SecKeychain;
use serde::{Deserialize, Serialize};
use sha1::{Digest, Sha1};

const KEY_TAG: &str = "io.github.ii999.agentenv.local-signing.v1";
const KEY_LABEL: &str = "agentenv local code signing";

/// A failed identity operation, with no private material in its diagnostic.
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct SigningError(String);

impl SigningError {
    fn context(operation: &str, error: impl std::fmt::Display) -> Self {
        Self(format!("{operation}: {error}"))
    }
}

/// Public certificate information and its explicitly selected Keychain.
pub struct LocalIdentity {
    certificate_sha1: String,
    certificate_der: Vec<u8>,
    keychain: SecKeychain,
    keychain_path: PathBuf,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Receipt {
    schema: u32,
    key_tag: String,
    serial: String,
    certificate_der: Option<String>,
    certificate_sha1: Option<String>,
}

/// Loads an enrolled identity without changing files or Keychain contents.
/// An incomplete enrollment needs `ensure` under the installation lock.
pub fn load(
    state_dir: &Path,
    keychain: &SecKeychain,
) -> Result<Option<LocalIdentity>, SigningError> {
    let Some(receipt) = read_receipt(state_dir)? else {
        return Ok(None);
    };
    native::require_unlocked(keychain)?;
    let key = native::find_key(keychain)?.ok_or_else(missing_key)?;
    let certificate = receipt_certificate(&receipt)?.ok_or_else(|| {
        SigningError(
            "Local signing enrollment is incomplete; run the installer again to finish it".into(),
        )
    })?;
    match_key(&key, &certificate)?;
    if !native::certificate_present(keychain, &certificate)? {
        return Err(SigningError("The local signing certificate is missing from the Keychain; run the installer again to restore the saved certificate".into()));
    }
    native::resolve_identity(keychain, &certificate)?;
    local_identity(keychain, certificate).map(Some)
}

/// Creates or recovers the one local identity in the selected Keychain.
/// The caller must serialize this operation with signing and installation.
pub fn ensure(state_dir: &Path, keychain: &SecKeychain) -> Result<LocalIdentity, SigningError> {
    native::require_unlocked(keychain)?;
    let mut key = native::find_key(keychain)?;
    let mut receipt = match read_receipt(state_dir)? {
        Some(receipt) => receipt,
        None => {
            if key.is_some() {
                return Err(SigningError("A local signing key exists without its recovery state; restore signing.toml from this installation before continuing".into()));
            }
            let mut serial = [0_u8; 20];
            crate::random::fill(&mut serial).map_err(|error| {
                SigningError::context("Could not generate a certificate serial", error)
            })?;
            serial[0] = (serial[0] & 0x7f) | 1;
            let receipt = Receipt {
                schema: 1,
                key_tag: KEY_TAG.into(),
                serial: hex(&serial),
                certificate_der: None,
                certificate_sha1: None,
            };
            fs::create_dir_all(state_dir).map_err(|error| {
                SigningError::context("Could not create signing state directory", error)
            })?;
            write_receipt(state_dir, &receipt)?;
            receipt
        }
    };
    let certificate = match receipt_certificate(&receipt)? {
        Some(certificate) => {
            match_key(key.as_ref().ok_or_else(missing_key)?, &certificate)?;
            certificate
        }
        None => {
            if key.is_none() {
                key = Some(native::generate_key(keychain)?);
            }
            let key = key.as_ref().ok_or_else(missing_key)?;
            let certificate = issue_certificate(key, &decode_hex(&receipt.serial)?)?;
            receipt.certificate_sha1 = Some(hex(&Sha1::digest(certificate.to_der())));
            receipt.certificate_der = Some(hex(&certificate.to_der()));
            // Publication always follows durable storage of the exact certificate.
            write_receipt(state_dir, &receipt)?;
            certificate
        }
    };
    if !native::certificate_present(keychain, &certificate)? {
        native::add_certificate(keychain, &certificate)?;
    }
    native::resolve_identity(keychain, &certificate)?;
    local_identity(keychain, certificate)
}

impl LocalIdentity {
    pub fn certificate_sha1(&self) -> &str {
        &self.certificate_sha1
    }

    pub fn certificate_der(&self) -> &[u8] {
        &self.certificate_der
    }

    /// Signs a staged executable, then verifies its contents and pinned identity.
    pub fn sign(&self, executable: &Path, identifier: &str) -> Result<(), SigningError> {
        let requirement = requirement(identifier, &self.certificate_sha1)?;
        native::require_unlocked(&self.keychain)?;
        let executable = fs::canonicalize(executable).map_err(|error| {
            SigningError::context("Could not locate the staged executable", error)
        })?;
        let metadata = fs::metadata(&executable).map_err(|error| {
            SigningError::context("Could not inspect the staged executable", error)
        })?;
        let current = std::env::current_exe()
            .and_then(fs::metadata)
            .map_err(|error| {
                SigningError::context("Could not identify the running executable", error)
            })?;
        if metadata.dev() == current.dev() && metadata.ino() == current.ino() {
            return Err(SigningError(
                "Cannot sign the running executable; sign a separate staged copy".into(),
            ));
        }
        if !metadata.is_file() {
            return Err(SigningError(
                "The staged executable must be a regular file".into(),
            ));
        }
        let result = Command::new("/usr/bin/codesign")
            .args(["--force", "--sign", &self.certificate_sha1, "--keychain"])
            .arg(&self.keychain_path)
            .args([
                "--identifier",
                identifier,
                "--timestamp=none",
                "--requirements",
            ])
            .arg(format!("=designated => {requirement}"))
            .arg(&executable)
            .output()
            .map_err(|error| SigningError::context("Could not launch macOS codesign", error))?;
        if !result.status.success() {
            return Err(SigningError(format!(
                "macOS could not sign the staged executable ({}): {}. Unlock the selected Keychain and allow codesign to use the agentenv signing key if macOS asks",
                result.status,
                String::from_utf8_lossy(&result.stderr).trim()
            )));
        }
        self.verify(&executable, identifier)
    }

    /// Validates every architecture's signed contents against the exact leaf certificate.
    pub fn verify(&self, executable: &Path, identifier: &str) -> Result<(), SigningError> {
        verify_signed(executable, identifier, &self.certificate_sha1)
    }
}

/// Verifies signed contents and the pinned designated requirement using public
/// metadata only. This remains available while the signing Keychain is locked.
pub fn verify_signed(
    executable: &Path,
    identifier: &str,
    certificate_sha1: &str,
) -> Result<(), SigningError> {
    if certificate_sha1.len() != 40
        || !certificate_sha1
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(SigningError(
            "The local signing certificate fingerprint must contain 40 hexadecimal digits".into(),
        ));
    }
    let requirement: SecRequirement =
        requirement(identifier, certificate_sha1)?
            .parse()
            .map_err(|error| {
                SigningError::context("Could not construct the signing requirement", error)
            })?;
    let path = CFURL::from_path(executable, false).ok_or_else(|| {
        SigningError("Could not represent the executable path for signature verification".into())
    })?;
    let code = SecStaticCode::from_path(&path, Flags::NONE).map_err(|error| {
        SigningError::context("Could not inspect the executable signature", error)
    })?;
    code.check_validity(
        Flags::CHECK_ALL_ARCHITECTURES | Flags::STRICT_VALIDATE,
        &requirement,
    )
    .map_err(|error| {
        SigningError::context(
            "The executable does not have a valid signature from this local identity",
            error,
        )
    })?;
    native::verify_designated_requirement(&code, &requirement)
}

fn requirement(identifier: &str, certificate_sha1: &str) -> Result<String, SigningError> {
    if identifier.is_empty()
        || !identifier
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b".-_".contains(&byte))
    {
        return Err(SigningError("A code-signing identifier must contain only ASCII letters, digits, dots, hyphens, and underscores".into()));
    }
    Ok(format!(
        "identifier \"{identifier}\" and certificate leaf = H\"{}\"",
        certificate_sha1
    ))
}

fn local_identity(
    keychain: &SecKeychain,
    certificate: SecCertificate,
) -> Result<LocalIdentity, SigningError> {
    let certificate_der = certificate.to_der();
    Ok(LocalIdentity {
        certificate_sha1: hex(&Sha1::digest(&certificate_der)),
        certificate_der,
        keychain: keychain.clone(),
        keychain_path: native::keychain_path(keychain)?,
    })
}

fn missing_key() -> SigningError {
    SigningError("The enrolled local signing private key is missing from the selected Keychain; restore that Keychain from backup. A replacement key would change this installation's identity".into())
}

fn read_receipt(state_dir: &Path) -> Result<Option<Receipt>, SigningError> {
    let text = match fs::read_to_string(state_dir.join("signing.toml")) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(SigningError::context(
                "Could not read local signing recovery state",
                error,
            ))
        }
    };
    // Parse diagnostics may echo input lines, so do not include their payload.
    let receipt: Receipt = toml::from_str(&text).map_err(|_| {
        SigningError(
            "Local signing recovery state is invalid; restore signing.toml from backup".into(),
        )
    })?;
    if receipt.schema != 1
        || receipt.key_tag != KEY_TAG
        || decode_hex(&receipt.serial)?.len() != 20
        || receipt.certificate_der.is_some() != receipt.certificate_sha1.is_some()
    {
        return Err(SigningError("Local signing recovery state is inconsistent or uses an unsupported schema; restore signing.toml from backup".into()));
    }
    Ok(Some(receipt))
}

fn write_receipt(state_dir: &Path, receipt: &Receipt) -> Result<(), SigningError> {
    let text = toml::to_string(receipt).map_err(|error| {
        SigningError::context("Could not encode local signing recovery state", error)
    })?;
    crate::fs_atomic::replace(
        &state_dir.join("signing.toml"),
        text.as_bytes(),
        Some(0o600),
    )
    .map_err(|error| {
        SigningError::context("Could not save local signing recovery state", error.error)
    })
}

fn receipt_certificate(receipt: &Receipt) -> Result<Option<SecCertificate>, SigningError> {
    let Some(der) = &receipt.certificate_der else {
        return Ok(None);
    };
    let der = decode_hex(der)?;
    if receipt.certificate_sha1.as_deref() != Some(hex(&Sha1::digest(&der)).as_str()) {
        return Err(SigningError("The saved local signing certificate does not match its fingerprint; restore signing.toml from backup".into()));
    }
    SecCertificate::from_der(&der).map(Some).map_err(|error| {
        SigningError::context("The saved local signing certificate is invalid", error)
    })
}

fn match_key(key: &SecKey, certificate: &SecCertificate) -> Result<(), SigningError> {
    let public_key = key
        .public_key()
        .ok_or_else(|| SigningError("Could not inspect the signing key's public key".into()))?;
    let certificate_key = certificate.public_key().map_err(|error| {
        SigningError::context("Could not inspect the certificate's public key", error)
    })?;
    if native::public_bytes(&public_key)? != native::public_bytes(&certificate_key)? {
        return Err(SigningError("The saved local signing certificate and Keychain private key do not match; restore the matching Keychain and signing.toml from backup".into()));
    }
    native::validate_key_attributes(key)
}

struct NativeSigner<'a> {
    key: &'a SecKey,
    public: Vec<u8>,
    failure: std::cell::RefCell<Option<String>>,
}

impl rcgen::PublicKeyData for NativeSigner<'_> {
    fn der_bytes(&self) -> &[u8] {
        &self.public
    }
    fn algorithm(&self) -> &'static rcgen::SignatureAlgorithm {
        &rcgen::PKCS_RSA_SHA256
    }
}

impl rcgen::SigningKey for NativeSigner<'_> {
    fn sign(&self, message: &[u8]) -> Result<Vec<u8>, rcgen::Error> {
        self.key
            .create_signature(Algorithm::RSASignatureMessagePKCS1v15SHA256, message)
            .map_err(|error| {
                *self.failure.borrow_mut() = Some(format!("{} (code {})", error, error.code()));
                rcgen::Error::RemoteKeyError
            })
    }
}

fn issue_certificate(key: &SecKey, serial: &[u8]) -> Result<SecCertificate, SigningError> {
    let public = key
        .public_key()
        .ok_or_else(|| SigningError("Could not obtain the signing public key".into()))?;
    let signer = NativeSigner {
        key,
        public: native::public_bytes(&public)?,
        failure: Default::default(),
    };
    let mut parameters = CertificateParams::default();
    parameters.distinguished_name = DistinguishedName::new();
    parameters
        .distinguished_name
        .push(DnType::CommonName, KEY_LABEL);
    parameters.serial_number = Some(serial.to_vec().into());
    // Fixed dates make an interrupted first enrollment reproducible with its saved serial.
    parameters.not_before = rcgen::date_time_ymd(2020, 1, 1);
    parameters.not_after = rcgen::date_time_ymd(2120, 1, 1);
    parameters.is_ca = IsCa::ExplicitNoCa;
    parameters.key_usages = vec![KeyUsagePurpose::DigitalSignature];
    let mut eku = CustomExtension::from_oid_content(
        &[2, 5, 29, 37],
        vec![
            0x30, 0x0a, 0x06, 0x08, 0x2b, 0x06, 0x01, 0x05, 0x05, 0x07, 0x03, 0x03,
        ],
    );
    eku.set_criticality(true);
    parameters.custom_extensions.push(eku);
    let certificate = parameters.self_signed(&signer).map_err(|error| {
        SigningError::context(
            "Could not self-sign the local certificate",
            signer
                .failure
                .borrow_mut()
                .take()
                .unwrap_or_else(|| error.to_string()),
        )
    })?;
    SecCertificate::from_der(certificate.der()).map_err(|error| {
        SigningError::context("Could not decode the local signing certificate", error)
    })
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn decode_hex(text: &str) -> Result<Vec<u8>, SigningError> {
    if !text.len().is_multiple_of(2) || !text.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(SigningError("Local signing recovery state contains invalid hexadecimal data; restore signing.toml from backup".into()));
    }
    text.as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| {
            let nibble = |byte: u8| {
                if byte.is_ascii_digit() {
                    byte - b'0'
                } else {
                    byte.to_ascii_lowercase() - b'a' + 10
                }
            };
            Ok(nibble(pair[0]) * 16 + nibble(pair[1]))
        })
        .collect()
}
