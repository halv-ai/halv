//! An in-process relay for tests (section 6).
//!
//! [`MockRelay`] listens on `127.0.0.1` with a random port and speaks the relay protocol over
//! real WebSockets. It does the checks of the Cloudflare relay in the same order:
//! the host signature with clock skew and replay, device admission with invites and tries,
//! the DATA frames, the control messages, the close codes, and the limits.
//!
//! It forwards each frame with no `await` between the receive and the send (section 6.10).
//! Tests can move its clock and break frames on purpose.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::handshake::derive_accept_key;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode as WsCloseCode;
use tokio_tungstenite::tungstenite::protocol::{CloseFrame, Role, WebSocketConfig};
use tokio_tungstenite::tungstenite::{Bytes, Message};
use tokio_tungstenite::WebSocketStream;

use crate::ids::{is_valid_id, ConnId, DeviceId, HostId, InviteId};
use crate::keys::TokenHash;
use crate::relay_auth::{
    bearer_token_hash, verify_host_auth, HostAuthRequest, HostAuthState, HEADER_HOST_KEY,
    HEADER_SIGNATURE, HEADER_TIMESTAMP,
};
use crate::relay_wire::{
    CloseCode, ConnOpen, ConnPeer, HostToRelay, RelayToHost, UpgradeError, DATA_FRAME_TYPE,
    DATA_HEADER_LEN, HOST_OFFLINE_RETRY_AFTER_SECS, MAX_ACTIVE_INVITES, MAX_CLOSE_REASON_BYTES,
    MAX_DEVICE_FRAME, MAX_HOST_CONTROL_FRAME, MAX_HOST_DATA_FRAME, MAX_INVITE_LIFETIME_MS,
    MAX_INVITE_TRIES, MAX_OPEN_DEVICE_SOCKETS, MAX_PAIRED_DEVICES, PING_TEXT, PONG_TEXT,
    RELAY_PROTOCOL_VERSION, UPGRADE_RATE_LIMIT, UPGRADE_RATE_WINDOW_MS,
};
use crate::unix_time_ms;

/// The largest HTTP request head that the relay reads.
const MAX_REQUEST_HEAD: usize = 16 * 1024;

/// The WebSocket limit of the relay sockets. It is above the frame limits of section 6.7,
/// so the relay can close an oversized frame with `4413` itself.
const SOCKET_FRAME_LIMIT: usize = 1024 * 1024;

/// The close code that the relay reports for a socket that ended without a close frame.
const ABNORMAL_CLOSE: u16 = 1006;

/// The settings of a [`MockRelay`].
#[derive(Debug, Clone)]
pub struct MockRelayConfig {
    /// Upgrades for each client IP and path kind in 60 seconds. `None` turns the limit off.
    pub upgrade_rate_limit: Option<u32>,
    /// The open device sockets of one host object. The contract says 8.
    /// A higher value acts as a relay that breaks the limit, so a test can check the host limit.
    pub max_device_sockets: usize,
}

impl Default for MockRelayConfig {
    fn default() -> Self {
        MockRelayConfig {
            upgrade_rate_limit: Some(UPGRADE_RATE_LIMIT),
            max_device_sockets: MAX_OPEN_DEVICE_SOCKETS,
        }
    }
}

/// The direction of a forwarded frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Direction {
    /// From the host socket to a device socket.
    ToDevice,
    /// From a device socket to the host socket.
    ToHost,
}

/// A change that the relay makes to the next forwarded Noise message of one direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fault {
    /// Flip one bit of the Noise message.
    Corrupt,
    /// Send the Noise message twice.
    Replay,
    /// Hold the Noise message and send it after the next one of the same connection.
    Reorder,
    /// Drop the Noise message.
    Drop,
}

/// An invite in the storage of a host object.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MockInvite {
    /// The invite id.
    pub invite: InviteId,
    /// The hash of the invite token.
    pub hash: TokenHash,
    /// The expiry time on the relay clock.
    pub exp: u64,
    /// The admissions that are left.
    pub tries: u32,
}

/// A frame for the writer task of one socket.
enum Out {
    Frame(Message),
    Close(u16, String),
}

#[derive(Clone)]
struct SocketRef {
    id: u64,
    out: mpsc::UnboundedSender<Out>,
    /// True after [`MockRelay::stall_device_sockets`].
    stalled: Arc<watch::Sender<bool>>,
}

impl SocketRef {
    fn send(&self, message: Message) {
        let _ = self.out.send(Out::Frame(message));
    }

    fn close(&self, code: u16, reason: &str) {
        let _ = self.out.send(Out::Close(code, reason.to_owned()));
    }
}

struct DeviceSocket {
    socket: SocketRef,
    peer: ConnPeer,
}

