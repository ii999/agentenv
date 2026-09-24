use std::time::Duration;

use agentenv::credential::CapturedSecret;
use agentenv::sudo::protocol::{
    read_frame, write_frame, Cancel, ExecutionResult, Failure, FailureReason, Frame, MessageKind,
    PasswordRequest, Start, Stream, MAX_STREAM_CHUNK_SIZE,
};
#[cfg(unix)]
use agentenv::sudo::protocol::{Hello, Mode, Ready};
use agentenv::sudo::transport::{self, Side, StreamEvent, TransportError};
use tokio::io::AsyncWriteExt;

#[cfg(unix)]
use {
    std::ffi::CStr,
    std::fs,
    std::os::unix::fs::PermissionsExt,
    std::process::Stdio,
    tokio::process::{Child, Command},
};

async fn session_pair(
    request_id: u64,
) -> (transport::SessionTransport, transport::SessionTransport) {
    let (client_io, helper_io) = tokio::io::duplex(256 * 1024);
    let (client_read, client_write) = tokio::io::split(client_io);
    let (helper_read, helper_write) = tokio::io::split(helper_io);
    let client = transport::spawn(client_read, client_write, request_id, Side::Client);
    let mut helper = transport::spawn(helper_read, helper_write, request_id, Side::Helper);
    let start = Frame::metadata(
        MessageKind::Start,
        request_id,
        &Start {
            executable: "/usr/bin/true".to_owned(),
            arguments: Vec::new(),
            cwd: None,
            run_as: "root".to_owned(),
            auth_user: "operator".to_owned(),
        },
    )
    .expect("start frame");
    client
        .sender
        .send_control_flushed(start)
        .await
        .expect("start writes");
    assert_eq!(
        helper
            .control
            .recv()
            .await
            .expect("helper receives start")
            .kind(),
        MessageKind::Start
    );
    helper.activate_streams().expect("start is accepted");
    (client, helper)
}

#[tokio::test]
async fn more_than_one_mibibyte_streams_with_bounded_credit() {
    let request_id = 41;
    let (client, mut helper) = session_pair(request_id).await;
    let mut stdin = helper.take_stream(Stream::Stdin).expect("stdin receiver");
    let sender = client.sender.clone();
    let chunk_count = 40;
    let send = tokio::spawn(async move {
        for sequence in 0..chunk_count {
            sender
                .send_stream(
                    Stream::Stdin,
                    vec![(sequence % 251) as u8; MAX_STREAM_CHUNK_SIZE],
                )
                .await
                .expect("credit returns as chunks are consumed");
        }
        sender.send_eof(Stream::Stdin).await.expect("stdin EOF");
    });

    let mut received = 0_usize;
    let mut chunks = 0_usize;
    while let StreamEvent::Chunk(bytes) = stdin.recv().await.expect("stream stays open") {
        assert_eq!(bytes.len(), MAX_STREAM_CHUNK_SIZE);
        assert_eq!(bytes[0], (chunks % 251) as u8);
        received += bytes.len();
        chunks += 1;
        helper
            .sender
            .acknowledge(Stream::Stdin, bytes.len() as u32)
            .await
            .expect("consumed bytes replenish credit");
    }
    send.await.expect("send task completes");
    assert_eq!(chunks, chunk_count);
    assert_eq!(received, chunk_count * MAX_STREAM_CHUNK_SIZE);
    assert!(received > 1024 * 1024);
}

#[tokio::test]
async fn many_short_frames_use_byte_capacity_and_do_not_starve_control() {
    let request_id = 45;
    let (client, mut helper) = session_pair(request_id).await;
    let mut stdin = helper.take_stream(Stream::Stdin).expect("stdin receiver");
    let sender = client.sender.clone();
    let send = tokio::spawn(async move {
        for byte in 0..4096_u32 {
            sender
                .send_stream(Stream::Stdin, vec![(byte % 251) as u8])
                .await
                .expect("short frame remains inside byte credit");
        }
    });
    send.await.expect("all short frames are accepted");

    let cancel = Frame::metadata(MessageKind::Cancel, request_id, &Cancel { signal: 15 })
        .expect("cancel frame");
    client
        .sender
        .send_control_flushed(cancel)
        .await
        .expect("control remains independent");
    assert_eq!(
        helper.control.recv().await.expect("cancel arrives").kind(),
        MessageKind::Cancel
    );

    let mut received = Vec::new();
    while received.len() < 4096 {
        match stdin.recv().await.expect("queued input remains available") {
            StreamEvent::Chunk(bytes) => received.extend_from_slice(&bytes),
            StreamEvent::Eof => panic!("unexpected EOF"),
        }
    }
    assert_eq!(received.len(), 4096);
    assert!(received
        .iter()
        .enumerate()
        .all(|(index, byte)| *byte == (index % 251) as u8));
}

