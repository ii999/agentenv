//! Asynchronous Chrome DevTools Protocol connection: one browser WebSocket,
//! flattened sessions, request/response matching, and an event queue.
//!
//! Every await here is driven by the coordinator's deadline: the operation
//! future is dropped on expiry, which stops all protocol activity. Nothing in
//! this module retries or spawns.
//!
//! Failures are classified, never quoted: no text received from the browser
//! reaches a diagnostic, because a proxy or a future browser could echo a
//! request payload in an error.

use std::collections::VecDeque;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;
use tokio_tungstenite::tungstenite::{Error as WsError, Message};
use tokio_tungstenite::WebSocketStream;

/// Largest protocol message accepted in either direction.
const MAX_MESSAGE: usize = 4 * 1024 * 1024;

/// A protocol-level failure with a fixed description. The only variable
/// parts are method names chosen by this crate and numeric error codes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum CdpError {
    /// The endpoint argument is not a supported loopback URL.
    Endpoint(&'static str),
    /// The loopback port did not accept a TCP connection.
    Connect,
    /// The version document could not be read or understood.
    VersionDocument(&'static str),
    /// The WebSocket handshake failed.
    Handshake,
    /// The connection is not open or closed while a request was pending.
    Closed { method: String },
    /// A request could not be written.
    Send { method: String },
    /// The browser sent a message over the size bound.
    Oversized,
    /// The browser answered the request with an error.
    Rejected { method: String, code: Option<i64> },
}

impl CdpError {
    pub(super) fn closed(method: &str) -> Self {
        Self::Closed {
            method: method.to_owned(),
        }
    }

    /// The browser reported that the method is unknown; callers treating a
    /// capability as optional can proceed.
    pub(super) fn method_not_found(&self) -> bool {
        matches!(
            self,
            Self::Rejected {
                code: Some(-32601),
                ..
            }
        )
    }

    /// A fixed description for diagnostics.
    pub(super) fn message(&self) -> String {
        match self {
            Self::Endpoint(detail) | Self::VersionDocument(detail) => (*detail).to_owned(),
            Self::Connect => {
                "cannot connect to the endpoint; is the browser's remote debugging port open?"
                    .to_owned()
            }
            Self::Handshake => "the browser did not accept the WebSocket connection".to_owned(),
            Self::Closed { method } => {
                format!("the browser connection closed while waiting for {method}")
            }
            Self::Send { method } => {
                format!("the browser connection failed while sending {method}")
            }
            Self::Oversized => "the browser sent an oversized message".to_owned(),
            Self::Rejected {
                method,
                code: Some(code),
            } => format!("the browser rejected {method} (protocol error {code})"),
            Self::Rejected { method, code: None } => format!("the browser rejected {method}"),
        }
    }
}

#[derive(Debug, Clone)]
pub(super) struct Event {
    pub(super) method: String,
    pub(super) params: Value,
}

pub(super) struct Connection {
    socket: WebSocketStream<TcpStream>,
    next_id: u64,
    events: VecDeque<Event>,
}

/// A loopback host as written on the command line, with its port.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Authority {
    host: String,
    port: u16,
}

impl Connection {
    /// Reads `/json/version` from the loopback HTTP endpoint and opens the
    /// browser WebSocket it names, which must be on the same loopback host.
    pub(super) async fn open(endpoint: &str) -> Result<(Self, Value), CdpError> {
        let authority = loopback_authority(endpoint)?;
        let addresses = loopback_addresses(&authority).await?;
        let (version, address) = fetch_version(&authority, &addresses).await?;
        let ws_url = version
            .get("webSocketDebuggerUrl")
            .and_then(Value::as_str)
            .ok_or(CdpError::VersionDocument(
                "the version document names no WebSocket",
            ))?;
        let ws_authority = websocket_authority(ws_url)?;
        if ws_authority.host != authority.host {
            return Err(CdpError::VersionDocument(
                "the version document names a WebSocket on another host; only the endpoint's loopback host is used",
            ));
        }
        // The WebSocket lives on the address that served the version
        // document; a name is not resolved a second time.
        let ws_address = SocketAddr::new(address.ip(), ws_authority.port);
        let (stream, _) = connect_loopback(&[ws_address]).await?;
        let config = WebSocketConfig::default()
            .max_message_size(Some(MAX_MESSAGE))
            .max_frame_size(Some(MAX_MESSAGE));
        let (socket, _) = tokio_tungstenite::client_async_with_config(ws_url, stream, Some(config))
            .await
            .map_err(|_| CdpError::Handshake)?;
        Ok((Self::from_socket(socket), version))
    }

