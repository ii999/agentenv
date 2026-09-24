//! Bounded, independently scheduled transport for one remote sudo session.

use std::collections::VecDeque;
use std::fmt;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex};

use thiserror::Error;
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt};
use tokio::sync::{mpsc, oneshot, watch, Notify};
use tokio::task::JoinHandle;
use zeroize::Zeroizing;

use super::protocol::{
    read_frame, write_frame, Credit, ExecutionResult, Failure, Frame, MessageKind, Stream,
    WindowUpdate, INITIAL_STREAM_CREDIT, MAX_STREAM_CHUNK_SIZE,
};

const CONTROL_QUEUE_CAPACITY: usize = 4;
const STREAM_CHUNK_CAPACITY: usize = 4;
const STREAM_QUEUE_CAPACITY: usize = STREAM_CHUNK_CAPACITY + 1;

const AWAITING_START: u8 = 0;
const VALIDATING_START: u8 = 1;
const ACTIVE: u8 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Client,
    Helper,
}

#[derive(Debug, Clone, Copy, Error, PartialEq, Eq)]
pub enum TransportError {
    #[error("remote transport closed")]
    Closed,
    #[error("remote protocol violation")]
    Protocol,
    #[error("remote stream flow-control violation")]
    FlowControl,
    #[error("remote transport queue is full")]
    QueueFull,
    #[error("message is invalid for this transport direction")]
    Direction,
    #[error("message has an inconsistent request identifier")]
    RequestId,
}

pub enum StreamEvent {
    Chunk(Zeroizing<Vec<u8>>),
    Eof,
}

impl fmt::Debug for StreamEvent {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Chunk(bytes) => formatter
                .debug_struct("Chunk")
                .field("length", &bytes.len())
                .finish(),
            Self::Eof => formatter.write_str("Eof"),
        }
    }
}

struct StreamQueueState {
    events: VecDeque<StreamEvent>,
    bytes: usize,
    closed: bool,
    eof_consumed: bool,
}

struct StreamQueue {
    state: Mutex<StreamQueueState>,
    changed: Notify,
    eof_consumed: Notify,
}

impl StreamQueue {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(StreamQueueState {
                events: VecDeque::with_capacity(STREAM_QUEUE_CAPACITY),
                bytes: 0,
                closed: false,
                eof_consumed: false,
            }),
            changed: Notify::new(),
            eof_consumed: Notify::new(),
        })
    }

    fn push_chunk(&self, bytes: Zeroizing<Vec<u8>>) -> Result<(), TransportError> {
        let length = bytes.len();
        let mut state = self.state.lock().map_err(|_| TransportError::Closed)?;
        if state.closed {
            return Err(TransportError::Closed);
        }
        state.bytes = state
            .bytes
            .checked_add(length)
            .filter(|queued| *queued <= INITIAL_STREAM_CREDIT)
            .ok_or(TransportError::QueueFull)?;
        if let Some(StreamEvent::Chunk(tail)) = state.events.back_mut() {
            let available = MAX_STREAM_CHUNK_SIZE.saturating_sub(tail.len());
            if length <= available {
                tail.extend_from_slice(&bytes);
                drop(state);
                self.changed.notify_one();
                return Ok(());
            }
        }
        state.events.push_back(StreamEvent::Chunk(bytes));
        drop(state);
        self.changed.notify_one();
        Ok(())
    }

    fn push_eof(&self) -> Result<(), TransportError> {
        let mut state = self.state.lock().map_err(|_| TransportError::Closed)?;
        if state.closed {
            return Err(TransportError::Closed);
        }
        state.events.push_back(StreamEvent::Eof);
        drop(state);
        self.changed.notify_one();
        Ok(())
    }

    fn close(&self) {
        if let Ok(mut state) = self.state.lock() {
            state.closed = true;
        }
        self.changed.notify_waiters();
        self.eof_consumed.notify_waiters();
    }

    async fn wait_eof_consumed(&self) -> Result<(), TransportError> {
        loop {
            let notified = self.eof_consumed.notified();
            {
                let state = self.state.lock().map_err(|_| TransportError::Closed)?;
                if state.eof_consumed {
                    return Ok(());
                }
                if state.closed {
                    return Err(TransportError::Closed);
                }
            }
            notified.await;
        }
    }
}

