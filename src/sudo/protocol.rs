//! Bounded framing and flow-control primitives for remote sudo execution.

use std::fmt;
use std::io;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use zeroize::Zeroizing;

use crate::credential::{CapturedSecret, Secret};

pub const MAGIC: [u8; 4] = *b"AGEP";
pub const VERSION: u16 = 1;
pub const HEADER_LEN: usize = 20;
pub const MAX_PAYLOAD_SIZE: usize = 64 * 1024;
pub const MAX_STREAM_CHUNK_SIZE: usize = 32 * 1024;
pub const MAX_SECRET_SIZE: usize = 255;
pub const INITIAL_STREAM_CREDIT: usize = 128 * 1024;
pub const MAX_STREAM_CREDIT: usize = INITIAL_STREAM_CREDIT;
pub const MAX_QUEUED_DATA_PER_DIRECTION: usize = 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum MessageKind {
    Hello = 1,
    Ready = 2,
    Start = 3,
    PasswordRequest = 4,
    PasswordResponse = 5,
    PasswordUnavailable = 6,
    StdinChunk = 7,
    StdinEof = 8,
    StdoutChunk = 9,
    StdoutEof = 10,
    StderrChunk = 11,
    StderrEof = 12,
    WindowUpdate = 13,
    Cancel = 14,
    Result = 15,
    Failure = 16,
}

impl TryFrom<u16> for MessageKind {
    type Error = ProtocolError;

    fn try_from(value: u16) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(Self::Hello),
            2 => Ok(Self::Ready),
            3 => Ok(Self::Start),
            4 => Ok(Self::PasswordRequest),
            5 => Ok(Self::PasswordResponse),
            6 => Ok(Self::PasswordUnavailable),
            7 => Ok(Self::StdinChunk),
            8 => Ok(Self::StdinEof),
            9 => Ok(Self::StdoutChunk),
            10 => Ok(Self::StdoutEof),
            11 => Ok(Self::StderrChunk),
            12 => Ok(Self::StderrEof),
            13 => Ok(Self::WindowUpdate),
            14 => Ok(Self::Cancel),
            15 => Ok(Self::Result),
            16 => Ok(Self::Failure),
            other => Err(ProtocolError::UnknownMessageKind(other)),
        }
    }
}

impl MessageKind {
    fn is_metadata(self) -> bool {
        matches!(
            self,
            Self::Hello
                | Self::Ready
                | Self::Start
                | Self::PasswordRequest
                | Self::WindowUpdate
                | Self::Cancel
                | Self::Result
                | Self::Failure
        )
    }

    fn is_stream(self) -> bool {
        matches!(
            self,
            Self::StdinChunk | Self::StdoutChunk | Self::StderrChunk
        )
    }

    fn is_empty(self) -> bool {
        matches!(
            self,
            Self::PasswordUnavailable | Self::StdinEof | Self::StdoutEof | Self::StderrEof
        )
    }
}

#[derive(Error, Debug)]
pub enum ProtocolError {
    #[error("protocol I/O failed: {0}")]
    Io(#[from] io::Error),
    #[error("protocol frame ended before its declared boundary")]
    IncompleteFrame,
    #[error("invalid protocol magic")]
    InvalidMagic,
    #[error("unsupported protocol version {0}")]
    UnsupportedVersion(u16),
    #[error("unknown protocol message kind {0}")]
    UnknownMessageKind(u16),
    #[error("request identifier must be nonzero")]
    ZeroRequestId,
    #[error("protocol payload length {length} exceeds limit {limit}")]
    PayloadTooLarge { length: usize, limit: usize },
    #[error("message kind {kind:?} cannot use this payload encoding")]
    InvalidKindContext { kind: MessageKind },
    #[error("message kind {kind:?} requires an empty payload")]
    ExpectedEmptyPayload { kind: MessageKind },
    #[error("metadata payload is invalid: {0}")]
    InvalidMetadata(serde_json::Error),
    #[error("password payload is invalid")]
    InvalidSecret,
    #[error("stream credit exhausted: requested {requested} bytes with {available} available")]
    InsufficientCredit { requested: usize, available: usize },
    #[error("stream credit grant exceeds the {maximum}-byte maximum")]
    CreditOverflow { maximum: usize },
}

/// One validated frame. Payload storage is zeroized on drop for every kind so
/// password frames cannot accidentally be downgraded to ordinary ownership.
pub struct Frame {
    kind: MessageKind,
    request_id: u64,
    payload: Zeroizing<Vec<u8>>,
}

impl Frame {
    pub fn metadata<T: Serialize>(
        kind: MessageKind,
        request_id: u64,
        value: &T,
    ) -> Result<Self, ProtocolError> {
        validate_request_id(request_id)?;
        if !kind.is_metadata() {
            return Err(ProtocolError::InvalidKindContext { kind });
        }
        let payload = serde_json::to_vec(value).map_err(ProtocolError::InvalidMetadata)?;
        validate_payload_length(kind, payload.len())?;
        Ok(Self::new(kind, request_id, payload))
    }

