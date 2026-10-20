//! The WebSocket connection layer that the host engine and the client engine share.
//!
//! - `connect` opens a WebSocket to the relay with extra headers.
//!   `wss://` uses rustls with the ring provider and the webpki roots.
//! - A failed upgrade gives a [`ConnectError`] with the HTTP error of section 6.6.
//! - [`Backoff`] gives the reconnect delays with full jitter (sections 9 and 10).
//! - `spawn_writer` writes the outgoing frames and sends a ping at a fixed interval (section 6.8).
//!
//! The functions never log or return header values, because a header can hold a token.
//! The `http` crate keeps its own copy of each header value. This crate cannot zeroize that copy.

use std::sync::{Arc, OnceLock};
use std::time::Duration;

use futures_util::stream::{SplitSink, SplitStream};
use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::{HeaderName, HeaderValue};
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode as WsCloseCode;
use tokio_tungstenite::tungstenite::protocol::{CloseFrame, WebSocketConfig};
use tokio_tungstenite::tungstenite::{self, Bytes, Message};
use tokio_tungstenite::{Connector, MaybeTlsStream, WebSocketStream};

use crate::relay_wire::UpgradeError;

/// A WebSocket to the relay.
pub(crate) type WsStream = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// The read half of a [`WsStream`].
pub(crate) type WsReader = SplitStream<WsStream>;

/// The largest frame that an engine reads.
/// A relay frame is at most 65,540 bytes (section 6.7), so this leaves a margin.
const MAX_INCOMING_FRAME: usize = 128 * 1024;

/// The number of frames that wait for the writer task.
pub(crate) const WRITE_QUEUE: usize = 256;

/// Why a connection to the relay failed before the WebSocket opened.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum ConnectError {
    /// The relay refused the upgrade with an HTTP error of section 6.6.
    #[error("the relay refused the connection: {} {}", .error.status(), .error.code())]
    Upgrade {
        /// The HTTP error.
        error: UpgradeError,
        /// The `Retry-After` value, if the answer had one.
        retry_after: Option<Duration>,
    },
    /// The relay answered with an HTTP status that section 6.6 does not define.
    #[error("the relay answered with HTTP status {0}")]
    Http(u16),
    /// DNS, TCP, TLS, or the WebSocket handshake failed.
    #[error("network error: {0}")]
    Network(String),
    /// The connection did not open in time.
    #[error("the connection timed out")]
    Timeout,
    /// The URL is not a `ws://` or `wss://` URL with a host.
    #[error("invalid relay URL")]
    InvalidUrl,
}

/// Returns the rustls configuration: the ring provider and the webpki roots.
///
/// The configuration does not use the process-wide default provider,
/// so another rustls user in the same process cannot change it.
fn tls_config() -> Arc<rustls::ClientConfig> {
    static CONFIG: OnceLock<Arc<rustls::ClientConfig>> = OnceLock::new();
    CONFIG
        .get_or_init(|| {
            let roots = rustls::RootCertStore {
                roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
            };
            let config = rustls::ClientConfig::builder_with_provider(Arc::new(
                rustls::crypto::ring::default_provider(),
            ))
            .with_safe_default_protocol_versions()
            .expect("the ring provider supports the default TLS versions")
            .with_root_certificates(roots)
            .with_no_client_auth();
            Arc::new(config)
        })
        .clone()
}

/// The WebSocket limits of an engine socket.
fn socket_config() -> WebSocketConfig {
    WebSocketConfig::default()
        .max_message_size(Some(MAX_INCOMING_FRAME))
        .max_frame_size(Some(MAX_INCOMING_FRAME))
}

/// Opens a WebSocket to `url` with `headers`.
///
/// The TCP connection, the TLS handshake, and the WebSocket upgrade must finish in `timeout`.
/// The function connects to the host of the URL itself, so IPv6 literals such as `[::1]` work.
pub(crate) async fn connect(
    url: &str,
    headers: Vec<(HeaderName, HeaderValue)>,
    timeout: Duration,
) -> Result<WsStream, ConnectError> {
    let mut request = url
        .into_client_request()
        .map_err(|_| ConnectError::InvalidUrl)?;
    for (name, value) in headers {
        request.headers_mut().insert(name, value);
    }
    let uri = request.uri();
    let secure = match uri.scheme_str() {
        Some("wss") => true,
        Some("ws") => false,
        _ => return Err(ConnectError::InvalidUrl),
    };
    let host = uri.host().ok_or(ConnectError::InvalidUrl)?;
    let host = host
        .strip_prefix('[')
        .and_then(|inner| inner.strip_suffix(']'))
        .unwrap_or(host)
        .to_owned();
    let port = uri.port_u16().unwrap_or(if secure { 443 } else { 80 });
    let connector = if secure {
        Connector::Rustls(tls_config())
    } else {
        Connector::Plain
    };

    let attempt = async move {
        let socket = TcpStream::connect((host.as_str(), port))
            .await
            .map_err(|error| ConnectError::Network(error.kind().to_string()))?;
        // Small frames such as acks must not wait for Nagle's algorithm.
        let _ = socket.set_nodelay(true);
        tokio_tungstenite::client_async_tls_with_config(
            request,
            socket,
            Some(socket_config()),
            Some(connector),
        )
        .await
        .map(|(socket, _response)| socket)
        .map_err(classify_error)
    };
    match tokio::time::timeout(timeout, attempt).await {
        Ok(result) => result,
        Err(_) => Err(ConnectError::Timeout),
    }
}