/// Byte-bounded receive side of one protocol stream.
pub struct StreamReceiver {
    queue: Arc<StreamQueue>,
}

impl StreamReceiver {
    pub async fn recv(&mut self) -> Option<StreamEvent> {
        loop {
            let notified = self.queue.changed.notified();
            {
                let mut state = self.queue.state.lock().ok()?;
                if let Some(event) = state.events.pop_front() {
                    match &event {
                        StreamEvent::Chunk(bytes) => state.bytes -= bytes.len(),
                        StreamEvent::Eof => {
                            state.eof_consumed = true;
                            self.queue.eof_consumed.notify_waiters();
                        }
                    }
                    return Some(event);
                }
                if state.closed {
                    return None;
                }
            }
            notified.await;
        }
    }
}

struct Activation {
    state: AtomicU8,
    changed: Notify,
}

struct ReaderContext {
    side: Side,
    request_id: u64,
    control: mpsc::Sender<Frame>,
    streams: [Arc<StreamQueue>; 3],
    incoming_flow: [Arc<Mutex<Credit>>; 3],
    outgoing_flow: [Arc<Flow>; 3],
    activation: Arc<Activation>,
}

impl Activation {
    fn new(side: Side) -> Arc<Self> {
        Arc::new(Self {
            state: AtomicU8::new(if side == Side::Client {
                ACTIVE
            } else {
                AWAITING_START
            }),
            changed: Notify::new(),
        })
    }

    async fn wait_active(&self) -> Result<(), TransportError> {
        loop {
            let notified = self.changed.notified();
            match self.state.load(Ordering::Acquire) {
                ACTIVE => return Ok(()),
                VALIDATING_START => notified.await,
                _ => return Err(TransportError::Protocol),
            }
        }
    }
}

struct Outgoing {
    frame: Frame,
    flushed: Option<oneshot::Sender<Result<(), TransportError>>>,
}

struct Flow {
    credit: Mutex<Credit>,
    changed: Notify,
}

impl Flow {
    fn new() -> Self {
        Self {
            credit: Mutex::new(Credit::new()),
            changed: Notify::new(),
        }
    }
}

#[derive(Clone)]
pub struct TransportSender {
    side: Side,
    request_id: u64,
    control: mpsc::Sender<Outgoing>,
    streams: [mpsc::Sender<Outgoing>; 3],
    outgoing_flow: [Arc<Flow>; 3],
    incoming_flow: [Arc<Mutex<Credit>>; 3],
}

impl TransportSender {
    pub async fn send_control(&self, frame: Frame) -> Result<(), TransportError> {
        self.validate_outgoing_control(&frame, false)?;
        self.control
            .send(Outgoing {
                frame,
                flushed: None,
            })
            .await
            .map_err(|_| TransportError::Closed)
    }

    pub async fn send_control_flushed(&self, frame: Frame) -> Result<(), TransportError> {
        self.validate_outgoing_control(&frame, false)?;
        let (flushed, received) = oneshot::channel();
        self.control
            .send(Outgoing {
                frame,
                flushed: Some(flushed),
            })
            .await
            .map_err(|_| TransportError::Closed)?;
        received.await.map_err(|_| TransportError::Closed)?
    }

    pub async fn send_terminal(&self, frame: Frame) -> Result<(), TransportError> {
        self.validate_outgoing_control(&frame, true)?;
        let (flushed, received) = oneshot::channel();
        self.control
            .send(Outgoing {
                frame,
                flushed: Some(flushed),
            })
            .await
            .map_err(|_| TransportError::Closed)?;
        received.await.map_err(|_| TransportError::Closed)?
    }