#[derive(Debug, Clone)]
struct InviteRow {
    hash: TokenHash,
    exp: u64,
    tries: u32,
}

/// One host object: the storage of section 6.9 and the open sockets.
#[derive(Default)]
struct HostObject {
    host_key: Option<String>,
    last_auth_ts: Option<u64>,
    devices: BTreeMap<DeviceId, TokenHash>,
    invites: BTreeMap<InviteId, InviteRow>,
    host: Option<SocketRef>,
    sockets: BTreeMap<u32, DeviceSocket>,
}

impl HostObject {
    fn send_host(&self, message: &RelayToHost) {
        if let (Some(host), Ok(text)) = (&self.host, message.to_json()) {
            host.send(Message::text(text));
        }
    }

    fn send_err(&self, rid: Option<u32>, code: &str, msg: &str) {
        self.send_host(&RelayToHost::Err {
            rid,
            code: code.to_owned(),
            msg: msg.to_owned(),
        });
    }

    /// Closes a device socket and tells the host with `conn_close`.
    fn close_device(&mut self, conn: u32, code: u16, reason: &str) {
        if let Some(device) = self.sockets.remove(&conn) {
            device.socket.close(code, reason);
            if let Some(conn) = ConnId::new(conn) {
                self.send_host(&RelayToHost::ConnClose { conn, code });
            }
        }
    }

    /// Closes every device socket with `4408`, without `conn_close`.
    fn close_all_devices(&mut self) {
        for (_, device) in std::mem::take(&mut self.sockets) {
            device
                .socket
                .close(CloseCode::HostGone.code(), "host closed");
        }
    }

    /// Closes the device sockets of paired devices that `keep` rejects, with `4401`.
    fn close_removed_devices(&mut self, keep: impl Fn(&DeviceId) -> bool) {
        let removed: Vec<u32> = self
            .sockets
            .iter()
            .filter(|(_, socket)| matches!(&socket.peer, ConnPeer::Device(device) if !keep(device)))
            .map(|(conn, _)| *conn)
            .collect();
        for conn in removed {
            self.close_device(conn, CloseCode::Unauthorized.code(), "unauthorized");
        }
    }
}

/// The faults that tests queued, and the Noise messages that a reorder holds.
#[derive(Default)]
struct Faults {
    queued: HashMap<Direction, VecDeque<Fault>>,
    held: HashMap<(Direction, u32), Vec<u8>>,
}

impl Faults {
    /// Applies the next fault of `direction` to one Noise message.
    /// It returns the Noise messages to send now, in order.
    fn apply(&mut self, direction: Direction, conn: u32, message: &[u8]) -> Vec<Vec<u8>> {
        let fault = self
            .queued
            .get_mut(&direction)
            .and_then(VecDeque::pop_front);
        let mut out = Vec::with_capacity(2);
        match fault {
            None => out.push(message.to_vec()),
            Some(Fault::Corrupt) => {
                let mut changed = message.to_vec();
                let last = changed.len() - 1;
                changed[last] ^= 0x01;
                out.push(changed);
            }
            Some(Fault::Replay) => {
                out.push(message.to_vec());
                out.push(message.to_vec());
            }
            Some(Fault::Drop) => {}
            Some(Fault::Reorder) => {
                self.held.insert((direction, conn), message.to_vec());
                return out;
            }
        }
        if let Some(held) = self.held.remove(&(direction, conn)) {
            out.push(held);
        }
        out
    }
}

struct RelayState {
    hosts: HashMap<HostId, HostObject>,
    rate: HashMap<(IpAddr, bool), VecDeque<std::time::Instant>>,
    faults: Faults,
}

struct RelayInner {
    config: MockRelayConfig,
    clock_offset_ms: AtomicI64,
    next_socket: AtomicU64,
    state: Mutex<RelayState>,
    shutdown: watch::Sender<bool>,
}

impl RelayInner {
    fn lock(&self) -> MutexGuard<'_, RelayState> {
        self.state
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
    }

    fn now_ms(&self) -> u64 {
        let offset = self.clock_offset_ms.load(Ordering::Relaxed);
        unix_time_ms().saturating_add_signed(offset)
    }
}

/// An in-process relay for tests. Dropping it closes every socket.
pub struct MockRelay {
    inner: Arc<RelayInner>,
    addr: SocketAddr,
    accept: JoinHandle<()>,
}

impl std::fmt::Debug for MockRelay {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MockRelay")
            .field("addr", &self.addr)
            .finish_non_exhaustive()
    }
}

impl Drop for MockRelay {
    fn drop(&mut self) {
        self.accept.abort();
        self.inner.shutdown.send_replace(true);
    }
}

impl MockRelay {
    /// Starts a relay with the limits of the contract.
    pub async fn start() -> std::io::Result<MockRelay> {
        MockRelay::start_with(MockRelayConfig::default()).await
    }