#[tokio::test]
async fn cancellation_is_delivered_while_target_stdin_is_blocked() {
    let request_id = 42;
    let (client, mut helper) = session_pair(request_id).await;
    let _blocked_stdin = helper.take_stream(Stream::Stdin).expect("stdin receiver");
    let stream_sender = client.sender.clone();
    let blocked = tokio::spawn(async move {
        for _ in 0..5 {
            stream_sender
                .send_stream(Stream::Stdin, vec![0; MAX_STREAM_CHUNK_SIZE])
                .await
                .expect("session remains active");
        }
    });

    tokio::time::sleep(Duration::from_millis(10)).await;
    let cancel = Frame::metadata(MessageKind::Cancel, request_id, &Cancel { signal: 15 })
        .expect("cancel frame");
    client
        .sender
        .send_control(cancel)
        .await
        .expect("reserved control capacity");
    let received = tokio::time::timeout(Duration::from_secs(1), helper.control.recv())
        .await
        .expect("cancel is not starved")
        .expect("control remains open");
    assert_eq!(received.kind(), MessageKind::Cancel);
    blocked.abort();
}

#[tokio::test]
async fn password_request_is_delivered_while_stdout_consumer_is_blocked() {
    let request_id = 43;
    let (mut client, helper) = session_pair(request_id).await;
    let _blocked_stdout = client.take_stream(Stream::Stdout).expect("stdout receiver");
    let stream_sender = helper.sender.clone();
    let blocked = tokio::spawn(async move {
        for _ in 0..5 {
            stream_sender
                .send_stream(Stream::Stdout, vec![0; MAX_STREAM_CHUNK_SIZE])
                .await
                .expect("session remains active");
        }
    });

    tokio::task::yield_now().await;
    let request = Frame::metadata(
        MessageKind::PasswordRequest,
        request_id,
        &PasswordRequest {
            auth_user: "operator".to_owned(),
        },
    )
    .expect("password request frame");
    helper
        .sender
        .send_control(request)
        .await
        .expect("reserved control capacity");
    let received = tokio::time::timeout(Duration::from_secs(1), client.control.recv())
        .await
        .expect("password request is not starved")
        .expect("control remains open");
    assert_eq!(received.kind(), MessageKind::PasswordRequest);
    blocked.abort();
}

#[tokio::test]
async fn output_eof_flushes_before_terminal_result() {
    let request_id = 44;
    let (mut client, helper) = session_pair(request_id).await;
    let mut stdout = client.take_stream(Stream::Stdout).expect("stdout receiver");
    helper
        .sender
        .send_stream(Stream::Stdout, b"complete".to_vec())
        .await
        .expect("stdout chunk");
    helper
        .sender
        .send_eof(Stream::Stdout)
        .await
        .expect("EOF is flushed");
    helper
        .sender
        .send_eof(Stream::Stderr)
        .await
        .expect("stderr EOF is flushed");
    let result = Frame::metadata(
        MessageKind::Result,
        request_id,
        &ExecutionResult {
            exit_code: Some(0),
            signal: None,
            password_delivered: false,
        },
    )
    .expect("result frame");
    helper
        .sender
        .send_terminal(result)
        .await
        .expect("terminal frame is flushed");

    assert!(
        tokio::time::timeout(Duration::from_millis(20), client.control.recv())
            .await
            .is_err()
    );

    assert!(matches!(
        stdout.recv().await,
        Some(StreamEvent::Chunk(bytes)) if bytes.as_slice() == b"complete"
    ));
    assert!(matches!(stdout.recv().await, Some(StreamEvent::Eof)));
    let mut stderr = client.take_stream(Stream::Stderr).expect("stderr receiver");
    assert!(matches!(stderr.recv().await, Some(StreamEvent::Eof)));
    assert_eq!(
        client.control.recv().await.expect("result frame").kind(),
        MessageKind::Result
    );
}

#[tokio::test]
async fn terminal_frame_seals_the_sending_side() {
    let request_id = 45;
    let (mut client, mut helper) = session_pair(request_id).await;
    client
        .sender
        .send_stream(Stream::Stdin, b"unread".to_vec())
        .await
        .expect("stdin chunk");
    assert!(matches!(
        helper
            .take_stream(Stream::Stdin)
            .expect("stdin receiver")
            .recv()
            .await,
        Some(StreamEvent::Chunk(_))
    ));
    let mut stdout = client.take_stream(Stream::Stdout).expect("stdout receiver");
    let mut stderr = client.take_stream(Stream::Stderr).expect("stderr receiver");
    helper
        .sender
        .send_eof(Stream::Stdout)
        .await
        .expect("stdout EOF");
    helper
        .sender
        .send_eof(Stream::Stderr)
        .await
        .expect("stderr EOF");
    assert!(matches!(stdout.recv().await, Some(StreamEvent::Eof)));
    assert!(matches!(stderr.recv().await, Some(StreamEvent::Eof)));
    let result = Frame::metadata(
        MessageKind::Result,
        request_id,
        &ExecutionResult {
            exit_code: Some(0),
            signal: None,
            password_delivered: false,
        },
    )
    .expect("result frame");
    helper
        .sender
        .send_terminal(result)
        .await
        .expect("terminal frame is flushed");

    assert!(helper.sender.acknowledge(Stream::Stdin, 6).await.is_err());
    assert_eq!(
        client.control.recv().await.expect("result frame").kind(),
        MessageKind::Result
    );
    let mut reader = client.reader_outcome();
    drop(helper);
    let closed = *reader
        .wait_for(Option::is_some)
        .await
        .expect("reader reports its outcome");
    assert_eq!(closed, Some(Ok(())));
}

