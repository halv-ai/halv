//! The host engine: the desktop side of the remote protocol (feature `host`).
//!
//! [`HostEngine::start`] connects the host socket to the relay and serves paired devices.
//! The desktop gateway gives the engine three parts:
//!
//! - a [`HostBackend`]: the panes, the terminal output, snapshots, replays, and input,
//! - a [`DeviceStore`]: the device registry (section 9),
//! - a [`PairingApprover`]: the approval dialog of a pairing (section 7.3).
//!
//! The engine follows sections 6.2 to 6.4, 7.3, 7.4, 8.3 to 8.8, and 9:
//!
//! - It signs the host upgrade, sends `devices_sync` after every `ready`, and reconnects
//!   with full jitter from 1 to 60 seconds.
//! - It runs one handshake for each connection, checks the device before message 2,
//!   and closes a connection with the close codes of section 6.6.
//! - It marks a device as connected only after `hello`, the first valid transport message,
//!   so a replayed message 1 changes no state (section 7.4).
//! - It keeps at most 8 connections and at most 4 open handshakes. It closes extras with `4429`.
//! - It keeps at most one pairing offer. The PSK stays in memory until the offer ends.
//! - It streams terminal output with the batching, the ack window, and the resume rules of section 8.8.
//!
//! The engine calls the backend and the store in `spawn_blocking`, so a slow call never
//! blocks a tokio worker. [`HostBackend::host_info`], [`HostBackend::agent_status`],
//! [`HostBackend::pane_changes`], and [`HostBackend::output`] must return at once.

mod conn;
mod engine;
mod panes;
mod paste;
mod stream;

use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{broadcast, watch};
use zeroize::Zeroizing;

use crate::framing::MAX_CONNECTION_LIFETIME_MS;
use crate::ids::{ConnId, DeviceId, InviteId};
use crate::keys::{HostIdentity, TokenHash};
use crate::net::ConnectError;
use crate::noise::{ConfirmationCode, MIN_PROTOCOL_VERSION, PROTOCOL_VERSION};
use crate::pairing::{is_valid_relay_url, OFFER_LIFETIME_MS};
use crate::relay_wire::{IDLE_TIMEOUT_MS, PAIRING_CLOSE_DELAY_MS, PING_INTERVAL_MS};
use crate::rpc::{HostInfo, Pane, Platform, APPROVAL_TIMEOUT_MS};

pub use paste::encode_paste;

/// The terminal output of a pane, from the output tap of the backend.
///
/// `seq` is the sequence number of the chunk in the ring of the pane.
/// The numbers of one pane only go up.
#[derive(Clone, PartialEq, Eq)]
pub enum OutputEvent {
    /// New output.
    Data {
        /// The terminal session id of the pane.
        session: String,
        /// The sequence number of the chunk.
        seq: u64,
        /// The output bytes.
        data: Arc<[u8]>,
    },
    /// The PTY size changed.
    Resized {
        /// The terminal session id of the pane.
        session: String,
        /// The new columns.
        cols: u16,
        /// The new rows.
        rows: u16,
    },
    /// The process exited. The pane stays.
    Exit {
        /// The terminal session id of the pane.
        session: String,
        /// The exit code, if the process exited normally.
        code: Option<i32>,
        /// The signal name, if a signal stopped the process.
        signal: Option<String>,
    },
    /// The pane went away.
    Closed {
        /// The terminal session id of the pane.
        session: String,
    },
}

impl OutputEvent {
    /// Returns the terminal session id of the event.
    pub fn session(&self) -> &str {
        match self {
            OutputEvent::Data { session, .. }
            | OutputEvent::Resized { session, .. }
            | OutputEvent::Exit { session, .. }
            | OutputEvent::Closed { session } => session,
        }
    }
}