    /// Starts a relay with other settings.
    pub async fn start_with(config: MockRelayConfig) -> std::io::Result<MockRelay> {
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).await?;
        let addr = listener.local_addr()?;
        let (shutdown, _) = watch::channel(false);
        let inner = Arc::new(RelayInner {
            config,
            clock_offset_ms: AtomicI64::new(0),
            next_socket: AtomicU64::new(1),
            state: Mutex::new(RelayState {
                hosts: HashMap::new(),
                rate: HashMap::new(),
                faults: Faults::default(),
            }),
            shutdown,
        });
        let accept = tokio::spawn(accept_loop(inner.clone(), listener));
        Ok(MockRelay {
            inner,
            addr,
            accept,
        })
    }

    /// Returns the relay base URL, for example `ws://127.0.0.1:53000`.
    pub fn url(&self) -> String {
        format!("ws://{}", self.addr)
    }

    /// Returns the address of the listener.
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// Returns the time of the relay clock in epoch milliseconds.
    pub fn now_ms(&self) -> u64 {
        self.inner.now_ms()
    }

    /// Moves the relay clock forward, for example past the expiry of an invite.
    pub fn advance_clock(&self, by: Duration) {
        let by = i64::try_from(by.as_millis()).unwrap_or(i64::MAX);
        self.inner.clock_offset_ms.fetch_add(by, Ordering::Relaxed);
    }

    /// Moves the relay clock back.
    pub fn rewind_clock(&self, by: Duration) {
        let by = i64::try_from(by.as_millis()).unwrap_or(i64::MAX);
        self.inner.clock_offset_ms.fetch_sub(by, Ordering::Relaxed);
    }

    /// Changes the next forwarded Noise message of `direction`.
    pub fn inject_fault(&self, direction: Direction, fault: Fault) {
        self.inner
            .lock()
            .faults
            .queued
            .entry(direction)
            .or_default()
            .push_back(fault);
    }

    /// Returns true while a host socket of `host` is open.
    pub fn host_connected(&self, host: &HostId) -> bool {
        self.inner
            .lock()
            .hosts
            .get(host)
            .is_some_and(|object| object.host.is_some())
    }

    /// Returns the number of open device sockets of `host`.
    pub fn device_sockets(&self, host: &HostId) -> usize {
        self.inner
            .lock()
            .hosts
            .get(host)
            .map_or(0, |object| object.sockets.len())
    }

    /// Returns the paired devices in the storage of `host`.
    pub fn devices(&self, host: &HostId) -> Vec<(DeviceId, TokenHash)> {
        self.inner
            .lock()
            .hosts
            .get(host)
            .map(|object| {
                object
                    .devices
                    .iter()
                    .map(|(device, hash)| (device.clone(), hash.clone()))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Returns the invites in the storage of `host`.
    pub fn invites(&self, host: &HostId) -> Vec<MockInvite> {
        self.inner
            .lock()
            .hosts
            .get(host)
            .map(|object| {
                object
                    .invites
                    .iter()
                    .map(|(invite, row)| MockInvite {
                        invite: invite.clone(),
                        hash: row.hash.clone(),
                        exp: row.exp,
                        tries: row.tries,
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Returns the stored `last_auth_ts` of `host`.
    pub fn last_auth_ts(&self, host: &HostId) -> Option<u64> {
        self.inner
            .lock()
            .hosts
            .get(host)
            .and_then(|object| object.last_auth_ts)
    }

    /// Adds a device to the storage of `host`, as a relay that still admits a revoked device.
    pub fn put_device(&self, host: &HostId, device: DeviceId, hash: TokenHash) {
        self.inner
            .lock()
            .hosts
            .entry(host.clone())
            .or_default()
            .devices
            .insert(device, hash);
    }

    /// Closes the host socket of `host` with `code`, as a relay deploy does.
    /// The device sockets close with `4408`.
    pub fn disconnect_host(&self, host: &HostId, code: u16) {
        let mut state = self.inner.lock();
        if let Some(object) = state.hosts.get_mut(host) {
            if let Some(socket) = object.host.take() {
                socket.close(code, "relay restart");
            }
            object.close_all_devices();
        }
    }

    /// Makes the open device sockets of `host` look open but dead, as after a suspend on iOS.
    ///
    /// The relay stops reading them, so it answers no ping, and it drops the frames to them.
    /// The TCP connections stay open until the relay drops.
    pub fn stall_device_sockets(&self, host: &HostId) {
        let state = self.inner.lock();
        if let Some(object) = state.hosts.get(host) {
            for device in object.sockets.values() {
                device.socket.stalled.send_replace(true);
            }
        }
    }

    /// Closes every device socket of `host` with `code`, and tells the host.
    pub fn close_device_sockets(&self, host: &HostId, code: u16) {
        let mut state = self.inner.lock();
        if let Some(object) = state.hosts.get_mut(host) {
            let conns: Vec<u32> = object.sockets.keys().copied().collect();
            for conn in conns {
                object.close_device(conn, code, "closed by the test");
            }
        }
    }
}

async fn accept_loop(inner: Arc<RelayInner>, listener: TcpListener) {
    loop {
        let Ok((stream, peer)) = listener.accept().await else {
            // For example too many open files. Wait a little, so the loop does not spin.
            tokio::time::sleep(Duration::from_millis(10)).await;
            continue;
        };
        let inner = inner.clone();
        tokio::spawn(async move {
            let _ = handle_connection(inner, stream, peer.ip()).await;
        });
    }
}

/// Writes an HTTP error with the JSON body `{"code": "<code>"}` (section 6.6).
async fn write_error(stream: &mut TcpStream, error: UpgradeError) -> std::io::Result<()> {
    let body = error.body();
    let reason = match error.status() {
        400 => "Bad Request",
        401 => "Unauthorized",
        404 => "Not Found",
        426 => "Upgrade Required",
        429 => "Too Many Requests",
        503 => "Service Unavailable",
        _ => "Error",
    };
    let mut head = format!(
        "HTTP/1.1 {} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n",
        error.status(),
        body.len()
    );
    match error {
        UpgradeError::HostOffline => {
            head.push_str(&format!("Retry-After: {HOST_OFFLINE_RETRY_AFTER_SECS}\r\n"));
        }
        UpgradeError::UpgradeRequired => head.push_str("Upgrade: websocket\r\n"),
        _ => {}
    }
    head.push_str("\r\n");
    head.push_str(&body);
    stream.write_all(head.as_bytes()).await?;
    stream.shutdown().await
}

/// Reads the request head. It returns the head and the bytes after it.
async fn read_head(stream: &mut TcpStream) -> std::io::Result<Option<(Vec<u8>, Vec<u8>)>> {
    let mut buffer = Vec::with_capacity(1024);
    let mut chunk = [0u8; 1024];
    loop {
        let read = stream.read(&mut chunk).await?;
        if read == 0 {
            return Ok(None);
        }
        buffer.extend_from_slice(&chunk[..read]);
        if let Some(end) = buffer.windows(4).position(|window| window == b"\r\n\r\n") {
            let rest = buffer.split_off(end + 4);
            return Ok(Some((buffer, rest)));
        }
        if buffer.len() > MAX_REQUEST_HEAD {
            return Ok(None);
        }
    }
}

/// The kind of an upgrade path (section 6.1).
#[derive(Clone, Copy, PartialEq, Eq)]
enum PathKind {
    Host,
    Device,
}

fn parse_upgrade_path(path: &str) -> Option<(HostId, PathKind)> {
    let rest = path.strip_prefix("/v1/hosts/")?;
    let (id, kind) = rest.split_once('/')?;
    if !is_valid_id(id) {
        return None;
    }
    let kind = match kind {
        "host" => PathKind::Host,
        "device" => PathKind::Device,
        _ => return None,
    };
    Some((HostId::parse(id).ok()?, kind))
}

async fn handle_connection(
    inner: Arc<RelayInner>,
    mut stream: TcpStream,
    ip: IpAddr,
) -> std::io::Result<()> {
    let _ = stream.set_nodelay(true);
    let Some((head, rest)) = read_head(&mut stream).await? else {
        return Ok(());
    };
    let mut headers = [httparse::EMPTY_HEADER; 64];
    let mut request = httparse::Request::new(&mut headers);
    if !matches!(request.parse(&head), Ok(httparse::Status::Complete(_))) {
        return write_error(&mut stream, UpgradeError::BadRequest).await;
    }
    let header = |name: &str| -> Option<String> {
        request
            .headers
            .iter()
            .find(|header| header.name.eq_ignore_ascii_case(name))
            .and_then(|header| std::str::from_utf8(header.value).ok())
            .map(str::to_owned)
    };
    let path = request.path.unwrap_or_default().to_owned();
    if path == crate::relay_wire::HEALTH_PATH && request.method == Some("GET") {
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok")
            .await?;
        return stream.shutdown().await;
    }
    let Some((host_id, kind)) = parse_upgrade_path(&path) else {
        return write_error(&mut stream, UpgradeError::NotFound).await;
    };
    let upgrade = header("Upgrade").is_some_and(|value| {
        value
            .split(',')
            .any(|part| part.trim().eq_ignore_ascii_case("websocket"))
    });
    if request.method != Some("GET") || !upgrade {
        return write_error(&mut stream, UpgradeError::UpgradeRequired).await;
    }
    if !inner.allow_upgrade(ip, kind == PathKind::Host) {
        return write_error(&mut stream, UpgradeError::RateLimited).await;
    }
    let Some(key) = header("Sec-WebSocket-Key") else {
        return write_error(&mut stream, UpgradeError::BadRequest).await;
    };
    if header("Sec-WebSocket-Version").as_deref() != Some("13") {
        return write_error(&mut stream, UpgradeError::BadRequest).await;
    }

    let socket_id = inner.next_socket.fetch_add(1, Ordering::Relaxed);
    let (out, outgoing) = mpsc::unbounded_channel();
    let (stalled, stall) = watch::channel(false);
    let socket = SocketRef {
        id: socket_id,
        out,
        stalled: Arc::new(stalled),
    };
    let admitted = match kind {
        PathKind::Host => {
            let host_key = header(HEADER_HOST_KEY);
            let timestamp = header(HEADER_TIMESTAMP);
            let signature = header(HEADER_SIGNATURE);
            let request = HostAuthRequest {
                host_key: host_key.as_deref(),
                timestamp: timestamp.as_deref(),
                signature: signature.as_deref(),
            };
            inner
                .accept_host(&host_id, &request, socket.clone())
                .map(|()| None)
        }
        PathKind::Device => inner
            .admit_device(&host_id, header("Authorization").as_deref(), socket.clone())
            .map(Some),
    };
    let conn = match admitted {
        Ok(conn) => conn,
        Err(error) => return write_error(&mut stream, error).await,
    };

    let accept = derive_accept_key(key.as_bytes());
    let response = format!(
        "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {accept}\r\n\r\n"
    );
    if stream.write_all(response.as_bytes()).await.is_err() {
        inner.socket_gone(&host_id, socket_id, conn, ABNORMAL_CLOSE);
        return Ok(());
    }
    let config = WebSocketConfig::default()
        .max_message_size(Some(SOCKET_FRAME_LIMIT))
        .max_frame_size(Some(SOCKET_FRAME_LIMIT));
    let websocket =
        WebSocketStream::from_partially_read(stream, rest, Role::Server, Some(config)).await;
    run_socket(inner, websocket, host_id, socket_id, conn, outgoing, stall).await;
    Ok(())
}

/// Runs one accepted socket: a writer task and the reader loop.
async fn run_socket(
    inner: Arc<RelayInner>,
    websocket: WebSocketStream<TcpStream>,
    host_id: HostId,
    socket_id: u64,
    conn: Option<u32>,
    mut outgoing: mpsc::UnboundedReceiver<Out>,
    mut stall: watch::Receiver<bool>,
) {
    let (mut sink, mut stream) = websocket.split();
    let mut shutdown = inner.shutdown.subscribe();
    let mut writer_shutdown = inner.shutdown.subscribe();
    let writer_stall = stall.clone();
    let writer = tokio::spawn(async move {
        loop {
            let next = tokio::select! {
                next = outgoing.recv() => next,
                _ = writer_shutdown.wait_for(|stop| *stop) => None,
            };
            match next {
                // A stalled socket gets nothing.
                Some(_) if *writer_stall.borrow() => {}
                Some(Out::Frame(message)) => {
                    if sink.send(message).await.is_err() {
                        return;
                    }
                }
                Some(Out::Close(code, reason)) => {
                    let frame = CloseFrame {
                        code: WsCloseCode::from(code),
                        reason: reason.into(),
                    };
                    let _ = sink.send(Message::Close(Some(frame))).await;
                    return;
                }
                None => {
                    let _ = sink.close().await;
                    return;
                }
            }
        }
    });
    let mut close_code = ABNORMAL_CLOSE;
    loop {
        let next = tokio::select! {
            next = stream.next() => Some(next),
            _ = shutdown.wait_for(|stop| *stop) => Some(None),
            _ = stall.wait_for(|stalled| *stalled) => None,
        };
        let next = match next {
            Some(next) => next,
            None => {
                // A stalled socket is not read, so tungstenite sends no pong.
                let _ = shutdown.wait_for(|stop| *stop).await;
                None
            }
        };
        match next {
            Some(Ok(Message::Binary(bytes))) => match conn {
                Some(conn) => inner.device_binary(&host_id, socket_id, conn, &bytes),
                None => inner.host_binary(&host_id, socket_id, &bytes),
            },
            Some(Ok(Message::Text(text))) => match conn {
                Some(conn) => inner.device_text(&host_id, socket_id, conn, text.as_str()),
                None => inner.host_text(&host_id, socket_id, text.as_str()),
            },
            Some(Ok(Message::Close(frame))) => {
                close_code = frame.map_or(1005, |frame| frame.code.into());
                break;
            }
            Some(Ok(_)) => {}
            Some(Err(_)) | None => break,
        }
    }
    inner.socket_gone(&host_id, socket_id, conn, close_code);
    // Give the writer time to answer the close frame, then stop it.
    if tokio::time::timeout(Duration::from_secs(1), writer)
        .await
        .is_err()
    {
        // The writer task ends with the dropped socket.
    }
}

impl RelayInner {
    /// The Worker rate limit: 30 upgrades for each client IP and path kind in 60 seconds.
    fn allow_upgrade(&self, ip: IpAddr, host_path: bool) -> bool {
        let Some(limit) = self.config.upgrade_rate_limit else {
            return true;
        };
        let now = std::time::Instant::now();
        let window = Duration::from_millis(UPGRADE_RATE_WINDOW_MS);
        let mut state = self.lock();
        let times = state.rate.entry((ip, host_path)).or_default();
        while times
            .front()
            .is_some_and(|time| now.duration_since(*time) >= window)
        {
            times.pop_front();
        }
        if times.len() >= limit as usize {
            return false;
        }
        times.push_back(now);
        true
    }

    /// Section 6.2: the host checks in their order, then the steps after them.
    fn accept_host(
        &self,
        host_id: &HostId,
        request: &HostAuthRequest<'_>,
        socket: SocketRef,
    ) -> Result<(), UpgradeError> {
        let now = self.now_ms();
        let mut state = self.lock();
        let object = state.hosts.entry(host_id.clone()).or_default();
        let auth_state = HostAuthState {
            stored_host_key: object.host_key.as_deref(),
            last_auth_ts: object.last_auth_ts,
        };
        let verified = verify_host_auth(host_id, request, &auth_state, now)
            .map_err(|error| error.upgrade_error())?;
        // 1. Store the host key (first time only) and the new last_auth_ts.
        if object.host_key.is_none() {
            object.host_key = Some(verified.host_key);
        }
        object.last_auth_ts = Some(verified.timestamp_ms);
        // 2. Close an older host socket with 4409.
        if let Some(old) = object.host.take() {
            old.close(CloseCode::Replaced.code(), "replaced");
        }
        // 3. Close every device socket with 4408.
        object.close_all_devices();
        // 4 and 5. Accept the socket and send ready.
        object.host = Some(socket);
        object.send_host(&RelayToHost::Ready {
            v: RELAY_PROTOCOL_VERSION,
        });
        Ok(())
    }

    /// Section 6.3: device admission. It returns the connection id.
    fn admit_device(
        &self,
        host_id: &HostId,
        authorization: Option<&str>,
        socket: SocketRef,
    ) -> Result<u32, UpgradeError> {
        // 1. A missing or malformed token.
        let hash = authorization
            .and_then(bearer_token_hash)
            .ok_or(UpgradeError::Unauthorized)?;
        let now = self.now_ms();
        let mut state = self.lock();
        let object = state.hosts.entry(host_id.clone()).or_default();
        // 2. No host socket.
        if object.host.is_none() {
            return Err(UpgradeError::HostOffline);
        }
        // 3. Look up the token hash.
        let peer =
            if let Some((device, _)) = object.devices.iter().find(|(_, stored)| **stored == hash) {
                ConnPeer::Device(device.clone())
            } else {
                let invite = object
                    .invites
                    .iter()
                    .find(|(_, row)| row.hash == hash)
                    .map(|(invite, row)| (invite.clone(), row.clone()));
                match invite {
                    None => return Err(UpgradeError::Unauthorized),
                    Some((invite, row)) if row.exp <= now => {
                        object.invites.remove(&invite);
                        return Err(UpgradeError::InviteExpired);
                    }
                    Some((_, row)) if row.tries == 0 => return Err(UpgradeError::Unauthorized),
                    Some((invite, _)) => {
                        if let Some(row) = object.invites.get_mut(&invite) {
                            row.tries -= 1;
                        }
                        ConnPeer::Invite(invite)
                    }
                }
            };
        // 4. The limit of open device sockets.
        if object.sockets.len() >= self.config.max_device_sockets {
            return Err(UpgradeError::RateLimited);
        }
        // 5. A new connection id.
        let conn = loop {
            let mut bytes = [0u8; 4];
            getrandom::fill(&mut bytes).map_err(|_| UpgradeError::RateLimited)?;
            let conn = u32::from_be_bytes(bytes);
            if conn != 0 && !object.sockets.contains_key(&conn) {
                break conn;
            }
        };
        object.sockets.insert(
            conn,
            DeviceSocket {
                socket,
                peer: peer.clone(),
            },
        );
        // 6. Tell the host.
        let open = ConnOpen {
            conn: ConnId::new(conn).expect("not zero"),
            peer,
        };
        object.send_host(&RelayToHost::ConnOpen(open));
        Ok(conn)
    }

    /// Cleans up after a socket ended on its own.
    fn socket_gone(&self, host_id: &HostId, socket_id: u64, conn: Option<u32>, code: u16) {
        let mut state = self.lock();
        let Some(object) = state.hosts.get_mut(host_id) else {
            return;
        };
        match conn {
            Some(conn) => {
                let current = object
                    .sockets
                    .get(&conn)
                    .is_some_and(|device| device.socket.id == socket_id);
                if current {
                    object.sockets.remove(&conn);
                    if let Some(conn) = ConnId::new(conn) {
                        object.send_host(&RelayToHost::ConnClose { conn, code });
                    }
                }
            }
            None => {
                let current = object
                    .host
                    .as_ref()
                    .is_some_and(|host| host.id == socket_id);
                if current {
                    object.host = None;
                    object.close_all_devices();
                }
            }
        }
    }

    /// Section 6.5: a binary frame of a device goes to the host in a DATA frame.
    fn device_binary(&self, host_id: &HostId, socket_id: u64, conn: u32, bytes: &Bytes) {
        let mut state = self.lock();
        let RelayState { hosts, faults, .. } = &mut *state;
        let Some(object) = hosts.get_mut(host_id) else {
            return;
        };
        if !object
            .sockets
            .get(&conn)
            .is_some_and(|device| device.socket.id == socket_id)
        {
            // The relay closed this socket. A frame in transit is dropped.
            return;
        }
        if bytes.len() > MAX_DEVICE_FRAME {
            object.close_device(conn, CloseCode::TooLarge.code(), "frame too large");
            return;
        }
        if bytes.is_empty() {
            object.close_device(conn, CloseCode::BadFrame.code(), "empty frame");
            return;
        }
        let Some(host) = object.host.clone() else {
            object.close_device(conn, CloseCode::HostGone.code(), "host closed");
            return;
        };
        for message in faults.apply(Direction::ToHost, conn, bytes) {
            let mut frame = Vec::with_capacity(DATA_HEADER_LEN + message.len());
            frame.push(DATA_FRAME_TYPE);
            frame.extend_from_slice(&conn.to_be_bytes());
            frame.extend_from_slice(&message);
            host.send(Message::Binary(frame.into()));
        }
    }

    /// Section 6.5: a device may send the text `ping`. Any other text closes the socket with 4400.
    fn device_text(&self, host_id: &HostId, socket_id: u64, conn: u32, text: &str) {
        let mut state = self.lock();
        let Some(object) = state.hosts.get_mut(host_id) else {
            return;
        };
        let Some(device) = object.sockets.get(&conn) else {
            return;
        };
        if device.socket.id != socket_id {
            return;
        }
        if text.len() > MAX_DEVICE_FRAME {
            object.close_device(conn, CloseCode::TooLarge.code(), "frame too large");
        } else if text == PING_TEXT {
            device.socket.send(Message::text(PONG_TEXT));
        } else {
            object.close_device(conn, CloseCode::BadFrame.code(), "text frame");
        }
    }

    /// Section 6.4: a DATA frame from the host goes to its device socket.
    fn host_binary(&self, host_id: &HostId, socket_id: u64, bytes: &Bytes) {
        let mut state = self.lock();
        let RelayState { hosts, faults, .. } = &mut *state;
        let Some(object) = hosts.get_mut(host_id) else {
            return;
        };
        if !object
            .host
            .as_ref()
            .is_some_and(|host| host.id == socket_id)
        {
            return;
        }
        if bytes.len() > MAX_HOST_DATA_FRAME {
            if let Some(host) = object.host.take() {
                host.close(CloseCode::TooLarge.code(), "frame too large");
            }
            object.close_all_devices();
            return;
        }
        if bytes.first() != Some(&DATA_FRAME_TYPE) {
            object.send_err(None, "bad_frame", "unknown frame type");
            return;
        }
        if bytes.len() <= DATA_HEADER_LEN {
            object.send_err(None, "bad_frame", "DATA frame without a Noise message");
            return;
        }
        let conn = u32::from_be_bytes([bytes[1], bytes[2], bytes[3], bytes[4]]);
        let Some(conn_id) = ConnId::new(conn) else {
            object.send_err(None, "bad_frame", "connection id 0");
            return;
        };
        let Some(device) = object.sockets.get(&conn) else {
            object.send_host(&RelayToHost::ConnClose {
                conn: conn_id,
                code: ABNORMAL_CLOSE,
            });
            return;
        };
        let socket = device.socket.clone();
        let payload = &bytes[DATA_HEADER_LEN..];
        for message in faults.apply(Direction::ToDevice, conn, payload) {
            socket.send(Message::Binary(message.into()));
        }
    }

    /// Section 6.4: a control message from the host.
    fn host_text(&self, host_id: &HostId, socket_id: u64, text: &str) {
        let now = self.now_ms();
        let mut state = self.lock();
        let Some(object) = state.hosts.get_mut(host_id) else {
            return;
        };
        if !object
            .host
            .as_ref()
            .is_some_and(|host| host.id == socket_id)
        {
            return;
        }
        if text.len() > MAX_HOST_CONTROL_FRAME {
            if let Some(host) = object.host.take() {
                host.close(CloseCode::TooLarge.code(), "frame too large");
            }
            object.close_all_devices();
            return;
        }
        let rid = serde_json::from_str::<serde_json::Value>(text)
            .ok()
            .and_then(|value| value.get("rid").and_then(serde_json::Value::as_u64))
            .and_then(|rid| u32::try_from(rid).ok());
        let message = match HostToRelay::from_json(text) {
            Ok(message) => message,
            Err(_) => {
                object.send_err(rid, "bad_request", "invalid control message");
                return;
            }
        };
        let ok = |object: &HostObject, rid: u32| object.send_host(&RelayToHost::Ok { rid });
        match message {
            HostToRelay::InvitePut {
                rid,
                invite,
                hash,
                exp,
                tries,
            } => {
                if !(1..=MAX_INVITE_TRIES).contains(&tries) {
                    object.send_err(Some(rid), "bad_request", "tries must be 1 to 5");
                    return;
                }
                if exp <= now {
                    object.send_err(Some(rid), "bad_request", "exp is in the past");
                    return;
                }
                // An invite lasts 5 minutes at most on the relay clock (section 6.7).
                let exp = exp.min(now + MAX_INVITE_LIFETIME_MS);
                // Expired and used invites are not active.
                object
                    .invites
                    .retain(|_, row| row.exp > now && row.tries > 0);
                if object
                    .invites
                    .iter()
                    .any(|(other, row)| row.hash == hash && *other != invite)
                {
                    object.send_err(Some(rid), "conflict", "another invite has this hash");
                    return;
                }
                let others = object
                    .invites
                    .keys()
                    .filter(|other| **other != invite)
                    .count();
                if others >= MAX_ACTIVE_INVITES {
                    object.send_err(Some(rid), "invite_limit", "4 invites are active");
                    return;
                }
                object
                    .invites
                    .insert(invite, InviteRow { hash, exp, tries });
                ok(object, rid);
            }
            HostToRelay::InviteDel { rid, invite } => {
                object.invites.remove(&invite);
                ok(object, rid);
            }
            HostToRelay::DevicePut { rid, device, hash } => {
                if object
                    .devices
                    .iter()
                    .any(|(other, stored)| *stored == hash && *other != device)
                {
                    object.send_err(Some(rid), "conflict", "another device has this hash");
                    return;
                }
                let others = object
                    .devices
                    .keys()
                    .filter(|other| **other != device)
                    .count();
                if others >= MAX_PAIRED_DEVICES {
                    object.send_err(Some(rid), "device_limit", "32 devices are paired");
                    return;
                }
                object.devices.insert(device, hash);
                ok(object, rid);
            }
            HostToRelay::DeviceDel { rid, device } => {
                object.devices.remove(&device);
                object.close_removed_devices(|other| *other != device);
                ok(object, rid);
            }
            HostToRelay::DevicesSync { rid, devices } => {
                if devices.len() > MAX_PAIRED_DEVICES {
                    object.send_err(Some(rid), "device_limit", "more than 32 devices");
                    return;
                }
                let mut wanted = BTreeMap::new();
                for entry in devices {
                    let duplicate = wanted.values().any(|hash| *hash == entry.hash);
                    if duplicate || wanted.insert(entry.device, entry.hash).is_some() {
                        object.send_err(Some(rid), "bad_request", "duplicate device or hash");
                        return;
                    }
                }
                object.devices = wanted;
                let kept: Vec<DeviceId> = object.devices.keys().cloned().collect();
                object.close_removed_devices(|device| kept.contains(device));
                ok(object, rid);
            }
            HostToRelay::ConnClose { conn, code, reason } => {
                if !(4000..=4999).contains(&code) {
                    object.send_err(rid, "bad_request", "code must be 4000 to 4999");
                    return;
                }
                let reason = reason
                    .filter(|reason| reason.len() <= MAX_CLOSE_REASON_BYTES)
                    .unwrap_or_default();
                // The host asked for the close, so the relay sends no conn_close back.
                if let Some(device) = object.sockets.remove(&conn.get()) {
                    device.socket.close(code, &reason);
                }
            }
            HostToRelay::Unknown => object.send_err(rid, "unknown_type", "unknown message type"),
        }
    }
}
