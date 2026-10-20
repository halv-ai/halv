//! The client engine: the phone side of the remote protocol (feature `client`).
//!
//! - [`start_pairing`] and [`pair`] pair the device with a host from an offer URI (section 7.3).
//!   The result is a [`PairedHost`]: the material that the app stores.
//! - [`ClientSession::connect`] runs a session with automatic reconnects (sections 7.4, 8, and 10).
//!   A [`ClientHandle`] sends requests. A [`TerminalSubscription`] gets the output of one pane
//!   and acknowledges it (section 8.8).
//!
//! The engine never deletes keys. It reports [`ClientStatus::Revoked`] after the encrypted
//! event `device.revoked`, and the app then removes the host. A relay close code or an HTTP
//! error never does this (section 10).

mod pairing;
mod session;

use std::fmt;
use std::time::Duration;

use serde::de::{self, Deserializer};
use serde::{Deserialize, Serialize, Serializer};
use zeroize::Zeroizing;

use crate::b64u;
use crate::framing::MAX_CONNECTION_LIFETIME_MS;
use crate::ids::{DeviceId, HostId};
use crate::keys::{DeviceKeys, RelayToken};
use crate::net::ConnectError;
use crate::noise::{MIN_PROTOCOL_VERSION, PROTOCOL_VERSION};
use crate::relay_wire::{IDLE_TIMEOUT_MS, PING_INTERVAL_MS};
use crate::rpc::{
    caps, HelloResult, PaneList, Platform, RpcError, ACK_INTERVAL_MS, PAIR_REQUEST_TIMEOUT_MS,
    REQUEST_TIMEOUT_MS,
};
use crate::secret::SecretText;

pub use pairing::{pair, start_pairing, PendingPairing};
pub use session::{ClientHandle, ClientSession, TerminalSubscription};

/// The device facts that a pairing sends to the host (section 8.3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceInfo {
    /// The device name, 1 to 64 characters. The approval dialog shows it.
    pub name: String,
    /// The platform.
    pub platform: Platform,
    /// The device model, if known.
    pub model: Option<String>,
    /// The app name and version, for example `halv-mobile/0.1.0`.
    pub app: String,
}

/// The settings of the client engine.
#[derive(Debug, Clone)]
pub struct ClientConfig {
    /// The app name and version for `hello` and the handshake payload.
    pub app: String,
    /// The platform for `hello`.
    pub platform: Platform,
    /// The device capabilities for `hello`.
    pub caps: Vec<String>,
    /// The time for the TCP, TLS, and WebSocket handshakes.
    pub connect_timeout: Duration,
    /// The time for handshake message 2 after message 1.
    pub handshake_timeout: Duration,
    /// The time that a request waits for its response (section 8.1).
    pub request_timeout: Duration,
    /// The time that `pair.request` waits for its response (section 8.1).
    pub pair_request_timeout: Duration,
    /// The first ceiling of the reconnect backoff (section 10).
    pub backoff_min: Duration,
    /// The largest ceiling of the reconnect backoff.
    pub backoff_max: Duration,
    /// The shortest wait after `503 host_offline` (section 10).
    pub host_offline_retry_min: Duration,
    /// The longest wait after `503 host_offline`.
    pub host_offline_retry_max: Duration,
    /// The pause of automatic retries after `4401` or HTTP `401` (section 6.6).
    pub not_authorized_pause: Duration,
    /// The engine sends a WebSocket ping this often (section 6.8).
    pub ping_interval: Duration,
    /// The engine reconnects after this time without a received frame.
    pub idle_timeout: Duration,
    /// The time for a ping or a pong after [`ClientHandle::reconnect_now`] on an open session.
    /// Without one, the engine connects again at once.
    pub probe_timeout: Duration,
    /// The engine sends at most one `terminal.ack` per subscription in this time (section 8.8).
    pub ack_interval: Duration,
    /// A connection closes after this time, because version 1 has no rekey (section 7.7).
    pub connection_lifetime: Duration,
    /// The lowest protocol version of the device. Change it only in tests.
    pub min_version: u64,
    /// The highest protocol version of the device. Change it only in tests.
    pub max_version: u64,
}

