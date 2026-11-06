//! Framing of the one-shot askpass channels.
//!
//! sudo and ssh run an agentenv companion as their askpass program. The
//! companion connects to a private channel of the invocation that launched
//! it, sends request fields, and receives at most one secret. Every frame
//! starts with a five-byte channel magic and a big-endian `u16` length. The
//! local sudo channel adds a status byte to each reply so a refusal is
//! explicit; the SSH login channel has no status byte and refuses by closing.
//!
//! The serving side reads asynchronously from a socket or pipe. Companions
//! encode their fields and decode the reply header here and do their own
//! blocking or asynchronous I/O around it.

use std::io;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use zeroize::Zeroizing;

use crate::credential::Secret;

/// Largest request field: a session marker or a prompt.
pub const FIELD_LIMIT: usize = 4096;

/// One channel's framing.
#[derive(Debug, Clone, Copy)]
pub struct Channel {
    magic: &'static [u8; 5],
    status: bool,
}

/// The local sudo broker channel.
pub const SUDO: Channel = Channel {
    magic: b"AGES\x01",
    status: true,
};

/// The SSH login broker channel.
pub const SSH_LOGIN: Channel = Channel {
    magic: b"AGEL\x01",
    status: false,
};

/// Why a request field could not be read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldError {
    /// The peer closed or failed before the field was complete.
    Incomplete,
    /// The field carried another channel's magic or an invalid length.
    Invalid,
}

const STATUS_SECRET: u8 = 0;
const STATUS_REFUSED: u8 = 1;

impl Channel {
    /// Reads one request field.
    pub async fn read_field(
        self,
        reader: &mut (impl AsyncRead + Unpin),
    ) -> Result<Zeroizing<Vec<u8>>, FieldError> {
        let mut magic = [0; 5];
        reader
            .read_exact(&mut magic)
            .await
            .map_err(|_| FieldError::Incomplete)?;
        if &magic != self.magic {
            return Err(FieldError::Invalid);
        }
        let length = reader
            .read_u16()
            .await
            .map_err(|_| FieldError::Incomplete)? as usize;
        if !(1..=FIELD_LIMIT).contains(&length) {
            return Err(FieldError::Invalid);
        }
        let mut field = Zeroizing::new(vec![0; length]);
        reader
            .read_exact(&mut field)
            .await
            .map_err(|_| FieldError::Incomplete)?;
        Ok(field)
    }

    /// Writes the one secret reply. The caller closes or acknowledges the
    /// channel afterwards as its transport requires.
    pub async fn write_secret(
        self,
        writer: &mut (impl AsyncWrite + Unpin),
        secret: &Secret,
    ) -> io::Result<()> {
        let value = secret.as_str().as_bytes();
        let length = u16::try_from(value.len()).map_err(|_| io::ErrorKind::InvalidInput)?;
        writer.write_all(self.magic).await?;
        if self.status {
            writer.write_u8(STATUS_SECRET).await?;
        }
        writer.write_u16(length).await?;
        writer.write_all(value).await
    }

    /// Writes an explicit refusal and closes the channel. A channel without
    /// a status byte refuses by closing without a reply.
    pub async fn refuse(self, writer: &mut (impl AsyncWrite + Unpin)) -> io::Result<()> {
        if self.status {
            writer.write_all(self.magic).await?;
            writer.write_u8(STATUS_REFUSED).await?;
            writer.write_u16(0).await?;
        }
        writer.shutdown().await
    }

    /// Encodes one request field for a companion to send.
    pub fn encode_field(self, value: &[u8]) -> io::Result<Vec<u8>> {
        if !(1..=FIELD_LIMIT).contains(&value.len()) {
            return Err(io::ErrorKind::InvalidInput.into());
        }
        let mut frame = Vec::with_capacity(self.magic.len() + 2 + value.len());
        frame.extend_from_slice(self.magic);
        frame.extend_from_slice(&(value.len() as u16).to_be_bytes());
        frame.extend_from_slice(value);
        Ok(frame)
    }

    /// Length of the reply header a companion reads before the secret.
    pub fn reply_header_len(self) -> usize {
        self.magic.len() + usize::from(self.status) + 2
    }

    /// Validates a reply header and returns the secret length, which must be
    /// between 1 and `limit` bytes. A refusal or another channel's reply is
    /// an error.
    pub fn secret_len(self, header: &[u8], limit: usize) -> io::Result<usize> {
        let invalid = || io::Error::from(io::ErrorKind::InvalidData);
        if header.len() != self.reply_header_len() || &header[..5] != self.magic {
            return Err(invalid());
        }
        if self.status && header[5] != STATUS_SECRET {
            return Err(invalid());
        }
        let length_at = header.len() - 2;
        let length = u16::from_be_bytes([header[length_at], header[length_at + 1]]) as usize;
        if !(1..=limit).contains(&length) {
            return Err(invalid());
        }
        Ok(length)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::credential::CapturedSecret;

    fn secret(value: &str) -> Secret {
        CapturedSecret::new(value.as_bytes().to_vec())
            .into_secret()
            .expect("test secret")
    }

    #[tokio::test]
    async fn sudo_frames_keep_their_wire_bytes() {
        assert_eq!(
            SUDO.encode_field(b"ab").unwrap(),
            b"AGES\x01\x00\x02ab".to_vec()
        );
        let mut reply = Vec::new();
        SUDO.write_secret(&mut reply, &secret("pw")).await.unwrap();
        assert_eq!(reply, b"AGES\x01\x00\x00\x02pw".to_vec());
        assert_eq!(SUDO.secret_len(&reply[..8], 255).unwrap(), 2);
        let mut refusal = Vec::new();
        SUDO.refuse(&mut refusal).await.unwrap();
        assert_eq!(refusal, b"AGES\x01\x01\x00\x00".to_vec());
        assert!(SUDO.secret_len(&refusal, 255).is_err());
    }

    #[tokio::test]
    async fn ssh_login_frames_have_no_status_byte() {
        let mut reply = Vec::new();
        SSH_LOGIN
            .write_secret(&mut reply, &secret("pw"))
            .await
            .unwrap();
        assert_eq!(reply, b"AGEL\x01\x00\x02pw".to_vec());
        assert_eq!(SSH_LOGIN.secret_len(&reply[..7], 255).unwrap(), 2);
        let mut refusal = Vec::new();
        SSH_LOGIN.refuse(&mut refusal).await.unwrap();
        assert!(refusal.is_empty());
    }

    #[tokio::test]
    async fn fields_reject_other_channels_and_bad_lengths() {
        let frame = SSH_LOGIN.encode_field(b"marker").unwrap();
        assert_eq!(
            SUDO.read_field(&mut frame.as_slice()).await,
            Err(FieldError::Invalid)
        );
        assert_eq!(
            SSH_LOGIN
                .read_field(&mut frame.as_slice())
                .await
                .unwrap()
                .as_slice(),
            b"marker"
        );
        assert_eq!(
            SUDO.read_field(&mut &b"AGES\x01\x00\x00"[..]).await,
            Err(FieldError::Invalid)
        );
        assert_eq!(
            SUDO.read_field(&mut &b"AGES\x01\x00\x05ab"[..]).await,
            Err(FieldError::Incomplete)
        );
        assert!(SUDO.encode_field(&[]).is_err());
        assert!(SUDO.encode_field(&vec![b'x'; FIELD_LIMIT + 1]).is_err());
    }
}