    pub fn stream(
        kind: MessageKind,
        request_id: u64,
        payload: Vec<u8>,
    ) -> Result<Self, ProtocolError> {
        validate_request_id(request_id)?;
        if !kind.is_stream() {
            return Err(ProtocolError::InvalidKindContext { kind });
        }
        validate_payload_length(kind, payload.len())?;
        Ok(Self::new(kind, request_id, payload))
    }

    pub fn password(request_id: u64, secret: &Secret) -> Result<Self, ProtocolError> {
        validate_request_id(request_id)?;
        secret
            .validate_authentication(MAX_SECRET_SIZE)
            .map_err(|_| ProtocolError::InvalidSecret)?;
        Ok(Self::new(
            MessageKind::PasswordResponse,
            request_id,
            secret.as_str().as_bytes().to_vec(),
        ))
    }

    pub fn empty(kind: MessageKind, request_id: u64) -> Result<Self, ProtocolError> {
        validate_request_id(request_id)?;
        if !kind.is_empty() {
            return Err(ProtocolError::InvalidKindContext { kind });
        }
        Ok(Self::new(kind, request_id, Vec::new()))
    }

    fn new(kind: MessageKind, request_id: u64, payload: Vec<u8>) -> Self {
        Self {
            kind,
            request_id,
            payload: Zeroizing::new(payload),
        }
    }

    pub fn kind(&self) -> MessageKind {
        self.kind
    }

    pub fn request_id(&self) -> u64 {
        self.request_id
    }

    pub fn payload_len(&self) -> usize {
        self.payload.len()
    }

    pub fn decode_metadata<T: DeserializeOwned>(&self) -> Result<T, ProtocolError> {
        if !self.kind.is_metadata() {
            return Err(ProtocolError::InvalidKindContext { kind: self.kind });
        }
        serde_json::from_slice(&self.payload).map_err(ProtocolError::InvalidMetadata)
    }

    pub fn into_stream(self) -> Result<Zeroizing<Vec<u8>>, ProtocolError> {
        if !self.kind.is_stream() {
            return Err(ProtocolError::InvalidKindContext { kind: self.kind });
        }
        Ok(Zeroizing::new(self.payload.to_vec()))
    }

    pub fn consume_secret(self) -> Result<Secret, ProtocolError> {
        if self.kind != MessageKind::PasswordResponse {
            return Err(ProtocolError::InvalidKindContext { kind: self.kind });
        }
        let secret = CapturedSecret::new(self.payload.to_vec())
            .into_secret()
            .map_err(|_| ProtocolError::InvalidSecret)?;
        secret
            .validate_authentication(MAX_SECRET_SIZE)
            .map_err(|_| ProtocolError::InvalidSecret)?;
        Ok(secret)
    }
}

impl fmt::Debug for Frame {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Frame")
            .field("kind", &self.kind)
            .field("request_id", &self.request_id)
            .field("payload_len", &self.payload.len())
            .finish()
    }
}

pub async fn read_frame<R>(reader: &mut R) -> Result<Option<Frame>, ProtocolError>
where
    R: AsyncRead + Unpin,
{
    let mut header = [0_u8; HEADER_LEN];
    let first = reader.read(&mut header[..1]).await?;
    if first == 0 {
        return Ok(None);
    }
    read_exact_or_incomplete(reader, &mut header[1..]).await?;

    if header[..4] != MAGIC {
        return Err(ProtocolError::InvalidMagic);
    }
    let version = u16::from_be_bytes([header[4], header[5]]);
    if version != VERSION {
        return Err(ProtocolError::UnsupportedVersion(version));
    }
    let kind = MessageKind::try_from(u16::from_be_bytes([header[6], header[7]]))?;
    let request_id = u64::from_be_bytes(header[8..16].try_into().expect("fixed header range"));
    validate_request_id(request_id)?;
    let payload_len =
        u32::from_be_bytes(header[16..20].try_into().expect("fixed header range")) as usize;
    validate_payload_length(kind, payload_len)?;

    let mut payload = Zeroizing::new(vec![0_u8; payload_len]);
    read_exact_or_incomplete(reader, &mut payload).await?;
    Ok(Some(Frame {
        kind,
        request_id,
        payload,
    }))
}