impl ClientConfig {
    /// Makes the settings of the contract for an app and a platform.
    pub fn new(app: impl Into<String>, platform: Platform) -> ClientConfig {
        ClientConfig {
            app: app.into(),
            platform,
            caps: vec![caps::TERMINAL_V1.to_owned(), caps::PANES_V1.to_owned()],
            connect_timeout: Duration::from_secs(15),
            handshake_timeout: Duration::from_secs(15),
            request_timeout: Duration::from_millis(REQUEST_TIMEOUT_MS),
            pair_request_timeout: Duration::from_millis(PAIR_REQUEST_TIMEOUT_MS),
            backoff_min: Duration::from_millis(500),
            backoff_max: Duration::from_secs(30),
            host_offline_retry_min: Duration::from_secs(10),
            host_offline_retry_max: Duration::from_secs(30),
            not_authorized_pause: Duration::from_secs(5 * 60),
            ping_interval: Duration::from_millis(PING_INTERVAL_MS),
            idle_timeout: Duration::from_millis(IDLE_TIMEOUT_MS),
            probe_timeout: Duration::from_secs(5),
            ack_interval: Duration::from_millis(ACK_INTERVAL_MS),
            connection_lifetime: Duration::from_millis(MAX_CONNECTION_LIFETIME_MS),
            min_version: MIN_PROTOCOL_VERSION,
            max_version: PROTOCOL_VERSION,
        }
    }
}

/// The version of the [`PairedHost`] JSON.
const PAIRED_HOST_VERSION: u64 = 1;

/// What the device stores for one paired host (section 10).
///
/// The device Noise secret and the relay token zeroize on drop, and `Debug` hides them.
/// Store the JSON of [`PairedHost::to_json`] only in "this device only" items of the iOS
/// Keychain or the Android Keystore (section 4.3), or store the secrets there and the
/// other fields in the app data directory.
#[derive(Clone)]
pub struct PairedHost {
    relay_url: String,
    host_id: HostId,
    host_noise_public: [u8; 32],
    host_name: String,
    device_id: DeviceId,
    keys: DeviceKeys,
}

impl PairedHost {
    /// Makes the record from stored parts.
    pub fn new(
        relay_url: impl Into<String>,
        host_id: HostId,
        host_noise_public: [u8; 32],
        host_name: impl Into<String>,
        device_id: DeviceId,
        keys: DeviceKeys,
    ) -> PairedHost {
        PairedHost {
            relay_url: relay_url.into(),
            host_id,
            host_noise_public,
            host_name: host_name.into(),
            device_id,
            keys,
        }
    }

    /// Returns the relay base URL.
    pub fn relay_url(&self) -> &str {
        &self.relay_url
    }

    /// Returns the host id.
    pub fn host_id(&self) -> &HostId {
        &self.host_id
    }

    /// Returns the pinned host Noise public key.
    pub fn host_noise_public(&self) -> &[u8; 32] {
        &self.host_noise_public
    }

    /// Returns the host name.
    pub fn host_name(&self) -> &str {
        &self.host_name
    }

    /// Returns the device id that the host made.
    pub fn device_id(&self) -> &DeviceId {
        &self.device_id
    }

    /// Returns the device keys: the Noise key pair and the relay token. Never log them.
    pub fn keys(&self) -> &DeviceKeys {
        &self.keys
    }

    /// Writes the JSON form. The text holds both secrets.
    pub fn to_json(&self) -> Zeroizing<String> {
        let mut json = Vec::with_capacity(512 + self.relay_url.len() + 6 * self.host_name.len());
        serde_json::to_writer(&mut json, self).expect("the record always serializes");
        Zeroizing::new(String::from_utf8(json).expect("JSON is UTF-8"))
    }

    /// Reads the JSON form.
    pub fn from_json(text: &str) -> crate::Result<PairedHost> {
        if !crate::is_json_object(text.as_bytes()) {
            return Err(crate::Error::Invalid("paired host JSON"));
        }
        serde_json::from_str(text).map_err(|_| crate::Error::Invalid("paired host JSON"))
    }
}