impl fmt::Debug for OutputEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OutputEvent::Data { session, seq, data } => f
                .debug_struct("Data")
                .field("session", session)
                .field("seq", seq)
                .field("data_len", &data.len())
                .finish(),
            OutputEvent::Resized {
                session,
                cols,
                rows,
            } => f
                .debug_struct("Resized")
                .field("session", session)
                .field("cols", cols)
                .field("rows", rows)
                .finish(),
            OutputEvent::Exit {
                session,
                code,
                signal,
            } => f
                .debug_struct("Exit")
                .field("session", session)
                .field("code", code)
                .field("signal", signal)
                .finish(),
            OutputEvent::Closed { session } => {
                f.debug_struct("Closed").field("session", session).finish()
            }
        }
    }
}

/// The full ring of a pane (section 8.8).
#[derive(Clone, PartialEq, Eq)]
pub struct Snapshot {
    /// The sequence number of the last chunk in the ring.
    pub seq: u64,
    /// The columns of the PTY.
    pub cols: u16,
    /// The rows of the PTY.
    pub rows: u16,
    /// The ring bytes.
    pub data: Vec<u8>,
}

impl fmt::Debug for Snapshot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Snapshot")
            .field("seq", &self.seq)
            .field("cols", &self.cols)
            .field("rows", &self.rows)
            .field("data_len", &self.data.len())
            .finish()
    }
}

/// One chunk of the ring of a pane.
#[derive(Clone, PartialEq, Eq)]
pub struct Chunk {
    /// The sequence number of the chunk.
    pub seq: u64,
    /// The chunk bytes.
    pub data: Vec<u8>,
}

impl fmt::Debug for Chunk {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Chunk")
            .field("seq", &self.seq)
            .field("data_len", &self.data.len())
            .finish()
    }
}

/// The chunks of a ring after a sequence number, for a resume with `fromSeq`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Replay {
    /// The columns of the PTY.
    pub cols: u16,
    /// The rows of the PTY.
    pub rows: u16,
    /// Every chunk after the requested sequence number, in order. It can be empty.
    pub chunks: Vec<Chunk>,
}

/// The error of a [`HostBackend::write`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum WriteError {
    /// The pane does not exist.
    #[error("the pane does not exist")]
    NotFound,
    /// The write failed. The text must not hold the written bytes.
    #[error("the write failed: {0}")]
    Failed(String),
}

/// The terminal side of the desktop app.
///
/// The engine calls [`HostBackend::host_info`], [`HostBackend::agent_status`],
/// [`HostBackend::pane_changes`], and [`HostBackend::output`] directly, so they must return at once.
/// It calls the other methods in `spawn_blocking`.
///
/// Rules for the sequence numbers:
///
/// - The numbers of one pane only go up.
/// - Append a chunk to the ring before you send its [`OutputEvent::Data`],
///   so a snapshot always holds every chunk that the output tap already sent.
pub trait HostBackend: Send + Sync + 'static {
    /// Returns the host name, the app name and version, and the operating system.
    fn host_info(&self) -> HostInfo;

    /// Returns true when [`Pane::status`] is available.
    /// The host then adds the capability `agent-status.v1`.
    fn agent_status(&self) -> bool {
        false
    }

    /// Returns the panes.
    fn panes(&self) -> Vec<Pane>;

    /// Returns a receiver that gets a value each time the pane list changes.
    fn pane_changes(&self) -> broadcast::Receiver<()>;

    /// Returns the full ring of a pane, or `None` when the pane does not exist.
    fn snapshot(&self, session: &str) -> Option<Snapshot>;

    /// Returns every chunk after `after_seq`.
    ///
    /// Return `None` when the ring no longer holds every chunk after `after_seq`,
    /// when `after_seq` is newer than the last chunk, or when the pane does not exist.
    /// The engine then sends a snapshot.
    fn replay_since(&self, session: &str, after_seq: u64) -> Option<Replay>;

    /// Returns a receiver of the output events of all panes.
    ///
    /// When a receiver falls behind, the engine sends a new snapshot,
    /// so the tap never needs to wait for a slow device.
    fn output(&self) -> broadcast::Receiver<OutputEvent>;

    /// Writes input to the PTY of a pane.
    fn write(&self, session: &str, data: &[u8]) -> Result<(), WriteError>;

    /// Returns the bracketed paste mode of a pane, if the backend knows it.
    ///
    /// The engine also follows `ESC [ ? 2004 h` and `ESC [ ? 2004 l` in the output (section 8.8).
    /// A value from the backend wins, because the scanner cannot see mode changes
    /// that left the ring before the engine started.
    /// When nobody knows the mode, the engine uses bracketed paste for text with a line break.
    fn bracketed_paste(&self, session: &str) -> Option<bool> {
        let _ = session;
        None
    }
}