/// Reads a failed upgrade. The texts never hold header values.
fn classify_error(error: tungstenite::Error) -> ConnectError {
    match error {
        tungstenite::Error::Http(response) => {
            let status = response.status().as_u16();
            let body = response
                .body()
                .as_deref()
                .map(String::from_utf8_lossy)
                .unwrap_or_default();
            let retry_after = response
                .headers()
                .get("Retry-After")
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.trim().parse::<u64>().ok())
                .map(Duration::from_secs);
            match UpgradeError::from_response(status, &body) {
                Some(error) => ConnectError::Upgrade { error, retry_after },
                None => ConnectError::Http(status),
            }
        }
        tungstenite::Error::Io(error) => ConnectError::Network(error.kind().to_string()),
        tungstenite::Error::Tls(_) => ConnectError::Network("TLS handshake failed".to_owned()),
        tungstenite::Error::Url(_) => ConnectError::InvalidUrl,
        _ => ConnectError::Network("the WebSocket handshake failed".to_owned()),
    }
}

/// Makes a header value that holds a secret. The `http` crate then hides it in `Debug` output.
#[cfg(feature = "client")]
pub(crate) fn secret_header(value: &str) -> Option<HeaderValue> {
    let mut value = HeaderValue::from_str(value).ok()?;
    value.set_sensitive(true);
    Some(value)
}

/// A frame for the writer task.
#[derive(Debug)]
pub(crate) enum Outgoing {
    /// A text or binary frame.
    Frame(Message),
    /// A close frame. The writer stops after it.
    Close(u16, &'static str),
}

/// Splits a socket and starts its writer task.
///
/// The writer sends the frames of the channel in order, and a ping every `ping_interval`.
/// It stops after a close frame, when the channel closes, or after a write error.
pub(crate) fn spawn_writer(
    socket: WsStream,
    ping_interval: Duration,
) -> (WsReader, mpsc::Sender<Outgoing>, JoinHandle<()>) {
    let (sink, reader) = socket.split();
    let (sender, receiver) = mpsc::channel(WRITE_QUEUE);
    let task = tokio::spawn(write_loop(sink, receiver, ping_interval));
    (reader, sender, task)
}

async fn write_loop(
    mut sink: SplitSink<WsStream, Message>,
    mut receiver: mpsc::Receiver<Outgoing>,
    ping_interval: Duration,
) {
    let start = tokio::time::Instant::now() + ping_interval;
    let mut ping = tokio::time::interval_at(start, ping_interval);
    ping.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            next = receiver.recv() => {
                let Some(outgoing) = next else {
                    let _ = sink.close().await;
                    return;
                };
                if write_one(&mut sink, outgoing).await.is_err() {
                    return;
                }
                // Write the frames that are already queued, then flush once.
                let mut closed = false;
                while let Ok(outgoing) = receiver.try_recv() {
                    if write_one(&mut sink, outgoing).await.is_err() {
                        closed = true;
                        break;
                    }
                }
                if closed || sink.flush().await.is_err() {
                    return;
                }
            }
            _ = ping.tick() => {
                if sink.send(Message::Ping(Bytes::new())).await.is_err() {
                    return;
                }
            }
        }
    }
}

/// Queues one frame. A close frame flushes and returns an error, so the writer stops.
async fn write_one(sink: &mut SplitSink<WsStream, Message>, outgoing: Outgoing) -> Result<(), ()> {
    match outgoing {
        Outgoing::Frame(message) => sink.feed(message).await.map_err(|_| ()),
        Outgoing::Close(code, reason) => {
            let frame = CloseFrame {
                code: WsCloseCode::from(code),
                reason: reason.into(),
            };
            let _ = sink.send(Message::Close(Some(frame))).await;
            Err(())
        }
    }
}

