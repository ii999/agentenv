use agentenv::credential::CapturedSecret;
use agentenv::sudo::protocol::{
    read_frame, write_frame, Credit, Frame, Hello, MessageKind, Mode, ProtocolError,
    INITIAL_STREAM_CREDIT, MAX_PAYLOAD_SIZE, MAX_SECRET_SIZE, MAX_STREAM_CHUNK_SIZE,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn header(kind: u16, request_id: u64, payload_len: u32) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(20);
    bytes.extend_from_slice(b"AGEP");
    bytes.extend_from_slice(&1_u16.to_be_bytes());
    bytes.extend_from_slice(&kind.to_be_bytes());
    bytes.extend_from_slice(&request_id.to_be_bytes());
    bytes.extend_from_slice(&payload_len.to_be_bytes());
    bytes
}

async fn encoded(frame: &Frame) -> Vec<u8> {
    let capacity = 20 + frame.payload_len();
    let (mut client, mut server) = tokio::io::duplex(capacity.max(64));
    write_frame(&mut client, frame).await.expect("frame writes");
    client.shutdown().await.expect("writer shuts down");
    let mut bytes = Vec::new();
    server.read_to_end(&mut bytes).await.expect("frame reads");
    bytes
}

async fn decoded(bytes: Vec<u8>) -> Result<Option<Frame>, ProtocolError> {
    let capacity = bytes.len().max(64);
    let (mut client, mut server) = tokio::io::duplex(capacity);
    client.write_all(&bytes).await.expect("fixture writes");
    client.shutdown().await.expect("fixture shuts down");
    read_frame(&mut server).await
}

#[tokio::test]
async fn hello_has_a_stable_golden_encoding() {
    let hello = Hello {
        mode: Mode::Execute,
        sudo_path: "/usr/bin/sudo".to_owned(),
        auth_user: "operator".to_owned(),
        setup_timeout_secs: 30,
        auth_timeout_secs: 60,
    };
    let frame =
        Frame::metadata(MessageKind::Hello, 0x0102_0304_0506_0708, &hello).expect("valid metadata");

    let payload = br#"{"mode":"execute","sudo_path":"/usr/bin/sudo","auth_user":"operator","setup_timeout_secs":30,"auth_timeout_secs":60}"#;
    let mut expected = header(1, 0x0102_0304_0506_0708, payload.len() as u32);
    expected.extend_from_slice(payload);
    assert_eq!(encoded(&frame).await, expected);

    let decoded = decoded(expected)
        .await
        .expect("golden frame parses")
        .expect("a frame exists");
    assert_eq!(decoded.kind(), MessageKind::Hello);
    assert_eq!(decoded.request_id(), 0x0102_0304_0506_0708);
    assert_eq!(
        decoded.decode_metadata::<Hello>().expect("typed JSON"),
        hello
    );
}

#[test]
fn message_kind_numbers_are_frozen() {
    let kinds = [
        MessageKind::Hello,
        MessageKind::Ready,
        MessageKind::Start,
        MessageKind::PasswordRequest,
        MessageKind::PasswordResponse,
        MessageKind::PasswordUnavailable,
        MessageKind::StdinChunk,
        MessageKind::StdinEof,
        MessageKind::StdoutChunk,
        MessageKind::StdoutEof,
        MessageKind::StderrChunk,
        MessageKind::StderrEof,
        MessageKind::WindowUpdate,
        MessageKind::Cancel,
        MessageKind::Result,
        MessageKind::Failure,
    ];
    assert_eq!(
        kinds.map(|kind| kind as u16),
        [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16]
    );
}