/// An error of the device registry.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("device store error: {0}")]
pub struct StoreError(pub String);

/// A paired device in the registry (section 9).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredDevice {
    /// The device id.
    pub id: DeviceId,
    /// The device name from `pair.request`.
    pub name: String,
    /// The platform from `pair.request`.
    pub platform: Platform,
    /// The model from `pair.request`, if any.
    pub model: Option<String>,
    /// The Noise static public key of the device. It is unique in the registry.
    pub noise_public: [u8; 32],
    /// `b64u(SHA-256(relay_token))`.
    pub token_hash: TokenHash,
    /// The pairing time in epoch milliseconds.
    pub created_at_ms: u64,
    /// The start of the last session in epoch milliseconds.
    pub last_seen_at_ms: Option<u64>,
    /// The revocation time in epoch milliseconds.
    pub revoked_at_ms: Option<u64>,
}

impl StoredDevice {
    /// Returns true after a revoke.
    pub fn is_revoked(&self) -> bool {
        self.revoked_at_ms.is_some()
    }
}

/// A device to add to the registry after the user allows a pairing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewDevice {
    /// The new device id. The engine makes it.
    pub id: DeviceId,
    /// The device name.
    pub name: String,
    /// The platform.
    pub platform: Platform,
    /// The model, if any.
    pub model: Option<String>,
    /// The Noise static public key of the device.
    pub noise_public: [u8; 32],
    /// `b64u(SHA-256(relay_token))`.
    pub token_hash: TokenHash,
    /// The pairing time in epoch milliseconds.
    pub created_at_ms: u64,
}

/// The device registry of the host (section 9).
///
/// The registry decides revocation: a revoked key fails at message 1,
/// also when the relay still admits its token.
pub trait DeviceStore: Send + Sync + 'static {
    /// Returns every device, also the revoked ones.
    fn list(&self) -> Result<Vec<StoredDevice>, StoreError>;

    /// Returns the device with this Noise public key, also a revoked one.
    fn find_by_noise_key(&self, key: &[u8; 32]) -> Result<Option<StoredDevice>, StoreError>;

    /// Adds a device. The Noise public key must be unique.
    fn insert(&self, device: NewDevice) -> Result<StoredDevice, StoreError>;

    /// Sets `revoked_at`. Returns false when the device does not exist.
    /// A second revoke of the same device returns true and keeps the first time.
    fn revoke(&self, id: &DeviceId, at_ms: u64) -> Result<bool, StoreError>;

    /// Sets `last_seen_at`.
    fn touch_last_seen(&self, id: &DeviceId, at_ms: u64) -> Result<(), StoreError>;
}

/// A pairing request that waits for the user (section 7.3, step 8).
///
/// The approval dialog shows the device name, the platform, and the confirmation code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairRequestInfo {
    /// The device name (1 to 64 characters).
    pub name: String,
    /// The platform.
    pub platform: Platform,
    /// The model, if the device sent one.
    pub model: Option<String>,
    /// The app name and version of the device.
    pub app: String,
    /// The confirmation code. The phone shows the same code.
    pub code: ConfirmationCode,
    /// The invite of the pairing offer.
    pub invite: InviteId,
}

/// A boxed future, for the trait methods that the engine calls through `dyn`.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Asks the user to allow a device.
pub trait PairingApprover: Send + Sync + 'static {
    /// Returns true when the user allows the device.
    ///
    /// The engine stops waiting after the approval timeout (120 seconds), when the device
    /// leaves, or when the offer ends. It then drops the future, and the dialog must close.
    fn approve(&self, request: PairRequestInfo) -> BoxFuture<'_, bool>;
}