/// A frame that the reader got, reduced to what the engines need.
#[derive(Debug)]
pub(crate) enum Incoming {
    /// A binary frame.
    Binary(Bytes),
    /// A text frame. Only the host reads control messages.
    Text(#[cfg_attr(not(feature = "host"), allow(dead_code))] String),
    /// A ping or a pong. It only proves that the peer is alive.
    Alive,
    /// The peer closed the socket. The close code, if the close frame had one.
    Closed(Option<u16>),
    /// The socket failed. The text has no frame content.
    Failed(String),
}

/// Reads the next frame.
pub(crate) async fn read_frame(reader: &mut WsReader) -> Incoming {
    match reader.next().await {
        Some(Ok(Message::Binary(bytes))) => Incoming::Binary(bytes),
        Some(Ok(Message::Text(text))) => Incoming::Text(text.as_str().to_owned()),
        Some(Ok(Message::Ping(_) | Message::Pong(_) | Message::Frame(_))) => Incoming::Alive,
        Some(Ok(Message::Close(frame))) => Incoming::Closed(frame.map(|frame| frame.code.into())),
        Some(Err(tungstenite::Error::Capacity(_))) => {
            Incoming::Failed("a frame is larger than the limit".to_owned())
        }
        Some(Err(tungstenite::Error::Io(error))) => Incoming::Failed(error.kind().to_string()),
        Some(Err(_)) => Incoming::Failed("the WebSocket failed".to_owned()),
        None => Incoming::Closed(None),
    }
}

/// Reconnect delays with full jitter.
///
/// Each delay is a random value from zero to a ceiling.
/// The ceiling starts at the minimum and doubles after each failure, up to the maximum.
/// [`Backoff::reset`] starts again at the minimum, for example after 60 seconds of connection.
#[derive(Debug, Clone)]
pub struct Backoff {
    min: Duration,
    max: Duration,
    failures: u32,
}

impl Backoff {
    /// Makes a backoff from `min` to `max`.
    pub fn new(min: Duration, max: Duration) -> Backoff {
        Backoff {
            min,
            max: max.max(min),
            failures: 0,
        }
    }

    /// Returns the current ceiling.
    pub fn ceiling(&self) -> Duration {
        let factor = 1u32 << self.failures.min(20);
        self.min.saturating_mul(factor).min(self.max)
    }

    /// Returns the next delay and raises the ceiling.
    pub fn next_delay(&mut self) -> Duration {
        let ceiling = self.ceiling();
        self.failures = self.failures.saturating_add(1);
        random_between(Duration::ZERO, ceiling)
    }

    /// Returns a long delay, from half the maximum to the maximum.
    /// The engines use it after a rate limit (`4429` or HTTP `429`).
    pub fn long_delay(&mut self) -> Duration {
        self.failures = self.failures.saturating_add(1);
        random_between(self.max / 2, self.max)
    }

    /// Starts again at the minimum.
    pub fn reset(&mut self) {
        self.failures = 0;
    }
}

/// Returns a random duration from `low` to `high`.
pub(crate) fn random_between(low: Duration, high: Duration) -> Duration {
    let (low, high) = (low.min(high), low.max(high));
    let span = u64::try_from((high - low).as_millis()).unwrap_or(u64::MAX);
    let mut bytes = [0u8; 8];
    // A failed random source gives the middle of the range. The delay is not a secret.
    let offset = if getrandom::fill(&mut bytes).is_ok() {
        u64::from_le_bytes(bytes) % span.saturating_add(1)
    } else {
        span / 2
    };
    low + Duration::from_millis(offset)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_ceiling_doubles_up_to_the_maximum() {
        let mut backoff = Backoff::new(Duration::from_secs(1), Duration::from_secs(60));
        let mut ceilings = Vec::new();
        for _ in 0..9 {
            ceilings.push(backoff.ceiling().as_secs());
            let delay = backoff.next_delay();
            assert!(delay <= Duration::from_secs(60));
        }
        assert_eq!(ceilings, [1, 2, 4, 8, 16, 32, 60, 60, 60]);
        backoff.reset();
        assert_eq!(backoff.ceiling(), Duration::from_secs(1));
    }

    #[test]
    fn delays_stay_in_their_range() {
        let mut backoff = Backoff::new(Duration::from_millis(500), Duration::from_secs(30));
        for _ in 0..200 {
            let delay = backoff.next_delay();
            assert!(delay <= Duration::from_secs(30));
            let long = backoff.long_delay();
            assert!(long >= Duration::from_secs(15) && long <= Duration::from_secs(30));
        }
        for _ in 0..200 {
            let delay = random_between(Duration::from_secs(10), Duration::from_secs(30));
            assert!(delay >= Duration::from_secs(10) && delay <= Duration::from_secs(30));
        }
        assert_eq!(
            random_between(Duration::from_secs(3), Duration::from_secs(3)),
            Duration::from_secs(3)
        );
    }

    #[test]
    fn upgrade_errors_are_classified() {
        let response = |status: u16, body: &str, retry: Option<&str>| {
            let mut builder = tungstenite::http::Response::builder().status(status);
            if let Some(retry) = retry {
                builder = builder.header("Retry-After", retry);
            }
            tungstenite::Error::Http(builder.body(Some(body.as_bytes().to_vec())).unwrap())
        };
        assert_eq!(
            classify_error(response(503, r#"{"code":"host_offline"}"#, Some("5"))),
            ConnectError::Upgrade {
                error: UpgradeError::HostOffline,
                retry_after: Some(Duration::from_secs(5)),
            }
        );
        assert_eq!(
            classify_error(response(401, r#"{"code":"invite_expired"}"#, None)),
            ConnectError::Upgrade {
                error: UpgradeError::InviteExpired,
                retry_after: None,
            }
        );
        assert_eq!(
            classify_error(response(500, "{}", None)),
            ConnectError::Http(500)
        );
    }
}