#[tokio::test]
async fn malformed_headers_fail_without_resynchronizing_or_allocating_payloads() {
    let mut invalid_magic = header(1, 1, 0);
    invalid_magic[..4].copy_from_slice(b"junk");
    assert!(matches!(
        decoded(invalid_magic).await,
        Err(ProtocolError::InvalidMagic)
    ));

    let mut invalid_version = header(1, 1, 0);
    invalid_version[4..6].copy_from_slice(&2_u16.to_be_bytes());
    assert!(matches!(
        decoded(invalid_version).await,
        Err(ProtocolError::UnsupportedVersion(2))
    ));
    assert!(matches!(
        decoded(header(99, 1, 0)).await,
        Err(ProtocolError::UnknownMessageKind(99))
    ));
    assert!(matches!(
        decoded(header(1, 0, 0)).await,
        Err(ProtocolError::ZeroRequestId)
    ));
    assert!(matches!(
        decoded(header(1, 1, (MAX_PAYLOAD_SIZE + 1) as u32)).await,
        Err(ProtocolError::PayloadTooLarge { .. })
    ));
    assert!(matches!(
        decoded(header(7, 1, (MAX_STREAM_CHUNK_SIZE + 1) as u32)).await,
        Err(ProtocolError::PayloadTooLarge { .. })
    ));
    assert!(matches!(
        decoded(header(5, 1, (MAX_SECRET_SIZE + 1) as u32)).await,
        Err(ProtocolError::PayloadTooLarge { .. })
    ));
    assert!(matches!(
        decoded(header(8, 1, 1)).await,
        Err(ProtocolError::ExpectedEmptyPayload {
            kind: MessageKind::StdinEof
        })
    ));
}

#[tokio::test]
async fn clean_eof_is_distinct_from_a_truncated_header_or_payload() {
    assert!(decoded(Vec::new()).await.expect("clean EOF").is_none());
    assert!(matches!(
        decoded(b"AGEP".to_vec()).await,
        Err(ProtocolError::IncompleteFrame)
    ));

    let mut truncated_payload = header(7, 4, 3);
    truncated_payload.extend_from_slice(b"ab");
    assert!(matches!(
        decoded(truncated_payload).await,
        Err(ProtocolError::IncompleteFrame)
    ));
}

#[tokio::test]
async fn payload_limits_accept_the_exact_boundary() {
    let stream = Frame::stream(
        MessageKind::StdoutChunk,
        5,
        vec![b'x'; MAX_STREAM_CHUNK_SIZE],
    )
    .expect("the maximum stream chunk is accepted");
    assert_eq!(stream.payload_len(), MAX_STREAM_CHUNK_SIZE);

    let secret = CapturedSecret::new(vec![b's'; MAX_SECRET_SIZE])
        .into_secret()
        .expect("valid maximum-length secret");
    let password = Frame::password(6, &secret).expect("the maximum secret is accepted");
    assert_eq!(password.payload_len(), MAX_SECRET_SIZE);

    let mut metadata = header(1, 7, MAX_PAYLOAD_SIZE as u32);
    metadata.extend_from_slice(&vec![b' '; MAX_PAYLOAD_SIZE]);
    let frame = decoded(metadata)
        .await
        .expect("the maximum metadata frame is accepted")
        .expect("frame exists");
    assert_eq!(frame.payload_len(), MAX_PAYLOAD_SIZE);
}

#[tokio::test]
async fn metadata_is_utf8_typed_and_rejects_unknown_fields() {
    let mut bytes = header(1, 9, 2);
    bytes.extend_from_slice(&[0xff, 0xfe]);
    let frame = decoded(bytes)
        .await
        .expect("framing is valid")
        .expect("frame exists");
    assert!(matches!(
        frame.decode_metadata::<Hello>(),
        Err(ProtocolError::InvalidMetadata(_))
    ));

    let payload = br#"{"mode":"check","sudo_path":"/usr/bin/sudo","auth_user":"operator","setup_timeout_secs":30,"auth_timeout_secs":60,"extra":true}"#;
    let mut bytes = header(1, 10, payload.len() as u32);
    bytes.extend_from_slice(payload);
    let frame = decoded(bytes)
        .await
        .expect("framing is valid")
        .expect("frame exists");
    assert!(matches!(
        frame.decode_metadata::<Hello>(),
        Err(ProtocolError::InvalidMetadata(_))
    ));
}