#[tokio::test]
async fn client_rejects_invalid_terminal_order_and_frames_after_terminal() {
    async fn assert_protocol_failure(frames: Vec<Frame>, request_id: u64) {
        let (mut peer, endpoint) = tokio::io::duplex(16 * 1024);
        let (read, write) = tokio::io::split(endpoint);
        let session = transport::spawn(read, write, request_id, Side::Client);
        let mut failure = session.failure();
        for frame in frames {
            write_frame(&mut peer, &frame)
                .await
                .expect("fixture writes");
        }
        peer.flush().await.expect("fixture flushes");
        tokio::time::timeout(Duration::from_secs(1), failure.changed())
            .await
            .expect("violation is reported")
            .expect("failure channel remains open");
        assert_eq!(*failure.borrow(), Some(TransportError::Protocol));
    }

    let result = |request_id| {
        Frame::metadata(
            MessageKind::Result,
            request_id,
            &ExecutionResult {
                exit_code: Some(0),
                signal: None,
                password_delivered: false,
            },
        )
        .expect("result frame")
    };
    assert_protocol_failure(vec![result(46)], 46).await;

    let failure = |request_id| {
        Frame::metadata(
            MessageKind::Failure,
            request_id,
            &Failure {
                reason: FailureReason::Protocol,
            },
        )
        .expect("failure frame")
    };
    assert_protocol_failure(vec![failure(47), failure(47)], 47).await;
    assert_protocol_failure(
        vec![
            failure(48),
            Frame::stream(MessageKind::StdoutChunk, 48, b"late".to_vec()).expect("stream frame"),
        ],
        48,
    )
    .await;
}

#[tokio::test]
async fn malformed_session_id_and_duplicate_eof_fail_closed() {
    let (peer, endpoint) = tokio::io::duplex(4096);
    let (read, write) = tokio::io::split(endpoint);
    let session = transport::spawn(read, write, 50, Side::Helper);
    let mut failure = session.failure();
    let (peer_read, mut peer_write) = tokio::io::split(peer);
    let wrong =
        Frame::metadata(MessageKind::Cancel, 51, &Cancel { signal: 15 }).expect("fixture frame");
    agentenv::sudo::protocol::write_frame(&mut peer_write, &wrong)
        .await
        .expect("fixture writes");
    peer_write.flush().await.expect("fixture flushes");
    tokio::time::timeout(Duration::from_secs(1), failure.changed())
        .await
        .expect("failure is reported")
        .expect("watch remains open");
    assert_eq!(*failure.borrow(), Some(TransportError::RequestId));
    drop(peer_read);

    let (mut client, helper) = session_pair(52).await;
    let mut stdout = client.take_stream(Stream::Stdout).expect("stdout receiver");
    let mut failure = client.failure();
    helper
        .sender
        .send_eof(Stream::Stdout)
        .await
        .expect("first EOF");
    helper
        .sender
        .send_eof(Stream::Stdout)
        .await
        .expect("second EOF reaches peer validation");
    assert!(matches!(stdout.recv().await, Some(StreamEvent::Eof)));
    tokio::time::timeout(Duration::from_secs(1), failure.changed())
        .await
        .expect("duplicate EOF fails")
        .expect("watch remains open");
    assert_eq!(*failure.borrow(), Some(TransportError::Protocol));
}

#[tokio::test]
async fn dropping_a_queued_flushed_password_response_revokes_it() {
    let request_id = 53;
    let (_input_peer, input) = tokio::io::duplex(64);
    let (reader, _input_writer) = tokio::io::split(input);
    let (output, mut output_peer) = tokio::io::duplex(64);
    let (_output_reader, writer) = tokio::io::split(output);
    let client = transport::spawn(reader, writer, request_id, Side::Client);
    let start = Frame::metadata(
        MessageKind::Start,
        request_id,
        &Start {
            executable: "/usr/bin/true".to_owned(),
            arguments: vec!["x".repeat(4096)],
            cwd: None,
            run_as: "root".to_owned(),
            auth_user: "operator".to_owned(),
        },
    )
    .expect("large start frame");
    client
        .sender
        .send_control(start)
        .await
        .expect("start queues and blocks the writer");

    let secret = CapturedSecret::new(b"synthetic-password".to_vec())
        .into_secret()
        .expect("valid synthetic secret");
    let password = Frame::password(request_id, &secret).expect("password frame");
    let password_sender = client.sender.clone();
    let queued_password =
        tokio::spawn(async move { password_sender.send_control_flushed(password).await });
    tokio::time::sleep(Duration::from_millis(10)).await;
    queued_password.abort();
    let _ = queued_password.await;

    let first = read_frame(&mut output_peer)
        .await
        .expect("first frame parses")
        .expect("first frame exists");
    assert_eq!(first.kind(), MessageKind::Start);

    let cancel = Frame::metadata(MessageKind::Cancel, request_id, &Cancel { signal: 15 })
        .expect("cancel frame");
    let cancel_sender = client.sender.clone();
    let sent_cancel = tokio::spawn(async move { cancel_sender.send_control_flushed(cancel).await });
    let second = read_frame(&mut output_peer)
        .await
        .expect("second frame parses")
        .expect("second frame exists");
    assert_eq!(second.kind(), MessageKind::Cancel);
    sent_cancel
        .await
        .expect("cancel task completes")
        .expect("cancel flushes");
}