/// The settings of the host engine.
#[derive(Debug, Clone)]
pub struct HostConfig {
    /// The relay base URL, for example `wss://relay.halv.ai` (section 5.1).
    pub relay_url: String,
    /// The host identity from the keychain.
    pub identity: Arc<HostIdentity>,
    /// The time that the user has to allow a device. No answer means "deny".
    pub approval_timeout: Duration,
    /// The time for message 1 after `conn_open`, and for `pair.request` after message 2.
    pub handshake_timeout: Duration,
    /// The time for `hello` after message 2 of a session (section 7.4, step 7).
    /// Without `hello`, the host closes the connection with `4400`.
    pub hello_timeout: Duration,
    /// The wait after the relay closed the host socket with `4409` (section 6.6).
    /// After a second `4409`, the engine stops with [`HostStatus::Replaced`].
    pub replaced_retry_delay: Duration,
    /// The time for the TCP, TLS, and WebSocket handshakes of the host socket.
    pub connect_timeout: Duration,
    /// The time for the `ok` or `err` answer of a control message.
    pub control_timeout: Duration,
    /// The first ceiling of the reconnect backoff.
    pub backoff_min: Duration,
    /// The largest ceiling of the reconnect backoff.
    pub backoff_max: Duration,
    /// The engine sends a WebSocket ping this often (section 6.8).
    pub ping_interval: Duration,
    /// The engine reconnects after this time without a received frame (section 6.8).
    pub idle_timeout: Duration,
    /// A connection closes after this time, because version 1 has no rekey (section 7.7).
    pub connection_lifetime: Duration,
    /// The host closes a finished pairing connection with `4410` after this time.
    pub pairing_close_delay: Duration,
    /// The lifetime of a pairing offer. The engine uses 5 minutes at most.
    pub offer_lifetime: Duration,
    /// The lowest protocol version of the host. Change it only in tests.
    pub min_version: u64,
    /// The highest protocol version of the host. Change it only in tests.
    pub max_version: u64,
}

impl HostConfig {
    /// Makes the settings of the contract for a relay URL and an identity.
    pub fn new(relay_url: impl Into<String>, identity: Arc<HostIdentity>) -> HostConfig {
        HostConfig {
            relay_url: relay_url.into(),
            identity,
            approval_timeout: Duration::from_millis(APPROVAL_TIMEOUT_MS),
            handshake_timeout: Duration::from_secs(10),
            hello_timeout: Duration::from_secs(15),
            replaced_retry_delay: Duration::from_secs(5 * 60),
            connect_timeout: Duration::from_secs(15),
            control_timeout: Duration::from_secs(10),
            backoff_min: Duration::from_secs(1),
            backoff_max: Duration::from_secs(60),
            ping_interval: Duration::from_millis(PING_INTERVAL_MS),
            idle_timeout: Duration::from_millis(IDLE_TIMEOUT_MS),
            connection_lifetime: Duration::from_millis(MAX_CONNECTION_LIFETIME_MS),
            pairing_close_delay: Duration::from_millis(PAIRING_CLOSE_DELAY_MS),
            offer_lifetime: Duration::from_millis(OFFER_LIFETIME_MS),
            min_version: MIN_PROTOCOL_VERSION,
            max_version: PROTOCOL_VERSION,
        }
    }
}

/// Why the host socket is not open.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum OfflineReason {
    /// The connection failed before the WebSocket opened.
    Connect(ConnectError),
    /// The relay closed the host socket with this code, or without a code.
    Closed(Option<u16>),
    /// The socket failed.
    Network(String),
    /// No frame arrived for the idle timeout.
    IdleTimeout,
}

