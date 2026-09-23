use std::fmt;

use thiserror::Error;
use zeroize::Zeroizing;

/// Bytes captured from an external credential source before validation.
///
/// ```compile_fail
/// use agentenv::credential::CapturedSecret;
///
/// fn requires_display(_: impl std::fmt::Display) {}
/// fn assert_not_display(value: CapturedSecret) {
///     requires_display(value);
/// }
/// ```
///
/// ```compile_fail
/// use agentenv::credential::CapturedSecret;
///
/// fn requires_serialize(_: impl serde::Serialize) {}
/// fn assert_not_serializable(value: CapturedSecret) {
///     requires_serialize(value);
/// }
/// ```
pub struct CapturedSecret(Zeroizing<Vec<u8>>);

/// A validated credential value. Its contents cannot be formatted or
/// serialized by callers.
///
/// ```compile_fail
/// use agentenv::credential::Secret;
///
/// fn requires_display(_: impl std::fmt::Display) {}
/// fn assert_not_display(value: Secret) {
///     requires_display(value);
/// }
/// ```
///
/// ```compile_fail
/// use agentenv::credential::Secret;
///
/// fn requires_serialize(_: impl serde::Serialize) {}
/// fn assert_not_serializable(value: Secret) {
///     requires_serialize(value);
/// }
/// ```
pub struct Secret(Zeroizing<String>);

/// A validation failure that intentionally carries no candidate bytes.
#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum SecretDomainError {
    #[error("the value is empty")]
    Empty,
    #[error("the value contains a NUL byte")]
    ContainsNul,
    #[error("the value is not valid UTF-8")]
    InvalidUtf8,
    #[error("the value is not supported for authentication")]
    InvalidAuthentication,
}

impl CapturedSecret {
    pub fn new(bytes: Vec<u8>) -> Self {
        Self(Zeroizing::new(bytes))
    }

    /// Removes one conventional line ending from a line-oriented capture.
    pub fn strip_one_trailing_newline(mut self) -> Self {
        let length = self.0.len();
        if self.0.ends_with(b"\r\n") {
            self.0.truncate(length - 2);
        } else if self.0.ends_with(b"\n") {
            self.0.truncate(length - 1);
        }
        self
    }

    /// Validates the captured bytes without changing them.
    pub fn into_secret(self) -> Result<Secret, SecretDomainError> {
        if self.0.contains(&0) {
            return Err(SecretDomainError::ContainsNul);
        }
        let value = std::str::from_utf8(&self.0).map_err(|_| SecretDomainError::InvalidUtf8)?;
        if value.is_empty() {
            return Err(SecretDomainError::Empty);
        }
        Ok(Secret(Zeroizing::new(value.to_owned())))
    }
}

impl Secret {
    /// Checks the full UTF-8 value at each authentication response boundary.
    /// The caller supplies its independently verified transport byte limit.
    pub fn validate_authentication(&self, limit: usize) -> Result<(), SecretDomainError> {
        if limit == 0
            || self.0.len() > limit.min(255)
            || self
                .0
                .as_bytes()
                .iter()
                .any(|byte| matches!(byte, 0 | b'\r' | b'\n'))
        {
            return Err(SecretDomainError::InvalidAuthentication);
        }
        Ok(())
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }

    #[cfg(all(feature = "test-keychain", debug_assertions))]
    pub(crate) fn as_bytes(&self) -> &[u8] {
        self.0.as_bytes()
    }
}

impl fmt::Debug for CapturedSecret {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("CapturedSecret(<redacted>)")
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Secret(<redacted>)")
    }
}

#[cfg(test)]
mod tests {
    use super::{CapturedSecret, SecretDomainError};

    #[test]
    fn captured_secret_redacts_without_normalizing_values() {
        let secret = CapturedSecret::new(b"hunter2\r\n".to_vec())
            .into_secret()
            .expect("a normal value is accepted");
        assert_eq!(secret.as_str(), "hunter2\r\n");
        assert_eq!(format!("{secret:?}"), "Secret(<redacted>)");
        assert_eq!(
            format!("{:?}", CapturedSecret::new(b"candidate".to_vec())),
            "CapturedSecret(<redacted>)"
        );
    }

    #[test]
    fn line_oriented_capture_removes_exactly_one_trailing_newline() {
        let secret = CapturedSecret::new(b"hunter2\r\n".to_vec())
            .strip_one_trailing_newline()
            .into_secret()
            .expect("a normal value is accepted");
        assert_eq!(secret.as_str(), "hunter2");
    }

    #[test]
    fn captured_secret_rejects_invalid_values_without_retaining_them() {
        assert!(matches!(
            CapturedSecret::new(Vec::new()).into_secret(),
            Err(SecretDomainError::Empty)
        ));
        assert!(matches!(
            CapturedSecret::new(b"a\0b".to_vec()).into_secret(),
            Err(SecretDomainError::ContainsNul)
        ));
        assert!(matches!(
            CapturedSecret::new(vec![0xff]).into_secret(),
            Err(SecretDomainError::InvalidUtf8)
        ));
    }
}