#[cfg(unix)]
#[tokio::test]
async fn helper_check_reports_identity_without_requesting_a_password() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let sudo = fake_sudo(temporary.path());
    let mut child = spawn_helper();
    let mut writer = child.stdin.take().expect("helper stdin");
    let mut reader = child.stdout.take().expect("helper stdout");
    let hello = Frame::metadata(
        MessageKind::Hello,
        61,
        &Hello {
            mode: Mode::Check,
            sudo_path: sudo.to_string_lossy().into_owned(),
            auth_user: account_name(),
            setup_timeout_secs: 5,
            auth_timeout_secs: 5,
        },
    )
    .expect("hello frame");
    write_frame(&mut writer, &hello)
        .await
        .expect("hello writes");
    writer.flush().await.expect("hello flushes");

    let ready = read_frame(&mut reader)
        .await
        .expect("ready parses")
        .expect("ready exists");
    assert_eq!(ready.kind(), MessageKind::Ready);
    let ready: Ready = ready.decode_metadata().expect("typed ready metadata");
    assert_eq!(ready.protocol, 1);
    assert_eq!(ready.auth_user, account_name());
    assert_eq!(ready.password_limit, 255);
    assert!(ready.features.iter().any(|feature| feature == "credits"));
    assert!(read_frame(&mut reader)
        .await
        .expect("check closes cleanly")
        .is_none());
    assert!(child.wait().await.expect("helper exits").success());
}

#[cfg(unix)]
#[tokio::test]
async fn helper_rejects_authentication_messages_before_start() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let sudo = fake_sudo(temporary.path());
    let mut child = spawn_helper();
    let mut writer = child.stdin.take().expect("helper stdin");
    let mut reader = child.stdout.take().expect("helper stdout");
    let request_id = 64;
    let hello = Frame::metadata(
        MessageKind::Hello,
        request_id,
        &Hello {
            mode: Mode::Execute,
            sudo_path: sudo.to_string_lossy().into_owned(),
            auth_user: account_name(),
            setup_timeout_secs: 5,
            auth_timeout_secs: 5,
        },
    )
    .expect("hello frame");
    write_frame(&mut writer, &hello)
        .await
        .expect("hello writes");
    writer.flush().await.expect("hello flushes");
    assert_eq!(
        read_frame(&mut reader)
            .await
            .expect("ready parses")
            .expect("ready exists")
            .kind(),
        MessageKind::Ready
    );
    let mut client = transport::spawn(reader, writer, request_id, Side::Client);
    let unavailable = Frame::empty(MessageKind::PasswordUnavailable, request_id)
        .expect("password unavailable frame");
    client
        .sender
        .send_control_flushed(unavailable)
        .await
        .expect("out-of-order message writes");
    let failure = tokio::time::timeout(Duration::from_secs(2), client.control.recv())
        .await
        .expect("failure arrives")
        .expect("failure frame exists");
    assert_eq!(failure.kind(), MessageKind::Failure);
    assert_eq!(
        failure
            .decode_metadata::<Failure>()
            .expect("typed failure")
            .reason,
        FailureReason::Protocol
    );
    drop(client);
    assert!(!child.wait().await.expect("helper exits").success());
}

#[cfg(unix)]
#[tokio::test]
async fn helper_rejects_stdin_data_and_eof_before_start() {
    for early in [MessageKind::StdinChunk, MessageKind::StdinEof] {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let sudo = fake_sudo(temporary.path());
        let mut child = spawn_helper();
        let mut writer = child.stdin.take().expect("helper stdin");
        let mut reader = child.stdout.take().expect("helper stdout");
        let request_id = if early == MessageKind::StdinChunk {
            65
        } else {
            66
        };
        let hello = Frame::metadata(
            MessageKind::Hello,
            request_id,
            &Hello {
                mode: Mode::Execute,
                sudo_path: sudo.to_string_lossy().into_owned(),
                auth_user: account_name(),
                setup_timeout_secs: 5,
                auth_timeout_secs: 5,
            },
        )
        .expect("hello frame");
        write_frame(&mut writer, &hello)
            .await
            .expect("hello writes");
        writer.flush().await.expect("hello flushes");
        assert_eq!(
            read_frame(&mut reader)
                .await
                .expect("ready parses")
                .expect("ready exists")
                .kind(),
            MessageKind::Ready
        );
        let early_frame = if early == MessageKind::StdinChunk {
            Frame::stream(early, request_id, b"premature".to_vec()).expect("stdin frame")
        } else {
            Frame::empty(early, request_id).expect("stdin EOF")
        };
        write_frame(&mut writer, &early_frame)
            .await
            .expect("early frame writes");
        writer.flush().await.expect("early frame flushes");

        let failure = tokio::time::timeout(Duration::from_secs(2), read_frame(&mut reader))
            .await
            .expect("failure arrives")
            .expect("failure parses")
            .expect("failure frame exists");
        assert_eq!(failure.kind(), MessageKind::Failure);
        assert_eq!(
            failure
                .decode_metadata::<Failure>()
                .expect("typed failure")
                .reason,
            FailureReason::Protocol
        );
        drop(writer);
        assert!(!child.wait().await.expect("helper exits").success());
    }
}