/// The state of the host engine.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum HostStatus {
    /// The engine connects the host socket.
    Connecting,
    /// The host socket is open and the relay sent `ready`.
    Online,
    /// The host socket is closed. The engine tries again after `retry_in`.
    Offline {
        /// Why the socket is closed.
        reason: OfflineReason,
        /// The time until the next attempt.
        retry_in: Duration,
    },
    /// A newer host socket with the same identity replaced this one (`4409`) for the second time.
    /// The engine does not connect again, so two desktops do not fight.
    /// After the first `4409`, the engine is [`HostStatus::Offline`] and tries again after 5 minutes,
    /// because the close code comes from the untrusted relay (section 6.6).
    Replaced,
    /// The engine stopped after [`HostHandle::shutdown`].
    Stopped,
}

/// A pairing offer to show as a QR code (section 5.1).
///
/// The URI holds the pairing secret. Show it only as a QR code or as text to copy. Never log it.
pub struct PairingOfferInfo {
    /// The `halv://pair?p=...` URI.
    pub uri: Zeroizing<String>,
    /// The expiry time in epoch milliseconds.
    pub expires_at_ms: u64,
    /// The invite id of the offer.
    pub invite: InviteId,
}

impl fmt::Debug for PairingOfferInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PairingOfferInfo")
            .field("expires_at_ms", &self.expires_at_ms)
            .field("invite", &self.invite)
            .finish_non_exhaustive()
    }
}

/// A device with an open session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectedDevice {
    /// The device id.
    pub device: DeviceId,
    /// The connection id of the session.
    pub conn: ConnId,
    /// The start of the session in epoch milliseconds.
    pub since_ms: u64,
}

/// An error of a [`HostHandle`] call.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum HostError {
    /// The host socket is not open.
    #[error("the relay is not connected")]
    Offline,
    /// The relay answered a control message with `err`.
    #[error("the relay refused the request ({code})")]
    Relay {
        /// The error code of the relay.
        code: String,
    },
    /// The relay did not answer in time.
    #[error("the relay did not answer in time")]
    Timeout,
    /// The device is not in the registry.
    #[error("the device is not known")]
    UnknownDevice,
    /// The relay URL is not valid (section 5.1).
    #[error("the relay URL is not valid")]
    InvalidRelayUrl,
    /// The device registry failed.
    #[error(transparent)]
    Store(#[from] StoreError),
    /// The engine stopped.
    #[error("the host engine stopped")]
    Stopped,
    /// The protocol code failed.
    #[error(transparent)]
    Protocol(#[from] crate::Error),
}

/// Starts the host engine.
pub struct HostEngine;

impl HostEngine {
    /// Starts the engine on the current tokio runtime.
    ///
    /// The engine connects at once and reconnects until [`HostHandle::shutdown`].
    /// It fails only when the relay URL breaks the rules of section 5.1.
    pub fn start(
        config: HostConfig,
        backend: Arc<dyn HostBackend>,
        store: Arc<dyn DeviceStore>,
        approver: Arc<dyn PairingApprover>,
    ) -> Result<HostHandle, HostError> {
        if !is_valid_relay_url(&config.relay_url) {
            return Err(HostError::InvalidRelayUrl);
        }
        let shared = engine::Shared::start(config, backend, store, approver);
        Ok(HostHandle {
            owner: Arc::new(HandleOwner { shared }),
        })
    }
}

/// Stops the engine when the last [`HostHandle`] drops.
struct HandleOwner {
    shared: Arc<engine::Shared>,
}

impl Drop for HandleOwner {
    fn drop(&mut self) {
        self.shared.request_stop();
    }
}

/// Controls a running host engine. Clones control the same engine.
/// The engine stops when the last clone drops.
#[derive(Clone)]
pub struct HostHandle {
    owner: Arc<HandleOwner>,
}

impl fmt::Debug for HostHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HostHandle")
            .field("status", &self.status())
            .finish_non_exhaustive()
    }
}

impl HostHandle {
    /// Returns the current state.
    pub fn status(&self) -> HostStatus {
        self.owner.shared.status.borrow().clone()
    }

    /// Returns a receiver that sees each change of the state.
    pub fn subscribe_status(&self) -> watch::Receiver<HostStatus> {
        self.owner.shared.status.subscribe()
    }