    pub async fn send_stream(&self, stream: Stream, bytes: Vec<u8>) -> Result<(), TransportError> {
        if bytes.len() > MAX_STREAM_CHUNK_SIZE {
            return Err(TransportError::FlowControl);
        }
        self.validate_outgoing_stream(stream)?;
        self.consume_credit(stream, bytes.len()).await?;
        let kind = chunk_kind(stream);
        let frame =
            Frame::stream(kind, self.request_id, bytes).map_err(|_| TransportError::Protocol)?;
        self.streams[stream_index(stream)]
            .send(Outgoing {
                frame,
                flushed: None,
            })
            .await
            .map_err(|_| TransportError::Closed)
    }

    /// Sends stream EOF after all earlier chunks on that stream and returns
    /// only after the EOF frame has been written and flushed.
    pub async fn send_eof(&self, stream: Stream) -> Result<(), TransportError> {
        self.validate_outgoing_stream(stream)?;
        let frame = Frame::empty(eof_kind(stream), self.request_id)
            .map_err(|_| TransportError::Protocol)?;
        let (flushed, received) = oneshot::channel();
        self.streams[stream_index(stream)]
            .send(Outgoing {
                frame,
                flushed: Some(flushed),
            })
            .await
            .map_err(|_| TransportError::Closed)?;
        received.await.map_err(|_| TransportError::Closed)?
    }

    /// Returns credit only after the caller has consumed the corresponding
    /// bytes from its local stream sink.
    pub async fn acknowledge(&self, stream: Stream, bytes: u32) -> Result<(), TransportError> {
        if bytes == 0 {
            return Err(TransportError::FlowControl);
        }
        self.validate_incoming_stream(stream)?;
        {
            let mut credit = self.incoming_flow[stream_index(stream)]
                .lock()
                .map_err(|_| TransportError::Closed)?;
            credit
                .grant(bytes)
                .map_err(|_| TransportError::FlowControl)?;
        }
        let frame = Frame::metadata(
            MessageKind::WindowUpdate,
            self.request_id,
            &WindowUpdate { stream, bytes },
        )
        .map_err(|_| TransportError::Protocol)?;
        self.control
            .send(Outgoing {
                frame,
                flushed: None,
            })
            .await
            .map_err(|_| TransportError::Closed)
    }

    async fn consume_credit(&self, stream: Stream, bytes: usize) -> Result<(), TransportError> {
        let flow = &self.outgoing_flow[stream_index(stream)];
        loop {
            let changed = flow.changed.notified();
            {
                let mut credit = flow.credit.lock().map_err(|_| TransportError::Closed)?;
                match credit.consume(bytes) {
                    Ok(()) => return Ok(()),
                    Err(super::protocol::ProtocolError::InsufficientCredit { .. }) => {}
                    Err(_) => return Err(TransportError::FlowControl),
                }
            }
            changed.await;
        }
    }

    fn validate_outgoing_control(
        &self,
        frame: &Frame,
        terminal: bool,
    ) -> Result<(), TransportError> {
        if frame.request_id() != self.request_id {
            return Err(TransportError::RequestId);
        }
        let allowed = matches!(
            (self.side, frame.kind(), terminal),
            (Side::Client, MessageKind::Hello | MessageKind::Start, false)
                | (
                    Side::Client,
                    MessageKind::PasswordResponse
                        | MessageKind::PasswordUnavailable
                        | MessageKind::Cancel,
                    false,
                )
                | (
                    Side::Helper,
                    MessageKind::Ready | MessageKind::PasswordRequest,
                    false
                )
                | (
                    Side::Helper,
                    MessageKind::Result | MessageKind::Failure,
                    true
                )
        );
        if allowed {
            Ok(())
        } else {
            Err(TransportError::Direction)
        }
    }

    fn validate_outgoing_stream(&self, stream: Stream) -> Result<(), TransportError> {
        match (self.side, stream) {
            (Side::Client, Stream::Stdin) | (Side::Helper, Stream::Stdout | Stream::Stderr) => {
                Ok(())
            }
            _ => Err(TransportError::Direction),
        }
    }

    fn validate_incoming_stream(&self, stream: Stream) -> Result<(), TransportError> {
        match (self.side, stream) {
            (Side::Client, Stream::Stdout | Stream::Stderr) | (Side::Helper, Stream::Stdin) => {
                Ok(())
            }
            _ => Err(TransportError::Direction),
        }
    }
}