#[tokio::test]
async fn password_frames_use_the_dedicated_validated_boundary_and_redacted_debug() {
    let secret = CapturedSecret::new("sëcret value".as_bytes().to_vec())
        .into_secret()
        .expect("valid secret");
    let frame = Frame::password(17, &secret).expect("valid password frame");
    let debug = format!("{frame:?}");
    assert_eq!(
        debug,
        "Frame { kind: PasswordResponse, request_id: 17, payload_len: 13 }"
    );
    assert!(!debug.contains("sëcret"));

    let wire = encoded(&frame).await;
    let received = decoded(wire)
        .await
        .expect("password frame parses")
        .expect("frame exists")
        .consume_secret()
        .expect("password is validated at receipt");
    assert_eq!(
        encoded(&Frame::password(17, &received).expect("re-encodes")).await,
        encoded(&frame).await
    );

    for candidate in [b"line\n".as_slice(), b"line\r"] {
        let candidate = CapturedSecret::new(candidate.to_vec())
            .into_secret()
            .expect("credential domain permits boundary validation later");
        assert!(matches!(
            Frame::password(1, &candidate),
            Err(ProtocolError::InvalidSecret)
        ));
    }

    let frame = decoded(header(5, 2, 0))
        .await
        .expect("bounded frame parses")
        .expect("frame exists");
    assert!(matches!(
        frame.consume_secret(),
        Err(ProtocolError::InvalidSecret)
    ));

    let mut contains_nul = header(5, 3, 8);
    contains_nul.extend_from_slice(b"nul\0byte");
    let frame = decoded(contains_nul)
        .await
        .expect("bounded frame parses")
        .expect("frame exists");
    assert!(matches!(
        frame.consume_secret(),
        Err(ProtocolError::InvalidSecret)
    ));

    let mut invalid_utf8 = header(5, 4, 2);
    invalid_utf8.extend_from_slice(&[0xc3, 0x28]);
    let frame = decoded(invalid_utf8)
        .await
        .expect("bounded frame parses")
        .expect("frame exists");
    assert!(matches!(
        frame.consume_secret(),
        Err(ProtocolError::InvalidSecret)
    ));
}

#[test]
fn constructors_enforce_payload_kind_boundaries() {
    assert!(matches!(
        Frame::metadata(
            MessageKind::PasswordResponse,
            1,
            &Hello {
                mode: Mode::Check,
                sudo_path: "/usr/bin/sudo".to_owned(),
                auth_user: "operator".to_owned(),
                setup_timeout_secs: 30,
                auth_timeout_secs: 60,
            }
        ),
        Err(ProtocolError::InvalidKindContext {
            kind: MessageKind::PasswordResponse
        })
    ));
    assert!(matches!(
        Frame::stream(
            MessageKind::StdoutChunk,
            1,
            vec![0; MAX_STREAM_CHUNK_SIZE + 1]
        ),
        Err(ProtocolError::PayloadTooLarge { .. })
    ));
    assert!(matches!(
        Frame::empty(MessageKind::StdinChunk, 1),
        Err(ProtocolError::InvalidKindContext {
            kind: MessageKind::StdinChunk
        })
    ));
}

#[test]
fn credit_can_only_replenish_consumed_bytes_without_overflow() {
    let mut credit = Credit::new();
    assert_eq!(credit.available(), INITIAL_STREAM_CREDIT);
    assert!(matches!(
        credit.grant(1),
        Err(ProtocolError::CreditOverflow { .. })
    ));
    assert_eq!(credit.available(), INITIAL_STREAM_CREDIT);

    credit.consume(32 * 1024).expect("credit is available");
    credit.grant(32 * 1024).expect("consumed credit returns");
    assert_eq!(credit.available(), INITIAL_STREAM_CREDIT);
    credit
        .consume(INITIAL_STREAM_CREDIT)
        .expect("the complete window may be consumed");
    assert!(matches!(
        credit.consume(1),
        Err(ProtocolError::InsufficientCredit {
            requested: 1,
            available: 0
        })
    ));
    assert!(matches!(
        credit.grant(u32::MAX),
        Err(ProtocolError::CreditOverflow { .. })
    ));
    assert_eq!(credit.available(), 0);
}