#[cfg(unix)]
#[tokio::test]
async fn helper_executes_once_and_delivers_both_output_eofs_before_result() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let sudo = fake_sudo(temporary.path());
    let mut child = spawn_helper();
    let mut writer = child.stdin.take().expect("helper stdin");
    let mut reader = child.stdout.take().expect("helper stdout");
    let request_id = 62;
    let auth_user = account_name();
    let hello = Frame::metadata(
        MessageKind::Hello,
        request_id,
        &Hello {
            mode: Mode::Execute,
            sudo_path: sudo.to_string_lossy().into_owned(),
            auth_user: auth_user.clone(),
            setup_timeout_secs: 5,
            auth_timeout_secs: 5,
        },
    )
    .expect("hello frame");
    write_frame(&mut writer, &hello)
        .await
        .expect("hello writes");
    writer.flush().await.expect("hello flushes");
    let ready = read_frame(&mut reader)
        .await
        .expect("ready parses")
        .expect("ready exists");
    assert_eq!(ready.kind(), MessageKind::Ready);

    let mut client = transport::spawn(reader, writer, request_id, Side::Client);
    let start = Frame::metadata(
        MessageKind::Start,
        request_id,
        &Start {
            executable: "/bin/sh".to_owned(),
            arguments: vec![
                "-c".to_owned(),
                "printf remote-out; printf remote-err >&2".to_owned(),
            ],
            cwd: None,
            run_as: "root".to_owned(),
            auth_user,
        },
    )
    .expect("start frame");
    client
        .sender
        .send_control_flushed(start)
        .await
        .expect("start writes");
    client
        .sender
        .send_eof(Stream::Stdin)
        .await
        .expect("stdin EOF writes");
    let mut stdout = client.take_stream(Stream::Stdout).expect("stdout stream");
    let mut stderr = client.take_stream(Stream::Stderr).expect("stderr stream");
    let mut stdout_bytes = Vec::new();
    let mut stderr_bytes = Vec::new();
    let mut stdout_eof = false;
    let mut stderr_eof = false;
    let result = loop {
        tokio::select! {
            biased;
            event = stdout.recv(), if !stdout_eof => match event.expect("stdout remains open") {
                StreamEvent::Chunk(bytes) => {
                    stdout_bytes.extend_from_slice(&bytes);
                    client.sender.acknowledge(Stream::Stdout, bytes.len() as u32).await.expect("stdout credit");
                }
                StreamEvent::Eof => stdout_eof = true,
            },
            event = stderr.recv(), if !stderr_eof => match event.expect("stderr remains open") {
                StreamEvent::Chunk(bytes) => {
                    stderr_bytes.extend_from_slice(&bytes);
                    client.sender.acknowledge(Stream::Stderr, bytes.len() as u32).await.expect("stderr credit");
                }
                StreamEvent::Eof => stderr_eof = true,
            },
            control = client.control.recv() => {
                let control = control.expect("terminal control");
                assert!(stdout_eof && stderr_eof, "result arrived before output EOF");
                assert_eq!(control.kind(), MessageKind::Result);
                break control.decode_metadata::<ExecutionResult>().expect("typed result");
            }
        }
    };
    assert_eq!(stdout_bytes, b"remote-out");
    assert_eq!(stderr_bytes, b"remote-err");
    assert_eq!(result.exit_code, Some(0));
    assert_eq!(result.signal, None);
    assert!(!result.password_delivered);
    drop(client);
    assert!(child.wait().await.expect("helper exits").success());
}