pub struct SessionTransport {
    pub sender: TransportSender,
    pub control: mpsc::Receiver<Frame>,
    streams: [Option<StreamReceiver>; 3],
    stream_queues: [Arc<StreamQueue>; 3],
    activation: Arc<Activation>,
    failure: watch::Receiver<Option<TransportError>>,
    reader: watch::Receiver<Option<Result<(), TransportError>>>,
    tasks: Vec<JoinHandle<()>>,
}

impl SessionTransport {
    pub fn take_stream(&mut self, stream: Stream) -> Result<StreamReceiver, TransportError> {
        self.streams[stream_index(stream)]
            .take()
            .ok_or(TransportError::Closed)
    }

    pub fn failure(&self) -> watch::Receiver<Option<TransportError>> {
        self.failure.clone()
    }

    /// Reports how the receive side ended. `Ok` means the peer closed cleanly
    /// after exactly one terminal frame; any later frame is an error.
    pub fn reader_outcome(&self) -> watch::Receiver<Option<Result<(), TransportError>>> {
        self.reader.clone()
    }

    /// Allows stream and window frames after the helper validates Start.
    pub fn activate_streams(&self) -> Result<(), TransportError> {
        self.activation
            .state
            .compare_exchange(
                VALIDATING_START,
                ACTIVE,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .map_err(|_| TransportError::Protocol)?;
        self.activation.changed.notify_waiters();
        Ok(())
    }
}

impl Drop for SessionTransport {
    fn drop(&mut self) {
        for queue in &self.stream_queues {
            queue.close();
        }
        for task in &self.tasks {
            task.abort();
        }
    }
}

pub fn spawn<R, W>(reader: R, writer: W, request_id: u64, side: Side) -> SessionTransport
where
    R: AsyncRead + Unpin + Send + 'static,
    W: AsyncWrite + Unpin + Send + 'static,
{
    let (control_tx, control_rx) = mpsc::channel(CONTROL_QUEUE_CAPACITY);
    let incoming_queues: [Arc<StreamQueue>; 3] = std::array::from_fn(|_| StreamQueue::new());

    let (out_control_tx, out_control_rx) = mpsc::channel(CONTROL_QUEUE_CAPACITY);
    let (out_stdin_tx, out_stdin_rx) = mpsc::channel(STREAM_QUEUE_CAPACITY);
    let (out_stdout_tx, out_stdout_rx) = mpsc::channel(STREAM_QUEUE_CAPACITY);
    let (out_stderr_tx, out_stderr_rx) = mpsc::channel(STREAM_QUEUE_CAPACITY);
    let outgoing_senders = [out_stdin_tx, out_stdout_tx, out_stderr_tx];

    let outgoing_flow: [Arc<Flow>; 3] = std::array::from_fn(|_| Arc::new(Flow::new()));
    let incoming_flow: [Arc<Mutex<Credit>>; 3] =
        std::array::from_fn(|_| Arc::new(Mutex::new(Credit::new())));
    let sender = TransportSender {
        side,
        request_id,
        control: out_control_tx,
        streams: outgoing_senders,
        outgoing_flow: outgoing_flow.clone(),
        incoming_flow: incoming_flow.clone(),
    };
    let activation = Activation::new(side);

    let (failure_tx, failure_rx) = watch::channel(None);
    let (reader_tx, reader_rx) = watch::channel(None);
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let reader_failure = failure_tx.clone();
    let reader_queues = incoming_queues.clone();
    let reader_activation = activation.clone();
    let reader_task = tokio::spawn(async move {
        // Keep the control channel open until the failure is recorded, so a
        // consumer that observes closure can also observe why it closed.
        let control_open = control_tx.clone();
        let result = reader_loop(
            reader,
            ReaderContext {
                side,
                request_id,
                control: control_tx,
                streams: reader_queues.clone(),
                incoming_flow,
                outgoing_flow,
                activation: reader_activation,
            },
        )
        .await;
        if let Err(error) = result {
            let _ = reader_failure.send(Some(error));
        }
        for queue in &reader_queues {
            queue.close();
        }
        drop(control_open);
        let _ = reader_tx.send(Some(result));
    });

    let writer_failure = failure_tx;
    let writer_shutdown = shutdown_tx;
    let writer_task = tokio::spawn(async move {
        let result = writer_loop(
            writer,
            out_control_rx,
            [out_stdin_rx, out_stdout_rx, out_stderr_rx],
            shutdown_rx,
        )
        .await;
        if let Err(error) = result {
            let _ = writer_failure.send(Some(error));
            let _ = writer_shutdown.send(true);
        }
    });

    SessionTransport {
        sender,
        control: control_rx,
        streams: incoming_queues
            .clone()
            .map(|queue| Some(StreamReceiver { queue })),
        stream_queues: incoming_queues,
        activation,
        failure: failure_rx,
        reader: reader_rx,
        tasks: vec![reader_task, writer_task],
    }
}

async fn reader_loop<R>(mut reader: R, context: ReaderContext) -> Result<(), TransportError>
where
    R: AsyncRead + Unpin,
{
    let ReaderContext {
        side,
        request_id,
        control,
        streams,
        incoming_flow,
        outgoing_flow,
        activation,
    } = context;
    let mut eof = [false; 3];
    let mut terminal = false;
    loop {
        let frame = match read_frame(&mut reader)
            .await
            .map_err(|_| TransportError::Protocol)?
        {
            Some(frame) => frame,
            None if terminal => return Ok(()),
            None => return Err(TransportError::Closed),
        };
        if frame.request_id() != request_id {
            return Err(TransportError::RequestId);
        }
        if terminal {
            return Err(TransportError::Protocol);
        }
        let kind = frame.kind();
        if kind == MessageKind::WindowUpdate {
            activation.wait_active().await?;
            let update: WindowUpdate = frame
                .decode_metadata()
                .map_err(|_| TransportError::Protocol)?;
            if update.bytes == 0 || !is_outgoing_stream(side, update.stream) {
                return Err(TransportError::FlowControl);
            }
            let flow = &outgoing_flow[stream_index(update.stream)];
            {
                let mut credit = flow.credit.lock().map_err(|_| TransportError::Closed)?;
                credit
                    .grant(update.bytes)
                    .map_err(|_| TransportError::FlowControl)?;
            }
            flow.changed.notify_waiters();
            continue;
        }
        if let Some((stream, is_eof)) = stream_kind(kind) {
            if !is_incoming_stream(side, stream) {
                return Err(TransportError::Direction);
            }
            let index = stream_index(stream);
            if eof[index] {
                return Err(TransportError::Protocol);
            }
            activation.wait_active().await?;
            if is_eof {
                eof[index] = true;
                streams[index].push_eof()?;
            } else {
                let length = frame.payload_len();
                if length == 0 {
                    // An empty chunk carries no data and consumes no credit.
                    continue;
                }
                incoming_flow[index]
                    .lock()
                    .map_err(|_| TransportError::Closed)?
                    .consume(length)
                    .map_err(|_| TransportError::FlowControl)?;
                streams[index]
                    .push_chunk(frame.into_stream().map_err(|_| TransportError::Protocol)?)?;
            }
            continue;
        }
        if !is_incoming_control(side, kind) {
            return Err(TransportError::Direction);
        }
        if side == Side::Helper && kind == MessageKind::Start {
            activation
                .state
                .compare_exchange(
                    AWAITING_START,
                    VALIDATING_START,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                )
                .map_err(|_| TransportError::Protocol)?;
        }
        if side == Side::Client && kind == MessageKind::Result {
            frame
                .decode_metadata::<ExecutionResult>()
                .map_err(|_| TransportError::Protocol)?;
            if !eof[stream_index(Stream::Stdout)] || !eof[stream_index(Stream::Stderr)] {
                return Err(TransportError::Protocol);
            }
            streams[stream_index(Stream::Stdout)]
                .wait_eof_consumed()
                .await?;
            streams[stream_index(Stream::Stderr)]
                .wait_eof_consumed()
                .await?;
            terminal = true;
        } else if side == Side::Client && kind == MessageKind::Failure {
            frame
                .decode_metadata::<Failure>()
                .map_err(|_| TransportError::Protocol)?;
            terminal = true;
        }
        control.try_send(frame).map_err(|error| match error {
            mpsc::error::TrySendError::Full(_) => TransportError::QueueFull,
            mpsc::error::TrySendError::Closed(_) => TransportError::Closed,
        })?;
        if side == Side::Helper && kind == MessageKind::Start {
            activation.wait_active().await?;
        }
    }
}

async fn writer_loop<W>(
    mut writer: W,
    mut control: mpsc::Receiver<Outgoing>,
    streams: [mpsc::Receiver<Outgoing>; 3],
    mut shutdown: watch::Receiver<bool>,
) -> Result<(), TransportError>
where
    W: AsyncWrite + Unpin,
{
    let [mut stdin, mut stdout, mut stderr] = streams;
    loop {
        let outgoing = tokio::select! {
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    return Ok(());
                }
                continue;
            }
            item = control.recv() => item,
            item = stdin.recv() => item,
            item = stdout.recv() => item,
            item = stderr.recv() => item,
        }
        .ok_or(TransportError::Closed)?;
        if outgoing
            .flushed
            .as_ref()
            .is_some_and(oneshot::Sender::is_closed)
        {
            continue;
        }
        let result = async {
            write_frame(&mut writer, &outgoing.frame)
                .await
                .map_err(|_| TransportError::Closed)?;
            writer.flush().await.map_err(|_| TransportError::Closed)
        }
        .await;
        let terminal = matches!(
            outgoing.frame.kind(),
            MessageKind::Result | MessageKind::Failure
        );
        if let Some(flushed) = outgoing.flushed {
            let _ = flushed.send(result);
        }
        result?;
        if terminal {
            // Seal the session: later sends observe a closed transport
            // instead of writing frames after the terminal.
            return Ok(());
        }
    }
}

