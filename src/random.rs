//! Operating-system randomness for session identifiers and request ids.

use std::io;

/// Fills `bytes` from the operating system's cryptographic random source.
pub(crate) fn fill(bytes: &mut [u8]) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::io::Read;
        std::fs::File::open("/dev/urandom")?.read_exact(bytes)
    }
    #[cfg(windows)]
    {
        crate::windows::random(bytes)
    }
}

/// A 24-byte random value encoded as 48 lowercase hexadecimal characters.
pub(crate) fn nonce() -> io::Result<String> {
    let mut bytes = [0_u8; 24];
    fill(&mut bytes)?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}