#[cfg(unix)]
#[tokio::test]
async fn early_target_exit_does_not_let_large_stdin_hide_the_result() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let sudo = fake_sudo(temporary.path());
    let mut child = spawn_helper();
    let mut writer = child.stdin.take().expect("helper stdin");
    let mut reader = child.stdout.take().expect("helper stdout");
    let request_id = 63;
    let auth_user = account_name();
    let hello = Frame::metadata(
        MessageKind::Hello,
        request_id,
        &Hello {
            mode: Mode::Execute,
            sudo_path: sudo.to_string_lossy().into_owned(),
            auth_user: auth_user.clone(),
            setup_timeout_secs: 5,
            auth_timeout_secs: 5,
        },
    )
    .expect("hello frame");
    write_frame(&mut writer, &hello)
        .await
        .expect("hello writes");
    writer.flush().await.expect("hello flushes");
    assert_eq!(
        read_frame(&mut reader)
            .await
            .expect("ready parses")
            .expect("ready exists")
            .kind(),
        MessageKind::Ready
    );
    let mut client = transport::spawn(reader, writer, request_id, Side::Client);
    let start = Frame::metadata(
        MessageKind::Start,
        request_id,
        &Start {
            executable: "/usr/bin/true".to_owned(),
            arguments: Vec::new(),
            cwd: None,
            run_as: "root".to_owned(),
            auth_user,
        },
    )
    .expect("start frame");
    client
        .sender
        .send_control_flushed(start)
        .await
        .expect("start writes");
    let input_sender = client.sender.clone();
    let input = tokio::spawn(async move {
        for _ in 0..40 {
            if input_sender
                .send_stream(Stream::Stdin, vec![b'x'; MAX_STREAM_CHUNK_SIZE])
                .await
                .is_err()
            {
                return;
            }
        }
        let _ = input_sender.send_eof(Stream::Stdin).await;
    });
    let mut stdout = client.take_stream(Stream::Stdout).expect("stdout stream");
    let mut stderr = client.take_stream(Stream::Stderr).expect("stderr stream");
    let mut stdout_eof = false;
    let mut stderr_eof = false;
    let result = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            tokio::select! {
                biased;
                event = stdout.recv(), if !stdout_eof => match event.expect("stdout remains open") {
                    StreamEvent::Chunk(bytes) => client.sender.acknowledge(Stream::Stdout, bytes.len() as u32).await.expect("stdout credit"),
                    StreamEvent::Eof => stdout_eof = true,
                },
                event = stderr.recv(), if !stderr_eof => match event.expect("stderr remains open") {
                    StreamEvent::Chunk(bytes) => client.sender.acknowledge(Stream::Stderr, bytes.len() as u32).await.expect("stderr credit"),
                    StreamEvent::Eof => stderr_eof = true,
                },
                control = client.control.recv() => {
                    let control = control.expect("result is not hidden by stdin close");
                    assert_eq!(control.kind(), MessageKind::Result);
                    break control.decode_metadata::<ExecutionResult>().expect("typed result");
                }
            }
        }
    })
    .await
    .expect("result arrives promptly");
    assert_eq!(result.exit_code, Some(0));
    // Draining the unread stdin must not produce credit updates after Result.
    let mut reader = client.reader_outcome();
    let closed = tokio::time::timeout(Duration::from_secs(15), reader.wait_for(Option::is_some))
        .await
        .expect("helper closes after its terminal frame")
        .map(|outcome| *outcome)
        .expect("reader reports its outcome");
    assert_eq!(closed, Some(Ok(())));
    input.abort();
    drop(client);
    assert!(child.wait().await.expect("helper exits").success());
}