    fn from_socket(socket: WebSocketStream<TcpStream>) -> Self {
        Self {
            socket,
            next_id: 0,
            events: VecDeque::new(),
        }
    }

    /// Sends one command and waits for its result. Events received meanwhile
    /// are queued for [`Connection::take_events`].
    pub(super) async fn send(
        &mut self,
        session_id: Option<&str>,
        method: &str,
        params: Value,
    ) -> Result<Value, CdpError> {
        self.next_id += 1;
        let id = self.next_id;
        let mut message = json!({ "id": id, "method": method, "params": params });
        if let Some(session_id) = session_id {
            message["sessionId"] = Value::String(session_id.to_owned());
        }
        let text = serde_json::to_string(&message).map_err(|_| CdpError::Send {
            method: method.to_owned(),
        })?;
        self.socket
            .send(Message::Text(text.into()))
            .await
            .map_err(|_| CdpError::Send {
                method: method.to_owned(),
            })?;
        loop {
            let incoming = match self.socket.next().await {
                Some(Ok(message)) => message,
                Some(Err(WsError::Capacity(_))) => return Err(CdpError::Oversized),
                Some(Err(_)) | None => return Err(CdpError::closed(method)),
            };
            let text = match incoming {
                Message::Text(text) => text,
                Message::Close(_) => return Err(CdpError::closed(method)),
                _ => continue,
            };
            if text.len() > MAX_MESSAGE {
                return Err(CdpError::Oversized);
            }
            let document: Value = match serde_json::from_str(text.as_str()) {
                Ok(document) => document,
                Err(_) => continue,
            };
            if document.get("id").and_then(Value::as_u64) == Some(id) {
                if let Some(error) = document.get("error") {
                    return Err(CdpError::Rejected {
                        method: method.to_owned(),
                        code: error.get("code").and_then(Value::as_i64),
                    });
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
    pub(super) fn take_events(&mut self, method: &str) -> Vec<Event> {
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

    /// Closes the WebSocket. Closing the browser connection detaches every
    /// session it owns without closing any target. A socket the browser
    /// already closed counts as detached.
    pub(super) async fn close(mut self) -> Result<(), CdpError> {
        match self.socket.close(None).await {
            Ok(())
            | Err(WsError::ConnectionClosed)
            | Err(WsError::AlreadyClosed)
            | Err(WsError::Protocol(_)) => Ok(()),
            Err(_) => Err(CdpError::Closed {
                method: "the close handshake".to_owned(),
            }),
        }
    }
}

/// Accepts `http://127.0.0.1:9222`, `http://localhost:9222`, or `http://[::1]:9222`
/// and returns the host and port. Remote endpoints are outside the contract.
fn loopback_authority(endpoint: &str) -> Result<Authority, CdpError> {
    let rest = endpoint.strip_prefix("http://").ok_or(CdpError::Endpoint(
        "the endpoint must be an http:// URL of the remote debugging port",
    ))?;
    parse_authority(rest.split('/').next().unwrap_or_default())
}

/// Accepts the `ws://` URL named by the version document with the same host
/// forms as the endpoint.
fn websocket_authority(url: &str) -> Result<Authority, CdpError> {
    let rest = url.strip_prefix("ws://").ok_or(CdpError::VersionDocument(
        "the version document names a WebSocket that is not a plain ws:// URL",
    ))?;
    parse_authority(rest.split('/').next().unwrap_or_default())
}

fn parse_authority(authority: &str) -> Result<Authority, CdpError> {
    let (host, port) = if let Some(bracketed) = authority.strip_prefix('[') {
        bracketed
            .split_once("]:")
            .ok_or(CdpError::Endpoint("the endpoint needs a port"))?
    } else {
        authority
            .rsplit_once(':')
            .ok_or(CdpError::Endpoint("the endpoint needs a port"))?
    };
    let port: u16 = port
        .parse()
        .map_err(|_| CdpError::Endpoint("the endpoint port is not a number"))?;
    let host = host.to_ascii_lowercase();
    let loopback = host == "localhost" || host == "127.0.0.1" || host == "::1";
    if !loopback {
        return Err(CdpError::Endpoint(
            "only loopback endpoints are supported; remote browsers need a connection-credential design",
        ));
    }
    Ok(Authority { host, port })
}

/// Resolves the authority to its loopback socket addresses. A name that
/// resolves elsewhere is refused even when it is `localhost`; a name that
/// resolves to several loopback addresses (`::1` and `127.0.0.1`) yields all
/// of them, because the browser usually listens on only one.
async fn loopback_addresses(authority: &Authority) -> Result<Vec<SocketAddr>, CdpError> {
    let candidates: Vec<SocketAddr> = match authority.host.as_str() {
        "127.0.0.1" => vec![SocketAddr::new(
            IpAddr::V4(Ipv4Addr::LOCALHOST),
            authority.port,
        )],
        "::1" => vec![SocketAddr::new(
            IpAddr::V6(Ipv6Addr::LOCALHOST),
            authority.port,
        )],
        name => tokio::net::lookup_host((name, authority.port))
            .await
            .map_err(|_| CdpError::Connect)?
            .collect(),
    };
    let loopback: Vec<SocketAddr> = candidates
        .into_iter()
        .filter(|address| address.ip().is_loopback())
        .collect();
    if loopback.is_empty() {
        return Err(CdpError::Endpoint(
            "the endpoint host does not resolve to a loopback address",
        ));
    }
    Ok(loopback)
}

/// Connects to the first loopback address that accepts the connection and
/// returns the stream with the address it reached.
async fn connect_loopback(addresses: &[SocketAddr]) -> Result<(TcpStream, SocketAddr), CdpError> {
    for address in addresses {
        if let Ok(stream) = TcpStream::connect(address).await {
            return Ok((stream, *address));
        }
    }
    Err(CdpError::Connect)
}

/// The HTTP Host header form of an authority.
fn host_header(authority: &Authority) -> String {
    if authority.host.contains(':') {
        format!("[{}]:{}", authority.host, authority.port)
    } else {
        format!("{}:{}", authority.host, authority.port)
    }
}

/// Fetches `/json/version` over a raw HTTP/1.1 request. The browser's
/// DevTools HTTP server ignores `Connection: close` and keeps the socket
/// open, so the response is read by its `Content-Length`, never to EOF.
async fn fetch_version(
    authority: &Authority,
    addresses: &[SocketAddr],
) -> Result<(Value, SocketAddr), CdpError> {
    let (mut stream, address) = connect_loopback(addresses).await?;
    let request = format!(
        "GET /json/version HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n\r\n",
        host_header(authority)
    );
    stream
        .write_all(request.as_bytes())
        .await
        .map_err(|_| CdpError::VersionDocument("cannot send the version request"))?;
    let mut response = Vec::new();
    let mut chunk = [0u8; 4096];
    let header_end = loop {
        if let Some(end) = find_header_end(&response) {
            break end;
        }
        if response.len() > MAX_MESSAGE {
            return Err(CdpError::VersionDocument(
                "the version response is oversized",
            ));
        }
        let read = stream
            .read(&mut chunk)
            .await
            .map_err(|_| CdpError::VersionDocument("cannot read the version response"))?;
        if read == 0 {
            return Err(CdpError::VersionDocument(
                "the version response is not HTTP",
            ));
        }
        response.extend_from_slice(&chunk[..read]);
    };
    let head = String::from_utf8_lossy(&response[..header_end]).into_owned();
    if !head.starts_with("HTTP/1.1 200") && !head.starts_with("HTTP/1.0 200") {
        return Err(CdpError::VersionDocument(
            "the endpoint did not return the version document",
        ));
    }
    let content_length = head
        .lines()
        .skip(1)
        .filter_map(|line| line.split_once(':'))
        .find(|(name, _)| name.trim().eq_ignore_ascii_case("content-length"))
        .and_then(|(_, value)| value.trim().parse::<usize>().ok());
    let body_start = header_end + 4;
    let mut body = response[body_start..].to_vec();
    match content_length {
        Some(length) if length > MAX_MESSAGE => {
            return Err(CdpError::VersionDocument(
                "the version document is oversized",
            ));
        }
        Some(length) => {
            while body.len() < length {
                let read = stream
                    .read(&mut chunk)
                    .await
                    .map_err(|_| CdpError::VersionDocument("cannot read the version response"))?;
                if read == 0 {
                    break;
                }
                body.extend_from_slice(&chunk[..read]);
            }
            body.truncate(length);
        }
        None => {
            stream
                .take((MAX_MESSAGE - body.len()) as u64)
                .read_to_end(&mut body)
                .await
                .map_err(|_| CdpError::VersionDocument("cannot read the version response"))?;
        }
    }
    let version = serde_json::from_slice(&body)
        .map_err(|_| CdpError::VersionDocument("the version document is not JSON"))?;
    Ok((version, address))
}

fn find_header_end(bytes: &[u8]) -> Option<usize> {
    bytes.windows(4).position(|window| window == b"\r\n\r\n")
}

#[cfg(test)]
mod tests {
    use std::net::SocketAddr;
    use std::time::Duration;

    use futures_util::{SinkExt, StreamExt};
    use serde_json::{json, Value};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};
    use tokio::task::JoinHandle;
    use tokio_tungstenite::tungstenite::Message;

    use super::{
        fetch_version, loopback_addresses, loopback_authority, websocket_authority, Authority,
        CdpError, Connection,
    };

    fn authority(host: &str, port: u16) -> Authority {
        Authority {
            host: host.to_owned(),
            port,
        }
    }

    #[test]
    fn endpoint_parsing_accepts_loopback_only() {
        for (endpoint, host, port) in [
            ("http://127.0.0.1:9222", "127.0.0.1", 9222),
            ("http://localhost:9222/", "localhost", 9222),
            ("http://LOCALHOST:9222/json", "localhost", 9222),
            ("http://[::1]:9222", "::1", 9222),
        ] {
            assert_eq!(
                loopback_authority(endpoint).unwrap(),
                authority(host, port),
                "{endpoint}"
            );
        }
        for bad in [
            "http://127.0.0.1",
            "http://example.com:9222",
            "http://10.0.0.5:9222",
            "https://127.0.0.1:9222",
            "ws://127.0.0.1:9222",
            "http://127.0.0.1:port",
        ] {
            assert!(loopback_authority(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn websocket_url_must_be_plain_ws_on_a_loopback_host() {
        assert_eq!(
            websocket_authority("ws://127.0.0.1:9222/devtools/browser/abc").unwrap(),
            authority("127.0.0.1", 9222)
        );
        for bad in [
            "wss://127.0.0.1:9222/devtools/browser/abc",
            "ws://203.0.113.5:9222/devtools/browser/abc",
            "ws://example.com:9222/devtools/browser/abc",
            "http://127.0.0.1:9222/devtools/browser/abc",
        ] {
            assert!(websocket_authority(bad).is_err(), "{bad}");
        }
    }

    #[tokio::test]
    async fn localhost_must_resolve_to_loopback_addresses() {
        let addresses = loopback_addresses(&authority("localhost", 9222))
            .await
            .unwrap();
        assert!(!addresses.is_empty());
        assert!(addresses.iter().all(|address| address.ip().is_loopback()));
        assert!(addresses.iter().all(|address| address.port() == 9222));
    }

    /// `localhost` may resolve to `::1` first while the browser listens on
    /// `127.0.0.1` only; every loopback address is tried.
    #[tokio::test]
    async fn localhost_reaches_a_server_bound_to_ipv4_loopback_only() {
        let (address, server) = http_server(
            r#"{"Browser":"Chrome/1","webSocketDebuggerUrl":"ws://127.0.0.1:1/x"}"#,
            "200 OK",
            "localhost",
        )
        .await;
        let addresses = loopback_addresses(&authority("localhost", address.port()))
            .await
            .unwrap();
        let (version, reached) = fetch_version(&authority("localhost", address.port()), &addresses)
            .await
            .unwrap();
        assert_eq!(version["Browser"], "Chrome/1");
        assert_eq!(reached, address, "the address that answered is reported");
        server.abort();
    }

    /// A fake DevTools HTTP server that answers one request by Content-Length
    /// and then keeps the connection open, as Chromium does.
    async fn http_server(
        body: &'static str,
        status: &'static str,
        expected_host: &'static str,
    ) -> (SocketAddr, JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let task = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = vec![0u8; 1024];
            let read = stream.read(&mut request).await.unwrap();
            let request = String::from_utf8_lossy(&request[..read]).into_owned();
            assert!(
                request.starts_with("GET /json/version HTTP/1.1\r\n"),
                "{request}"
            );
            assert!(
                request.contains(&format!("Host: {expected_host}:{}\r\n", address.port())),
                "{request}"
            );
            let response = format!(
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(response.as_bytes()).await.unwrap();
            tokio::time::sleep(Duration::from_secs(5)).await;
        });
        (address, task)
    }

    #[tokio::test]
    async fn version_fetch_reads_by_content_length_on_a_connection_that_stays_open() {
        let (address, server) = http_server(
            r#"{"Browser":"Chrome/1","webSocketDebuggerUrl":"ws://127.0.0.1:1/x"}"#,
            "200 OK",
            "127.0.0.1",
        )
        .await;
        let version = tokio::time::timeout(
            Duration::from_secs(2),
            fetch_version(&authority("127.0.0.1", address.port()), &[address]),
        )
        .await
        .expect("the fetch must not wait for EOF")
        .unwrap()
        .0;
        assert_eq!(version["Browser"], "Chrome/1");
        server.abort();
    }

    #[tokio::test]
    async fn version_fetch_rejects_non_success_and_non_json() {
        let (address, server) = http_server("nope", "404 Not Found", "127.0.0.1").await;
        assert_eq!(
            fetch_version(&authority("127.0.0.1", address.port()), &[address])
                .await
                .unwrap_err(),
            CdpError::VersionDocument("the endpoint did not return the version document")
        );
        server.abort();
        let (address, server) = http_server("not json", "200 OK", "127.0.0.1").await;
        assert_eq!(
            fetch_version(&authority("127.0.0.1", address.port()), &[address])
                .await
                .unwrap_err(),
            CdpError::VersionDocument("the version document is not JSON")
        );
        server.abort();
    }

    /// A fake browser WebSocket: `responder` maps each request to the frames
    /// it sends back.
    async fn ws_server(
        responder: impl Fn(Value) -> Vec<Message> + Send + 'static,
    ) -> (Connection, JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
            while let Some(Ok(message)) = socket.next().await {
                let Message::Text(text) = message else {
                    continue;
                };
                let request: Value = serde_json::from_str(text.as_str()).unwrap();
                for reply in responder(request) {
                    if socket.send(reply).await.is_err() {
                        return;
                    }
                }
            }
        });
        let stream = TcpStream::connect(address).await.unwrap();
        let (socket, _) = tokio_tungstenite::client_async(
            format!("ws://127.0.0.1:{}/devtools/browser/test", address.port()),
            stream,
        )
        .await
        .unwrap();
        (Connection::from_socket(socket), server)
    }

    fn text(value: Value) -> Message {
        Message::Text(value.to_string().into())
    }

    #[tokio::test]
    async fn requests_match_their_reply_and_queue_events_in_between() {
        let (mut connection, server) = ws_server(|request| {
            let id = request["id"].as_u64().unwrap();
            vec![
                Message::Text("not json".into()),
                Message::Binary(vec![1, 2, 3].into()),
                text(json!({"id": id + 1000, "result": {"foreign": true}})),
                text(json!({
                    "method": "Target.attachedToTarget",
                    "params": {"sessionId": "s1"},
                    "sessionId": "page"
                })),
                text(json!({"method": "Other.event", "params": {}})),
                text(json!({"id": id, "result": {"ok": request["method"]}})),
            ]
        })
        .await;
        let result = connection
            .send(Some("page"), "Runtime.evaluate", json!({"expression": "1"}))
            .await
            .unwrap();
        assert_eq!(result["ok"], "Runtime.evaluate");
        let attached = connection.take_events("Target.attachedToTarget");
        assert_eq!(attached.len(), 1);
        assert_eq!(attached[0].params["sessionId"], "s1");
        assert!(connection.take_events("Target.attachedToTarget").is_empty());
        assert_eq!(connection.take_events("Other.event").len(), 1);
        let result = connection
            .send(None, "Browser.getVersion", json!({}))
            .await
            .unwrap();
        assert_eq!(result["ok"], "Browser.getVersion");
        server.abort();
    }

    #[tokio::test]
    async fn browser_error_text_never_reaches_the_diagnostic() {
        let (mut connection, server) = ws_server(|request| {
            let id = request["id"].as_u64().unwrap();
            let echoed = format!(
                "{} with params {} is not allowed",
                request["method"].as_str().unwrap(),
                request["params"]
            );
            vec![text(
                json!({"id": id, "error": {"code": -32000, "message": echoed}}),
            )]
        })
        .await;
        let error = connection
            .send(
                Some("page"),
                "Input.insertText",
                json!({"text": "SENTINEL-VALUE-9000"}),
            )
            .await
            .unwrap_err();
        assert_eq!(
            error,
            CdpError::Rejected {
                method: "Input.insertText".to_owned(),
                code: Some(-32000)
            }
        );
        assert!(!error.message().contains("SENTINEL"));
        assert!(!format!("{error:?}").contains("SENTINEL"));
        assert!(!error.method_not_found());
        server.abort();
    }

    #[tokio::test]
    async fn unknown_methods_are_distinguished_and_closure_is_reported() {
        let (mut connection, server) = ws_server(|request| {
            let id = request["id"].as_u64().unwrap();
            if request["method"] == "Target.getDevToolsTarget" {
                vec![text(json!({
                    "id": id,
                    "error": {"code": -32601, "message": "'Target.getDevToolsTarget' wasn't found"}
                }))]
            } else {
                vec![Message::Close(None)]
            }
        })
        .await;
        let error = connection
            .send(None, "Target.getDevToolsTarget", json!({"targetId": "t"}))
            .await
            .unwrap_err();
        assert!(error.method_not_found());
        let error = connection
            .send(None, "Target.getTargets", json!({}))
            .await
            .unwrap_err();
        assert_eq!(error, CdpError::closed("Target.getTargets"));
        assert!(
            connection.close().await.is_ok(),
            "a closed socket counts as detached"
        );
        server.abort();
    }
}