impl fmt::Debug for PairedHost {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PairedHost")
            .field("relay_url", &self.relay_url)
            .field("host_id", &self.host_id)
            .field("host_name", &self.host_name)
            .field("device_id", &self.device_id)
            .field("keys", &self.keys)
            .finish_non_exhaustive()
    }
}

impl Serialize for PairedHost {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct Wire<'a> {
            v: u64,
            relay: &'a str,
            host: &'a HostId,
            host_key: &'a str,
            host_name: &'a str,
            device: &'a DeviceId,
            device_key: &'a str,
            token: &'a str,
        }

        let host_key = b64u::encode(&self.host_noise_public);
        let device_key = b64u::encode_secret(self.keys.noise_secret());
        let token = self.keys.relay_token().to_b64u();
        Wire {
            v: PAIRED_HOST_VERSION,
            relay: &self.relay_url,
            host: &self.host_id,
            host_key: &host_key,
            host_name: &self.host_name,
            device: &self.device_id,
            device_key: &device_key,
            token: &token,
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for PairedHost {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Wire {
            v: u64,
            relay: String,
            host: HostId,
            host_key: String,
            host_name: String,
            device: DeviceId,
            device_key: SecretText,
            token: SecretText,
        }

        let wire = Wire::deserialize(deserializer)?;
        if wire.v != PAIRED_HOST_VERSION {
            return Err(de::Error::custom("unknown paired host version"));
        }
        if !crate::pairing::is_valid_relay_url(&wire.relay) {
            return Err(de::Error::custom("invalid relay URL"));
        }
        let host_key = b64u::decode_array::<32>(&wire.host_key)
            .ok_or_else(|| de::Error::custom("invalid host key"))?;
        let device_key = b64u::decode_array::<32>(&wire.device_key.0)
            .ok_or_else(|| de::Error::custom("invalid device key"))?;
        let token = RelayToken::from_b64u(&wire.token.0)
            .map_err(|_| de::Error::custom("invalid relay token"))?;
        Ok(PairedHost {
            relay_url: wire.relay,
            host_id: wire.host,
            host_noise_public: *host_key,
            host_name: wire.host_name,
            device_id: wire.device,
            keys: DeviceKeys::from_parts(*device_key, token),
        })
    }
}

/// Why a pairing failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum PairError {
    /// The offer breaks a rule of section 5.1, expired, or needs a newer app
    /// ([`crate::Error::UpdateRequired`]).
    #[error("the pairing code is not valid: {0}")]
    InvalidOffer(crate::Error),
    /// The device name is not 1 to 64 characters.
    #[error("the device name must have 1 to 64 characters")]
    InvalidDeviceName,
    /// The invite expired (HTTP `401 invite_expired`), or the offer ended (`expired`).
    #[error("the pairing code expired")]
    Expired,
    /// The relay or the host refused the device: an unknown or used invite (HTTP `401`),
    /// or a failed handshake (`4401`).
    #[error("the desktop did not accept the pairing")]
    NotAuthorized,
    /// No host socket is open (HTTP `503 host_offline`).
    #[error("the desktop is offline")]
    HostOffline,
    /// The relay rate limited the device.
    #[error("too many attempts; try again later")]
    RateLimited,
    /// The protocol versions do not overlap. The user must update an app.
    #[error("the desktop app and the phone app versions do not match")]
    Incompatible,
    /// The user denied the device on the desktop.
    #[error("the desktop denied the pairing")]
    Denied,
    /// Nobody answered on the desktop in time.
    #[error("nobody answered on the desktop in time")]
    Timeout,
    /// Another device waits for approval.
    #[error("the desktop is busy with another pairing")]
    Busy,
    /// The host refused with another error.
    #[error("the desktop refused the pairing: {0}")]
    Rejected(RpcError),
    /// The connection failed or ended.
    #[error("network error: {0}")]
    Network(String),
    /// The host sent a message that breaks the protocol.
    #[error("protocol error: {0}")]
    Protocol(String),
}