#[cfg(unix)]
#[tokio::test]
async fn completed_process_waits_for_backpressured_output_and_services_cancel() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let sudo = fake_sudo(temporary.path());
    let marker = temporary.path().join("process-finished");
    let mut child = spawn_helper();
    let mut writer = child.stdin.take().expect("helper stdin");
    let mut reader = child.stdout.take().expect("helper stdout");
    let request_id = 67;
    let auth_user = account_name();
    let hello = Frame::metadata(
        MessageKind::Hello,
        request_id,
        &Hello {
            mode: Mode::Execute,
            sudo_path: sudo.to_string_lossy().into_owned(),
            auth_user: auth_user.clone(),
            setup_timeout_secs: 5,
            auth_timeout_secs: 5,
        },
    )
    .expect("hello frame");
    write_frame(&mut writer, &hello)
        .await
        .expect("hello writes");
    writer.flush().await.expect("hello flushes");
    assert_eq!(
        read_frame(&mut reader)
            .await
            .expect("ready parses")
            .expect("ready exists")
            .kind(),
        MessageKind::Ready
    );

    let mut client = transport::spawn(reader, writer, request_id, Side::Client);
    let start = Frame::metadata(
        MessageKind::Start,
        request_id,
        &Start {
            executable: "/bin/sh".to_owned(),
            arguments: vec![
                "-c".to_owned(),
                "dd if=/dev/zero bs=1024 count=129 2>/dev/null; : > \"$1\"".to_owned(),
                "sh".to_owned(),
                marker.to_string_lossy().into_owned(),
            ],
            cwd: None,
            run_as: "root".to_owned(),
            auth_user,
        },
    )
    .expect("start frame");
    client
        .sender
        .send_control_flushed(start)
        .await
        .expect("start writes");
    client
        .sender
        .send_eof(Stream::Stdin)
        .await
        .expect("stdin EOF writes");
    let mut stdout = client.take_stream(Stream::Stdout).expect("stdout stream");
    let mut stderr = client.take_stream(Stream::Stderr).expect("stderr stream");

    tokio::time::timeout(Duration::from_secs(2), async {
        while !marker.exists() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("target exits while output consumer is stalled");
    tokio::time::sleep(Duration::from_millis(5_200)).await;
    let cancel = Frame::metadata(
        MessageKind::Cancel,
        request_id,
        &Cancel {
            signal: libc::SIGTERM,
        },
    )
    .expect("cancel frame");
    client
        .sender
        .send_control_flushed(cancel)
        .await
        .expect("cancel is still serviced after process completion");

    let mut stdout_bytes = 0_usize;
    let mut stdout_eof = false;
    let mut stderr_eof = false;
    let result = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            tokio::select! {
                event = stdout.recv(), if !stdout_eof => match event.expect("stdout remains open") {
                    StreamEvent::Chunk(bytes) => {
                        stdout_bytes += bytes.len();
                        client.sender.acknowledge(Stream::Stdout, bytes.len() as u32).await.expect("stdout credit");
                    }
                    StreamEvent::Eof => stdout_eof = true,
                },
                event = stderr.recv(), if !stderr_eof => match event.expect("stderr remains open") {
                    StreamEvent::Chunk(bytes) => client.sender.acknowledge(Stream::Stderr, bytes.len() as u32).await.expect("stderr credit"),
                    StreamEvent::Eof => stderr_eof = true,
                },
                control = client.control.recv() => {
                    let control = control.expect("terminal frame");
                    assert!(stdout_eof && stderr_eof);
                    assert_eq!(control.kind(), MessageKind::Result);
                    break control.decode_metadata::<ExecutionResult>().expect("typed result");
                }
            }
        }
    })
    .await
    .expect("output and result complete after consumer resumes");
    assert_eq!(stdout_bytes, 129 * 1024);
    assert_eq!(result.exit_code, Some(0));
    drop(client);
    assert!(child.wait().await.expect("helper exits").success());
}

#[cfg(unix)]
#[tokio::test]
async fn cancel_after_exit_bounds_output_held_open_by_a_descendant() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let sudo = fake_sudo(temporary.path());
    let pid_file = temporary.path().join("descendant-pid");
    let mut child = spawn_helper();
    let mut writer = child.stdin.take().expect("helper stdin");
    let mut reader = child.stdout.take().expect("helper stdout");
    let request_id = 68;
    let auth_user = account_name();
    let hello = Frame::metadata(
        MessageKind::Hello,
        request_id,
        &Hello {
            mode: Mode::Execute,
            sudo_path: sudo.to_string_lossy().into_owned(),
            auth_user: auth_user.clone(),
            setup_timeout_secs: 5,
            auth_timeout_secs: 5,
        },
    )
    .expect("hello frame");
    write_frame(&mut writer, &hello)
        .await
        .expect("hello writes");
    writer.flush().await.expect("hello flushes");
    assert_eq!(
        read_frame(&mut reader)
            .await
            .expect("ready parses")
            .expect("ready exists")
            .kind(),
        MessageKind::Ready
    );
    let mut client = transport::spawn(reader, writer, request_id, Side::Client);
    let start = Frame::metadata(
        MessageKind::Start,
        request_id,
        &Start {
            executable: "/bin/sh".to_owned(),
            arguments: vec![
                "-c".to_owned(),
                "/bin/sleep 30 & echo $! > \"$1\"".to_owned(),
                "sh".to_owned(),
                pid_file.to_string_lossy().into_owned(),
            ],
            cwd: None,
            run_as: "root".to_owned(),
            auth_user,
        },
    )
    .expect("start frame");
    client
        .sender
        .send_control_flushed(start)
        .await
        .expect("start writes");
    client
        .sender
        .send_eof(Stream::Stdin)
        .await
        .expect("stdin EOF writes");
    let descendant: i32 = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Ok(text) = fs::read_to_string(&pid_file) {
                if let Ok(pid) = text.trim().parse() {
                    return pid;
                }
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("target starts its descendant");
    tokio::time::sleep(Duration::from_millis(300)).await;
    let cancel = Frame::metadata(
        MessageKind::Cancel,
        request_id,
        &Cancel {
            signal: libc::SIGTERM,
        },
    )
    .expect("cancel frame");
    client
        .sender
        .send_control_flushed(cancel)
        .await
        .expect("cancel writes");

    let terminal = tokio::time::timeout(Duration::from_secs(10), client.control.recv())
        .await
        .expect("cancel bounds the wait for held output")
        .expect("terminal frame");
    unsafe {
        libc::kill(descendant, libc::SIGKILL);
    }
    assert_eq!(terminal.kind(), MessageKind::Failure);
    assert_eq!(
        terminal
            .decode_metadata::<Failure>()
            .expect("typed failure")
            .reason,
        FailureReason::CompletionUnknown
    );
    drop(client);
    let _ = child.wait().await;
}

