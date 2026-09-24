//! Minimal synchronous CDP connection: one browser WebSocket, flattened
//! sessions, request/response matching, and an event queue.

use std::collections::VecDeque;
use std::net::TcpStream;
use std::time::Duration;

use serde_json::{json, Value};
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{Message, WebSocket};

/// A protocol-level failure. `timed_out` is set when the deadline expired
/// while waiting for the browser.
#[derive(Debug)]
pub struct CdpError {
    pub message: String,
    pub timed_out: bool,
}

impl CdpError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            timed_out: false,
        }
    }

    fn timeout() -> Self {
        Self {
            message: "the operation deadline expired while waiting for the browser".to_owned(),
            timed_out: true,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Event {
    pub method: String,
    pub params: Value,
}

pub struct Connection {
    socket: WebSocket<MaybeTlsStream<TcpStream>>,
    next_id: u64,
    pub events: VecDeque<Event>,
}

impl Connection {
    /// Fetches `/json/version` and opens the browser WebSocket. Returns the
    /// connection and the version document.
    pub fn open(endpoint: &str, remaining: Duration) -> Result<(Self, Value), CdpError> {
        let version_url = format!("{}/json/version", endpoint.trim_end_matches('/'));
        let agent = ureq::Agent::config_builder()
            .timeout_global(Some(remaining))
            .build()
            .new_agent();
        let body = agent
            .get(&version_url)
            .call()
            .map_err(|error| CdpError::new(format!("cannot read {version_url}: {error}")))?
            .body_mut()
            .read_to_string()
            .map_err(|error| CdpError::new(format!("cannot read {version_url}: {error}")))?;
        let version: Value = serde_json::from_str(&body)
            .map_err(|error| CdpError::new(format!("invalid /json/version document: {error}")))?;
        let ws_url = version
            .get("webSocketDebuggerUrl")
            .and_then(Value::as_str)
            .ok_or_else(|| CdpError::new("/json/version has no webSocketDebuggerUrl"))?;
        let (socket, _) = tungstenite::connect(ws_url).map_err(|error| {
            CdpError::new(format!("cannot open the browser WebSocket: {error}"))
        })?;
        let mut connection = Self {
            socket,
            next_id: 0,
            events: VecDeque::new(),
        };
        connection.set_read_timeout(remaining)?;
        Ok((connection, version))
    }

    fn set_read_timeout(&mut self, remaining: Duration) -> Result<(), CdpError> {
        let timeout = if remaining.is_zero() {
            Duration::from_millis(1)
        } else {
            remaining
        };
        match self.socket.get_mut() {
            MaybeTlsStream::Plain(stream) => stream
                .set_read_timeout(Some(timeout))
                .map_err(|error| CdpError::new(format!("cannot set the socket timeout: {error}"))),
            _ => Err(CdpError::new(
                "unexpected TLS stream for a loopback endpoint",
            )),
        }
    }

    /// Sends one command and waits for its result within `remaining`.
    /// Events received meanwhile are queued.
    pub fn send(
        &mut self,
        session_id: Option<&str>,
        method: &str,
        params: Value,
        remaining: Duration,
    ) -> Result<Value, CdpError> {
        if remaining.is_zero() {
            return Err(CdpError::timeout());
        }
        self.next_id += 1;
        let id = self.next_id;
        let mut message = json!({ "id": id, "method": method, "params": params });
        if let Some(session_id) = session_id {
            message["sessionId"] = Value::String(session_id.to_owned());
        }
        let text = serde_json::to_string(&message)
            .map_err(|error| CdpError::new(format!("cannot encode {method}: {error}")))?;
        self.socket
            .send(Message::Text(text.into()))
            .map_err(|error| CdpError::new(format!("cannot send {method}: {error}")))?;
        self.set_read_timeout(remaining)?;
        loop {
            let incoming = match self.socket.read() {
                Ok(message) => message,
                Err(tungstenite::Error::Io(error))
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) =>
                {
                    return Err(CdpError::timeout());
                }
                Err(error) => {
                    return Err(CdpError::new(format!(
                        "the browser connection failed during {method}: {error}"
                    )))
                }
            };
            let text = match incoming {
                Message::Text(text) => text,
                Message::Close(_) => {
                    return Err(CdpError::new(format!(
                        "the browser closed the connection during {method}"
                    )))
                }
                _ => continue,
            };
            let document: Value = match serde_json::from_str(text.as_str()) {
                Ok(document) => document,
                Err(_) => continue,
            };
            if document.get("id").and_then(Value::as_u64) == Some(id) {
                if let Some(error) = document.get("error") {
                    let description = error
                        .get("message")
                        .and_then(Value::as_str)
                        .unwrap_or("unknown protocol error");
                    return Err(CdpError::new(format!("{method} failed: {description}")));
                }
                return Ok(document.get("result").cloned().unwrap_or(Value::Null));
            }
            if let Some(event_method) = document.get("method").and_then(Value::as_str) {
                self.events.push_back(Event {
                    method: event_method.to_owned(),
                    params: document.get("params").cloned().unwrap_or(Value::Null),
                });
            }
        }
    }

    /// Drains queued events matching `method`.
    pub fn take_events(&mut self, method: &str) -> Vec<Event> {
        let mut taken = Vec::new();
        let mut kept = VecDeque::new();
        while let Some(event) = self.events.pop_front() {
            if event.method == method {
                taken.push(event);
            } else {
                kept.push_back(event);
            }
        }
        self.events = kept;
        taken
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        let _ = self.socket.close(None);
        let _ = self.socket.flush();
    }
}