/// The state of a client session.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ClientStatus {
    /// The engine connects.
    Connecting,
    /// The session is open. The value is the `hello` result.
    Connected(HelloResult),
    /// No host socket is open (`503 host_offline`). The app shows "Desktop offline".
    HostOffline {
        /// The time until the next attempt (10 to 30 seconds).
        retry_in: Duration,
    },
    /// The relay or the host refused the device (`4401` or HTTP `401`).
    /// The app shows "Not authorized" and keeps the keys. Retries pause for 5 minutes.
    /// [`ClientHandle::reconnect_now`] does not end the pause. [`ClientHandle::retry_unauthorized`] does.
    NotAuthorized {
        /// The time until the next attempt.
        retry_in: Duration,
    },
    /// The host sent the encrypted event `device.revoked`.
    /// The app shows "Access revoked", removes the host, and offers pairing again. The engine stopped.
    Revoked,
    /// The own handshake showed that the protocol versions do not overlap.
    /// The app asks the user to update. The engine stopped.
    /// A `4426` close code alone does not give this state, because the relay is not trusted.
    Incompatible,
    /// The connection ended. The engine tries again after `retry_in`.
    Disconnected {
        /// Why the connection ended. The text has no secrets.
        reason: String,
        /// The time until the next attempt.
        retry_in: Duration,
    },
    /// The engine stopped after [`ClientHandle::close`] or [`ClientHandle::unpair`].
    Closed,
}

/// An event of a client session.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ClientEvent {
    /// The state changed.
    Status(ClientStatus),
    /// The pane list changed (`panes.changed`, or a new list after a reconnect).
    PanesChanged(PaneList),
}

/// An update of a terminal subscription (section 8.8).
#[derive(Clone, PartialEq, Eq)]
pub enum TerminalUpdate {
    /// The full ring. Reset the terminal before you write it.
    Snapshot {
        /// The sequence number of the last chunk.
        seq: u64,
        /// The columns of the PTY.
        cols: u16,
        /// The rows of the PTY.
        rows: u16,
        /// The ring bytes.
        data: Vec<u8>,
    },
    /// New output.
    Data {
        /// The sequence number of the last chunk.
        seq: u64,
        /// The output bytes.
        data: Vec<u8>,
    },
    /// The PTY size changed.
    Resized {
        /// The new columns.
        cols: u16,
        /// The new rows.
        rows: u16,
    },
    /// The process exited. The subscription stays open.
    Exit {
        /// The exit code, if the process exited normally.
        code: Option<i32>,
        /// The signal name, if a signal stopped the process.
        signal: Option<String>,
    },
    /// The pane went away. The subscription ended.
    Closed {
        /// The reason.
        reason: String,
    },
}

impl fmt::Debug for TerminalUpdate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TerminalUpdate::Snapshot {
                seq,
                cols,
                rows,
                data,
            } => f
                .debug_struct("Snapshot")
                .field("seq", seq)
                .field("cols", cols)
                .field("rows", rows)
                .field("data_len", &data.len())
                .finish(),
            TerminalUpdate::Data { seq, data } => f
                .debug_struct("Data")
                .field("seq", seq)
                .field("data_len", &data.len())
                .finish(),
            TerminalUpdate::Resized { cols, rows } => f
                .debug_struct("Resized")
                .field("cols", cols)
                .field("rows", rows)
                .finish(),
            TerminalUpdate::Exit { code, signal } => f
                .debug_struct("Exit")
                .field("code", code)
                .field("signal", signal)
                .finish(),
            TerminalUpdate::Closed { reason } => {
                f.debug_struct("Closed").field("reason", reason).finish()
            }
        }
    }
}