#[cfg(unix)]
#[tokio::test]
async fn cancelling_a_running_passwordless_target_returns_its_signal_result() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let sudo = fake_sudo(temporary.path());
    let pid_file = temporary.path().join("target-pid");
    let mut child = spawn_helper();
    let mut writer = child.stdin.take().expect("helper stdin");
    let mut reader = child.stdout.take().expect("helper stdout");
    let request_id = 69;
    let auth_user = account_name();
    let hello = Frame::metadata(
        MessageKind::Hello,
        request_id,
        &Hello {
            mode: Mode::Execute,
            sudo_path: sudo.to_string_lossy().into_owned(),
            auth_user: auth_user.clone(),
            setup_timeout_secs: 5,
            auth_timeout_secs: 5,
        },
    )
    .expect("hello frame");
    write_frame(&mut writer, &hello)
        .await
        .expect("hello writes");
    writer.flush().await.expect("hello flushes");
    assert_eq!(
        read_frame(&mut reader)
            .await
            .expect("ready parses")
            .expect("ready exists")
            .kind(),
        MessageKind::Ready
    );
    let mut client = transport::spawn(reader, writer, request_id, Side::Client);
    let start = Frame::metadata(
        MessageKind::Start,
        request_id,
        &Start {
            executable: "/bin/sh".to_owned(),
            arguments: vec![
                "-c".to_owned(),
                "echo $$ > \"$1\"; exec /bin/sleep 30".to_owned(),
                "sh".to_owned(),
                pid_file.to_string_lossy().into_owned(),
            ],
            cwd: None,
            run_as: "root".to_owned(),
            auth_user,
        },
    )
    .expect("start frame");
    client
        .sender
        .send_control_flushed(start)
        .await
        .expect("start writes");
    client
        .sender
        .send_eof(Stream::Stdin)
        .await
        .expect("stdin EOF writes");
    let mut stdout = client.take_stream(Stream::Stdout).expect("stdout stream");
    let mut stderr = client.take_stream(Stream::Stderr).expect("stderr stream");
    tokio::time::timeout(Duration::from_secs(5), async {
        while fs::read_to_string(&pid_file).map_or(true, |text| text.trim().is_empty()) {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("target starts");
    let cancel = Frame::metadata(
        MessageKind::Cancel,
        request_id,
        &Cancel {
            signal: libc::SIGTERM,
        },
    )
    .expect("cancel frame");
    client
        .sender
        .send_control_flushed(cancel)
        .await
        .expect("cancel writes");

    let (mut stdout_eof, mut stderr_eof) = (false, false);
    let terminal = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            tokio::select! {
                event = stdout.recv(), if !stdout_eof => if matches!(event, Some(StreamEvent::Eof) | None) { stdout_eof = true },
                event = stderr.recv(), if !stderr_eof => if matches!(event, Some(StreamEvent::Eof) | None) { stderr_eof = true },
                control = client.control.recv() => break control.expect("terminal frame"),
            }
        }
    })
    .await
    .expect("cancelled target reports a terminal frame");
    assert_eq!(terminal.kind(), MessageKind::Result);
    let result = terminal
        .decode_metadata::<ExecutionResult>()
        .expect("typed result");
    assert_eq!(result.signal, Some(libc::SIGTERM));
    assert!(!result.password_delivered);
    drop(client);
    let _ = child.wait().await;
}

#[tokio::test]
async fn empty_stream_chunks_are_ignored_without_consuming_credit() {
    let (client, mut helper) = session_pair(46).await;
    client
        .sender
        .send_stream(Stream::Stdin, Vec::new())
        .await
        .expect("empty chunk writes");
    client
        .sender
        .send_stream(Stream::Stdin, b"data".to_vec())
        .await
        .expect("data chunk writes");
    let mut stdin = helper.take_stream(Stream::Stdin).expect("stdin receiver");
    assert!(matches!(
        stdin.recv().await,
        Some(StreamEvent::Chunk(bytes)) if bytes.as_slice() == b"data"
    ));
    helper
        .sender
        .acknowledge(Stream::Stdin, 4)
        .await
        .expect("credit for the delivered bytes is valid");
}

#[cfg(unix)]
fn spawn_helper() -> Child {
    let mut command = Command::new(env!("CARGO_BIN_EXE_agentenv-sudo-helper"));
    command
        .arg("--serve")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    command.spawn().expect("helper starts")
}

#[cfg(unix)]
fn fake_sudo(directory: &std::path::Path) -> std::path::PathBuf {
    let path = directory.join("sudo");
    fs::write(
        &path,
        b"#!/bin/sh\nwhile [ \"$1\" != \"--\" ]; do shift; done\nshift\nexec \"$@\"\n",
    )
    .expect("fake sudo writes");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).expect("permissions set");
    path
}

#[cfg(unix)]
fn account_name() -> String {
    unsafe {
        let record = libc::getpwuid(libc::getuid());
        assert!(!record.is_null(), "account record exists");
        CStr::from_ptr((*record).pw_name)
            .to_str()
            .expect("account name is UTF-8")
            .to_owned()
    }
}