pub async fn write_frame<W>(writer: &mut W, frame: &Frame) -> Result<(), ProtocolError>
where
    W: AsyncWrite + Unpin,
{
    validate_request_id(frame.request_id)?;
    validate_payload_length(frame.kind, frame.payload.len())?;
    let payload_len =
        u32::try_from(frame.payload.len()).map_err(|_| ProtocolError::PayloadTooLarge {
            length: frame.payload.len(),
            limit: MAX_PAYLOAD_SIZE,
        })?;

    let mut header = [0_u8; HEADER_LEN];
    header[..4].copy_from_slice(&MAGIC);
    header[4..6].copy_from_slice(&VERSION.to_be_bytes());
    header[6..8].copy_from_slice(&(frame.kind as u16).to_be_bytes());
    header[8..16].copy_from_slice(&frame.request_id.to_be_bytes());
    header[16..20].copy_from_slice(&payload_len.to_be_bytes());
    writer.write_all(&header).await?;
    writer.write_all(&frame.payload).await?;
    Ok(())
}

async fn read_exact_or_incomplete<R>(reader: &mut R, bytes: &mut [u8]) -> Result<(), ProtocolError>
where
    R: AsyncRead + Unpin,
{
    match reader.read_exact(bytes).await {
        Ok(_) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => {
            Err(ProtocolError::IncompleteFrame)
        }
        Err(error) => Err(ProtocolError::Io(error)),
    }
}

fn validate_request_id(request_id: u64) -> Result<(), ProtocolError> {
    if request_id == 0 {
        Err(ProtocolError::ZeroRequestId)
    } else {
        Ok(())
    }
}

fn validate_payload_length(kind: MessageKind, length: usize) -> Result<(), ProtocolError> {
    let limit = if kind.is_stream() {
        MAX_STREAM_CHUNK_SIZE
    } else if kind == MessageKind::PasswordResponse {
        MAX_SECRET_SIZE
    } else {
        MAX_PAYLOAD_SIZE
    };
    if length > limit {
        return Err(ProtocolError::PayloadTooLarge { length, limit });
    }
    if kind.is_empty() && length != 0 {
        return Err(ProtocolError::ExpectedEmptyPayload { kind });
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    Check,
    Execute,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Hello {
    pub mode: Mode,
    pub sudo_path: String,
    pub auth_user: String,
    pub setup_timeout_secs: u64,
    pub auth_timeout_secs: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Ready {
    pub build: String,
    pub protocol: u16,
    pub platform: String,
    pub auth_user: String,
    pub uid: u32,
    pub cwd: String,
    pub password_limit: usize,
    pub features: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Start {
    pub executable: String,
    pub arguments: Vec<String>,
    pub cwd: Option<String>,
    pub run_as: String,
    pub auth_user: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PasswordRequest {
    pub auth_user: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Stream {
    Stdin,
    Stdout,
    Stderr,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct WindowUpdate {
    pub stream: Stream,
    pub bytes: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Cancel {
    pub signal: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ExecutionResult {
    pub exit_code: Option<i32>,
    pub signal: Option<i32>,
    pub password_delivered: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FailureReason {
    Preflight,
    Protocol,
    Credential,
    CompletionUnknown,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Failure {
    pub reason: FailureReason,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Credit {
    available: usize,
}

impl Credit {
    pub fn new() -> Self {
        Self {
            available: INITIAL_STREAM_CREDIT,
        }
    }

    pub fn available(self) -> usize {
        self.available
    }

    pub fn consume(&mut self, bytes: usize) -> Result<(), ProtocolError> {
        if bytes > self.available {
            return Err(ProtocolError::InsufficientCredit {
                requested: bytes,
                available: self.available,
            });
        }
        self.available -= bytes;
        Ok(())
    }

    pub fn grant(&mut self, bytes: u32) -> Result<(), ProtocolError> {
        self.available = self
            .available
            .checked_add(bytes as usize)
            .filter(|available| *available <= MAX_STREAM_CREDIT)
            .ok_or(ProtocolError::CreditOverflow {
                maximum: MAX_STREAM_CREDIT,
            })?;
        Ok(())
    }
}

impl Default for Credit {
    fn default() -> Self {
        Self::new()
    }
}