    /// Makes a new pairing offer and registers its invite with the relay.
    ///
    /// A new offer ends the previous one. The offer allows 5 tries and lasts 5 minutes.
    /// It fails when the host socket is not open or when the relay refuses the invite.
    pub async fn start_pairing(&self) -> Result<PairingOfferInfo, HostError> {
        self.owner.shared.start_pairing().await
    }

    /// Ends the active pairing offer and deletes its invite. A waiting approval gets `expired`.
    pub async fn cancel_pairing(&self) {
        self.owner.shared.end_offer(None).await;
    }

    /// Revokes a device (section 9).
    ///
    /// The engine sets `revoked_at`, sends `device.revoked` on each open session of the device,
    /// sends `device_del`, and closes those connections with `4401`.
    /// From the start of the call, the sessions of the device take no input.
    /// Without a host socket, the next `devices_sync` removes the device from the relay.
    pub async fn revoke_device(&self, id: &DeviceId) -> Result<(), HostError> {
        self.owner.shared.revoke_device(id, true, None).await
    }

    /// Sends the device list of the registry to the relay (`devices_sync`).
    ///
    /// The engine does this after every `ready`. Call it after a change of the registry
    /// outside the engine, for example after an identity reset.
    pub async fn sync_devices(&self) -> Result<(), HostError> {
        self.owner.shared.sync_devices().await
    }

    /// Returns the devices with an open session.
    pub fn connected_devices(&self) -> Vec<ConnectedDevice> {
        self.owner.shared.connected_devices()
    }

    /// Stops the engine and closes the host socket. The relay then closes every device socket.
    pub async fn shutdown(&self) {
        self.owner.shared.shutdown().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The gateway keeps these values in Tauri state and moves them between threads.
    #[test]
    fn public_types_are_send_and_sync() {
        fn check<T: Send + Sync>() {}
        check::<HostHandle>();
        check::<HostConfig>();
        check::<HostStatus>();
        check::<PairingOfferInfo>();
        check::<PairRequestInfo>();
        check::<OutputEvent>();
        check::<HostError>();
    }

    #[test]
    fn debug_output_hides_the_offer_and_terminal_bytes() {
        let info = PairingOfferInfo {
            uri: Zeroizing::new("halv://pair?p=c2VjcmV0".to_owned()),
            expires_at_ms: 1,
            invite: InviteId::parse("IIIIIIIIIIIIIIIIIIIIII").unwrap(),
        };
        assert!(!format!("{info:?}").contains("halv://"));
        let event = OutputEvent::Data {
            session: "term-1".to_owned(),
            seq: 3,
            data: Arc::from(&b"password"[..]),
        };
        assert!(!format!("{event:?}").contains("password"));
        let snapshot = Snapshot {
            seq: 1,
            cols: 80,
            rows: 24,
            data: b"secret".to_vec(),
        };
        assert!(!format!("{snapshot:?}").contains("secret"));
    }

    #[test]
    fn default_config_follows_the_contract() {
        let identity = Arc::new(HostIdentity::from_secrets([1; 32], [2; 32], 0));
        let config = HostConfig::new("wss://relay.halv.ai", identity);
        assert_eq!(config.approval_timeout, Duration::from_secs(120));
        assert_eq!(config.backoff_min, Duration::from_secs(1));
        assert_eq!(config.backoff_max, Duration::from_secs(60));
        assert_eq!(config.ping_interval, Duration::from_secs(20));
        assert_eq!(config.idle_timeout, Duration::from_secs(60));
        assert_eq!(
            config.connection_lifetime,
            Duration::from_secs(24 * 60 * 60)
        );
        assert_eq!(config.pairing_close_delay, Duration::from_secs(10));
        assert_eq!(config.offer_lifetime, Duration::from_secs(5 * 60));
        assert_eq!(config.hello_timeout, Duration::from_secs(15));
        assert_eq!(config.replaced_retry_delay, Duration::from_secs(5 * 60));
    }
}