fn stream_index(stream: Stream) -> usize {
    match stream {
        Stream::Stdin => 0,
        Stream::Stdout => 1,
        Stream::Stderr => 2,
    }
}

fn chunk_kind(stream: Stream) -> MessageKind {
    match stream {
        Stream::Stdin => MessageKind::StdinChunk,
        Stream::Stdout => MessageKind::StdoutChunk,
        Stream::Stderr => MessageKind::StderrChunk,
    }
}

fn eof_kind(stream: Stream) -> MessageKind {
    match stream {
        Stream::Stdin => MessageKind::StdinEof,
        Stream::Stdout => MessageKind::StdoutEof,
        Stream::Stderr => MessageKind::StderrEof,
    }
}

fn stream_kind(kind: MessageKind) -> Option<(Stream, bool)> {
    match kind {
        MessageKind::StdinChunk => Some((Stream::Stdin, false)),
        MessageKind::StdinEof => Some((Stream::Stdin, true)),
        MessageKind::StdoutChunk => Some((Stream::Stdout, false)),
        MessageKind::StdoutEof => Some((Stream::Stdout, true)),
        MessageKind::StderrChunk => Some((Stream::Stderr, false)),
        MessageKind::StderrEof => Some((Stream::Stderr, true)),
        _ => None,
    }
}

fn is_outgoing_stream(side: Side, stream: Stream) -> bool {
    matches!(
        (side, stream),
        (Side::Client, Stream::Stdin) | (Side::Helper, Stream::Stdout | Stream::Stderr)
    )
}

fn is_incoming_stream(side: Side, stream: Stream) -> bool {
    matches!(
        (side, stream),
        (Side::Client, Stream::Stdout | Stream::Stderr) | (Side::Helper, Stream::Stdin)
    )
}

fn is_incoming_control(side: Side, kind: MessageKind) -> bool {
    match side {
        Side::Client => matches!(
            kind,
            MessageKind::Ready
                | MessageKind::PasswordRequest
                | MessageKind::Result
                | MessageKind::Failure
        ),
        Side::Helper => matches!(
            kind,
            MessageKind::Hello
                | MessageKind::Start
                | MessageKind::PasswordResponse
                | MessageKind::PasswordUnavailable
                | MessageKind::Cancel
        ),
    }
}