/// An error of a [`ClientHandle`] call.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum ClientError {
    /// No session is open. The app shows the input as "not delivered".
    #[error("not connected to the desktop")]
    NotConnected,
    /// The connection ended before the response came.
    #[error("the connection ended before the answer")]
    Disconnected,
    /// No response came in time.
    #[error("the desktop did not answer in time")]
    Timeout,
    /// The host answered with an error.
    #[error(transparent)]
    Rpc(#[from] RpcError),
    /// The input or the text is above its limit (section 8.4).
    #[error("the input is too large")]
    TooLarge,
    /// The response did not decode.
    #[error("protocol error: {0}")]
    Protocol(String),
    /// The engine stopped.
    #[error("the session is closed")]
    Closed,
}

/// Maps a connection failure of a pairing.
fn pair_error_from_connect(error: ConnectError) -> PairError {
    use crate::relay_wire::UpgradeError;
    match error {
        ConnectError::Upgrade { error, .. } => match error {
            UpgradeError::InviteExpired => PairError::Expired,
            UpgradeError::HostOffline => PairError::HostOffline,
            UpgradeError::RateLimited => PairError::RateLimited,
            UpgradeError::Unauthorized | UpgradeError::ClockSkew | UpgradeError::Replay => {
                PairError::NotAuthorized
            }
            UpgradeError::BadRequest | UpgradeError::NotFound | UpgradeError::UpgradeRequired => {
                PairError::Network(format!("the relay answered {}", error.status()))
            }
        },
        other => PairError::Network(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The phone app keeps these values in Tauri state and moves them between threads.
    #[test]
    fn public_types_are_send_and_sync() {
        fn check<T: Send + Sync>() {}
        fn check_send<T: Send>() {}
        check::<ClientHandle>();
        check::<ClientConfig>();
        check::<PairedHost>();
        check::<ClientStatus>();
        check::<ClientEvent>();
        check::<PairError>();
        check_send::<TerminalSubscription>();
        check_send::<PendingPairing>();
    }

    #[test]
    fn default_config_follows_the_contract() {
        let config = ClientConfig::new("halv-mobile/0.1.0", Platform::Android);
        assert_eq!(config.request_timeout, Duration::from_secs(15));
        assert_eq!(config.pair_request_timeout, Duration::from_secs(130));
        assert_eq!(config.backoff_min, Duration::from_millis(500));
        assert_eq!(config.backoff_max, Duration::from_secs(30));
        assert_eq!(config.host_offline_retry_min, Duration::from_secs(10));
        assert_eq!(config.host_offline_retry_max, Duration::from_secs(30));
        assert_eq!(config.not_authorized_pause, Duration::from_secs(300));
        assert_eq!(config.ack_interval, Duration::from_millis(250));
        assert_eq!(config.probe_timeout, Duration::from_secs(5));
    }

    #[test]
    fn paired_host_json_round_trip_and_redaction() {
        let keys = DeviceKeys::from_parts([7u8; 32], RelayToken::from_bytes([9u8; 32]));
        let paired = PairedHost::new(
            "wss://relay.halv.ai",
            HostId::parse("nYqEorAQnfAioCLxARz8mA").unwrap(),
            [5u8; 32],
            "Studio Mac",
            DeviceId::parse("AAAAAAAAAAAAAAAAAAAAAA").unwrap(),
            keys,
        );
        let json = paired.to_json();
        assert!(json.contains("\"hostName\":\"Studio Mac\""));
        let restored = PairedHost::from_json(&json).unwrap();
        assert_eq!(restored.relay_url(), "wss://relay.halv.ai");
        assert_eq!(restored.host_noise_public(), &[5u8; 32]);
        assert_eq!(restored.keys().noise_secret(), &[7u8; 32]);
        assert_eq!(restored.keys().relay_token().as_bytes(), &[9u8; 32]);
        assert_eq!(restored.device_id(), paired.device_id());

        let debug = format!("{paired:?}");
        assert!(!debug.contains(&b64u::encode(&[7u8; 32])));
        assert!(!debug.contains(&b64u::encode(&[9u8; 32])));

        assert!(PairedHost::from_json("[]").is_err());
        assert!(PairedHost::from_json(&json.replace("\"v\":1", "\"v\":2")).is_err());
        assert!(PairedHost::from_json(&json.replace("wss://", "ws://")).is_err());
    }
}
