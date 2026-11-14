//! One request and one response on a private inherited local socket.

use std::io::{self, Read, Write};

use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use crate::credential::{CapturedSecret, KeychainFailure, Secret};

use super::native::{Diagnostic, UnlockReport};

pub const VERSION: u32 = 1;
pub const IDENTITY: &str = "agentenv-keychain-helper 1";
const MAGIC: &[u8; 5] = b"AEKC\x01";
const REPLY_LIMIT: usize = 64 * 1024;

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "kebab-case", deny_unknown_fields)]
pub(crate) enum Request {
    Read {
        service: String,
        account: String,
    },
    Store {
        service: String,
        account: String,
    },
    Status,
    Unlock,
    #[cfg(all(feature = "test-support", debug_assertions))]
    DeleteFixture {
        service: String,
        account: String,
    },
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "kebab-case", deny_unknown_fields)]
pub(crate) enum Reply {
    Read,
    Stored,
    Inspected {
        state: Diagnostic,
    },
    Unlocked {
        report: UnlockReport,
    },
    Failed {
        error: KeychainFailure,
    },
    #[cfg(all(feature = "test-support", debug_assertions))]
    FixtureDeleted,
}

pub(crate) struct RequestFrame {
    pub metadata: Request,
    pub secret: Option<Secret>,
}

pub(crate) struct ReplyFrame {
    pub metadata: Reply,
    pub secret: Option<Secret>,
}

fn invalid() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "invalid credential helper message",
    )
}

fn write_bytes(output: &mut impl Write, bytes: &[u8]) -> io::Result<()> {
    let length = u32::try_from(bytes.len()).map_err(|_| invalid())?;
    output.write_all(&length.to_be_bytes())?;
    output.write_all(bytes)
}

fn read_bytes(input: &mut impl Read, limit: usize) -> io::Result<Zeroizing<Vec<u8>>> {
    let mut length = [0; 4];
    input.read_exact(&mut length)?;
    let length = u32::from_be_bytes(length) as usize;
    if length > limit {
        return Err(invalid());
    }
    let mut bytes = Zeroizing::new(Vec::new());
    bytes.try_reserve_exact(length).map_err(|_| {
        io::Error::new(
            io::ErrorKind::OutOfMemory,
            "credential message allocation failed",
        )
    })?;
    bytes.resize(length, 0);
    input.read_exact(&mut bytes)?;
    Ok(bytes)
}

fn check_magic(input: &mut impl Read) -> io::Result<()> {
    let mut magic = [0; 5];
    input.read_exact(&mut magic)?;
    if &magic != MAGIC {
        return Err(invalid());
    }
    Ok(())
}

pub(crate) fn write_request(
    output: &mut impl Write,
    metadata: &Request,
    secret: Option<&Secret>,
) -> io::Result<()> {
    if matches!(metadata, Request::Store { .. }) != secret.is_some() {
        return Err(invalid());
    }
    let encoded = serde_json::to_vec(metadata).map_err(|_| invalid())?;
    output.write_all(MAGIC)?;
    write_bytes(output, &encoded)?;
    write_bytes(
        output,
        secret.map_or(&[], |value| value.as_str().as_bytes()),
    )?;
    output.flush()
}

pub(crate) fn read_request(input: &mut impl Read) -> io::Result<RequestFrame> {
    check_magic(input)?;
    // Keychain's string and data lengths are UInt32. The private same-user
    // transport preserves that boundary instead of narrowing supported values.
    let encoded = read_bytes(input, u32::MAX as usize)?;
    let metadata: Request = serde_json::from_slice(&encoded).map_err(|_| invalid())?;
    let bytes = read_bytes(input, u32::MAX as usize)?;
    let secret = if matches!(metadata, Request::Store { .. }) {
        Some(
            CapturedSecret::new(bytes.to_vec())
                .into_secret()
                .map_err(|_| invalid())?,
        )
    } else if bytes.is_empty() {
        None
    } else {
        return Err(invalid());
    };
    Ok(RequestFrame { metadata, secret })
}

pub(crate) fn write_reply(output: &mut impl Write, frame: &ReplyFrame) -> io::Result<()> {
    if matches!(frame.metadata, Reply::Read) != frame.secret.is_some() {
        return Err(invalid());
    }
    let encoded = serde_json::to_vec(&frame.metadata).map_err(|_| invalid())?;
    output.write_all(MAGIC)?;
    write_bytes(output, &encoded)?;
    write_bytes(
        output,
        frame
            .secret
            .as_ref()
            .map_or(&[], |value| value.as_str().as_bytes()),
    )?;
    output.flush()
}

pub(crate) fn read_reply(input: &mut impl Read) -> io::Result<ReplyFrame> {
    check_magic(input)?;
    let encoded = read_bytes(input, REPLY_LIMIT)?;
    let metadata: Reply = serde_json::from_slice(&encoded).map_err(|_| invalid())?;
    let bytes = read_bytes(input, u32::MAX as usize)?;
    let secret = if matches!(metadata, Reply::Read) {
        Some(
            CapturedSecret::new(bytes.to_vec())
                .into_secret()
                .map_err(|_| invalid())?,
        )
    } else if bytes.is_empty() {
        None
    } else {
        return Err(invalid());
    };
    Ok(ReplyFrame { metadata, secret })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn private_channel_preserves_long_unicode_and_multiline_credentials() {
        let value = format!("  密钥-🙂\r\n{}\n", "x".repeat(20_000));
        let secret = CapturedSecret::new(value.as_bytes().to_vec())
            .into_secret()
            .unwrap();
        let mut request = Vec::new();
        write_request(
            &mut request,
            &Request::Store {
                service: "service".into(),
                account: "account".into(),
            },
            Some(&secret),
        )
        .unwrap();
        let received = read_request(&mut request.as_slice()).unwrap();
        assert_eq!(received.secret.unwrap().as_str(), value);
        let mut response = Vec::new();
        write_reply(
            &mut response,
            &ReplyFrame {
                metadata: Reply::Read,
                secret: Some(secret),
            },
        )
        .unwrap();
        assert_eq!(
            read_reply(&mut response.as_slice())
                .unwrap()
                .secret
                .unwrap()
                .as_str(),
            value
        );
    }

    #[test]
    fn metadata_only_responses_refuse_secret_payloads() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(MAGIC);
        write_bytes(&mut bytes, br#"{"status":"stored"}"#).unwrap();
        write_bytes(&mut bytes, b"synthetic-unexpected-data").unwrap();
        assert!(read_reply(&mut bytes.as_slice()).is_err());
    }
}
