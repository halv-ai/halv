//! The relay wire format (section 6).
//!
//! - Endpoints (section 6.1).
//! - DATA frames and JSON control messages on the host socket (section 6.4).
//! - Device socket text frames (section 6.5).
//! - Close codes and HTTP errors (section 6.6).
//! - Limits and keepalive times (sections 6.7 and 6.8).
//!
//! The relay never sees plaintext. These types carry only Noise messages and metadata.

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::ids::{ConnId, DeviceId, HostId, InviteId};
use crate::keys::TokenHash;

/// The relay protocol version in `ready` (section 6.2).
pub const RELAY_PROTOCOL_VERSION: u32 = 1;

/// The health path. It returns `200` with the body `ok`.
pub const HEALTH_PATH: &str = "/health";

/// Returns the path of the host WebSocket: `/v1/hosts/{hostId}/host`.
pub fn host_socket_path(host_id: &HostId) -> String {
    format!("/v1/hosts/{host_id}/host")
}

/// Returns the path of a device WebSocket: `/v1/hosts/{hostId}/device`.
pub fn device_socket_path(host_id: &HostId) -> String {
    format!("/v1/hosts/{host_id}/device")
}

/// Returns the URL of the host WebSocket for a relay base URL (offer field `r`).
pub fn host_socket_url(relay_url: &str, host_id: &HostId) -> String {
    format!("{relay_url}{}", host_socket_path(host_id))
}

/// Returns the URL of a device WebSocket for a relay base URL (offer field `r`).
pub fn device_socket_url(relay_url: &str, host_id: &HostId) -> String {
    format!("{relay_url}{}", device_socket_path(host_id))
}

/// Open device sockets per host object.
pub const MAX_OPEN_DEVICE_SOCKETS: usize = 8;

/// Paired devices per host object.
pub const MAX_PAIRED_DEVICES: usize = 32;

/// Active invites per host object.
pub const MAX_ACTIVE_INVITES: usize = 4;

/// The longest invite lifetime (5 minutes).
pub const MAX_INVITE_LIFETIME_MS: u64 = 300_000;

/// The most tries of one invite.
pub const MAX_INVITE_TRIES: u32 = 5;

/// The largest Noise message. It is also the largest device frame.
pub const MAX_NOISE_MESSAGE: usize = 65_535;

/// The largest device frame: one Noise message.
pub const MAX_DEVICE_FRAME: usize = MAX_NOISE_MESSAGE;

/// The largest host DATA frame: the 5-byte header and one Noise message.
pub const MAX_HOST_DATA_FRAME: usize = DATA_HEADER_LEN + MAX_NOISE_MESSAGE;

/// The largest host control frame, in bytes of UTF-8 text.
pub const MAX_HOST_CONTROL_FRAME: usize = 65_536;

/// Upgrades for each client IP and path kind in one rate window.
pub const UPGRADE_RATE_LIMIT: u32 = 30;

/// The rate window of the Worker (60 seconds).
pub const UPGRADE_RATE_WINDOW_MS: u64 = 60_000;

/// A client sends a WebSocket ping at least this often (section 6.8).
pub const PING_INTERVAL_MS: u64 = 20_000;

/// A client closes the socket after this time with no received frame and no pong.
pub const IDLE_TIMEOUT_MS: u64 = 60_000;

/// The `Retry-After` value of a `503 host_offline` answer, in seconds.
pub const HOST_OFFLINE_RETRY_AFTER_SECS: u32 = 5;

/// The host closes a pairing connection with `4410` after this time (section 7.3, step 10).
pub const PAIRING_CLOSE_DELAY_MS: u64 = 10_000;

/// The longest close reason in bytes. A WebSocket close frame allows no more.
pub const MAX_CLOSE_REASON_BYTES: usize = 123;

/// The text frame that a device can send when it cannot send protocol pings (section 6.5).
pub const PING_TEXT: &str = "ping";

/// The auto-response of the relay to [`PING_TEXT`].
pub const PONG_TEXT: &str = "pong";

/// The type byte of a DATA frame.
pub const DATA_FRAME_TYPE: u8 = 0x01;

/// The length of the DATA frame header: the type byte and the `u32` connection id.
pub const DATA_HEADER_LEN: usize = 5;

/// A binary host socket frame that is not valid (section 6.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum FrameError {
    /// The frame is larger than its limit. The relay closes the socket with `4413`.
    #[error("the frame is larger than its limit")]
    TooLarge,
    /// The type byte is not `0x01`. The relay answers with `err` and drops the frame.
    #[error("unknown frame type {0:#04x}")]
    UnknownType(u8),
    /// The frame is shorter than its header.
    #[error("the frame is shorter than its header")]
    Truncated,
    /// The frame has no Noise message.
    #[error("the frame has no Noise message")]
    EmptyPayload,
    /// The connection id is zero.
    #[error("the connection id is zero")]
    ZeroConnection,
}

/// A DATA frame: one Noise message for one connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DataFrame<'a> {
    /// The connection id.
    pub conn: ConnId,
    /// One Noise message (1 to 65,535 bytes).
    pub payload: &'a [u8],
}

/// Writes a DATA frame: `0x01 || u32 connection id || Noise message`.
pub fn encode_data(conn: ConnId, noise_message: &[u8]) -> Result<Vec<u8>, FrameError> {
    if noise_message.is_empty() {
        return Err(FrameError::EmptyPayload);
    }
    if noise_message.len() > MAX_NOISE_MESSAGE {
        return Err(FrameError::TooLarge);
    }
    let mut frame = Vec::with_capacity(DATA_HEADER_LEN + noise_message.len());
    frame.push(DATA_FRAME_TYPE);
    frame.extend_from_slice(&conn.get().to_be_bytes());
    frame.extend_from_slice(noise_message);
    Ok(frame)
}

/// Reads a DATA frame. The payload borrows from `frame`.
///
/// The size check comes first, so an oversized frame of any type gives [`FrameError::TooLarge`].
pub fn decode_data(frame: &[u8]) -> Result<DataFrame<'_>, FrameError> {
    if frame.len() > MAX_HOST_DATA_FRAME {
        return Err(FrameError::TooLarge);
    }
    let (&kind, rest) = frame.split_first().ok_or(FrameError::Truncated)?;
    if kind != DATA_FRAME_TYPE {
        return Err(FrameError::UnknownType(kind));
    }
    if rest.len() < 4 {
        return Err(FrameError::Truncated);
    }
    let (conn, payload) = rest.split_at(4);
    let conn = u32::from_be_bytes(conn.try_into().expect("4 bytes"));
    let conn = ConnId::new(conn).ok_or(FrameError::ZeroConnection)?;
    if payload.is_empty() {
        return Err(FrameError::EmptyPayload);
    }
    Ok(DataFrame { conn, payload })
}

/// A paired device in `devices_sync`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceEntry {
    /// The device id.
    pub device: DeviceId,
    /// `b64u(SHA-256(relay_token))`.
    pub hash: TokenHash,
}

/// A control message from the host to the host object (section 6.4).
///
/// The JSON object has a `t` field. Unknown fields are ignored.
/// An unknown `t` decodes as [`HostToRelay::Unknown`]. The relay answers it with `err`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum HostToRelay {
    /// Adds or replaces an invite. At most 4 invites. `tries` is at most 5.
    InvitePut {
        /// The request id.
        rid: u32,
        /// The invite id.
        invite: InviteId,
        /// `b64u(SHA-256(invite_token))`.
        hash: TokenHash,
        /// The expiry time in epoch milliseconds.
        exp: u64,
        /// The number of admissions that the invite allows.
        tries: u32,
    },
    /// Deletes an invite.
    InviteDel {
        /// The request id.
        rid: u32,
        /// The invite id.
        invite: InviteId,
    },
    /// Adds or replaces a device. At most 32 devices.
    DevicePut {
        /// The request id.
        rid: u32,
        /// The device id.
        device: DeviceId,
        /// `b64u(SHA-256(relay_token))`.
        hash: TokenHash,
    },
    /// Deletes a device. The relay closes its device sockets with `4401`.
    DeviceDel {
        /// The request id.
        rid: u32,
        /// The device id.
        device: DeviceId,
    },
    /// Replaces all devices. The relay closes the sockets of removed devices with `4401`.
    /// The host sends it after every `ready`.
    DevicesSync {
        /// The request id.
        rid: u32,
        /// The complete device list.
        devices: Vec<DeviceEntry>,
    },
    /// Closes one device socket.
    ConnClose {
        /// The connection id.
        conn: ConnId,
        /// The close code, in 4000–4999.
        code: u16,
        /// An optional close reason of at most 123 bytes.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    /// A message with an unknown `t`. It is never sent.
    #[serde(other, skip_serializing)]
    Unknown,
}

impl HostToRelay {
    /// Makes a `conn_close` message with a close code of section 6.6.
    pub fn conn_close(conn: ConnId, code: CloseCode) -> HostToRelay {
        HostToRelay::ConnClose {
            conn,
            code: code.code(),
            reason: None,
        }
    }

    /// Checks the rules that one message can break:
    /// `tries` from 1 to 5, a close code in 4000–4999, a reason of at most 123 bytes,
    /// and at most 32 devices in `devices_sync`.
    pub fn validate(&self) -> Result<()> {
        match self {
            HostToRelay::InvitePut { tries, .. } if !(1..=MAX_INVITE_TRIES).contains(tries) => {
                Err(Error::Invalid("invite tries"))
            }
            HostToRelay::DevicesSync { devices, .. } if devices.len() > MAX_PAIRED_DEVICES => {
                Err(Error::Invalid("device count"))
            }
            HostToRelay::ConnClose { code, reason, .. } => {
                if !(4000..=4999).contains(code) {
                    Err(Error::Invalid("close code"))
                } else if reason
                    .as_ref()
                    .is_some_and(|reason| reason.len() > MAX_CLOSE_REASON_BYTES)
                {
                    Err(Error::Invalid("close reason"))
                } else {
                    Ok(())
                }
            }
            HostToRelay::Unknown => Err(Error::Invalid("relay control message")),
            _ => Ok(()),
        }
    }

    /// Writes the JSON text frame. It fails when [`HostToRelay::validate`] fails,
    /// or when the text is larger than 65,536 bytes.
    pub fn to_json(&self) -> Result<String> {
        self.validate()?;
        to_control_json(self)
    }

    /// Reads a JSON text frame. It checks the structure and the size, not [`HostToRelay::validate`].
    pub fn from_json(text: &str) -> Result<HostToRelay> {
        from_control_json(text)
    }
}

/// The kind of a device connection in `conn_open`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnKind {
    /// A paired device that connected with its relay token.
    Device,
    /// A device that connected with an invite token to pair.
    Invite,
}

/// The token that admitted a device connection.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ConnPeer {
    /// A paired device (`kind: "device"`).
    Device(DeviceId),
    /// An invite (`kind: "invite"`).
    Invite(InviteId),
}

impl ConnPeer {
    /// Returns the connection kind.
    pub fn kind(&self) -> ConnKind {
        match self {
            ConnPeer::Device(_) => ConnKind::Device,
            ConnPeer::Invite(_) => ConnKind::Invite,
        }
    }
}

/// The `conn_open` message: a device socket opened.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "ConnOpenWire", into = "ConnOpenWire")]
pub struct ConnOpen {
    /// The connection id.
    pub conn: ConnId,
    /// The device or the invite of the connection.
    pub peer: ConnPeer,
}

#[derive(Serialize, Deserialize)]
struct ConnOpenWire {
    conn: ConnId,
    kind: ConnKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    device: Option<DeviceId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    invite: Option<InviteId>,
}

impl TryFrom<ConnOpenWire> for ConnOpen {
    type Error = &'static str;

    fn try_from(wire: ConnOpenWire) -> Result<Self, Self::Error> {
        let peer = match (wire.kind, wire.device, wire.invite) {
            (ConnKind::Device, Some(device), _) => ConnPeer::Device(device),
            (ConnKind::Invite, _, Some(invite)) => ConnPeer::Invite(invite),
            (ConnKind::Device, None, _) => return Err("conn_open of kind device has no device"),
            (ConnKind::Invite, _, None) => return Err("conn_open of kind invite has no invite"),
        };
        Ok(ConnOpen {
            conn: wire.conn,
            peer,
        })
    }
}

impl From<ConnOpen> for ConnOpenWire {
    fn from(open: ConnOpen) -> Self {
        let kind = open.peer.kind();
        let (device, invite) = match open.peer {
            ConnPeer::Device(device) => (Some(device), None),
            ConnPeer::Invite(invite) => (None, Some(invite)),
        };
        ConnOpenWire {
            conn: open.conn,
            kind,
            device,
            invite,
        }
    }
}

/// A control message from the host object to the host (section 6.4).
///
/// The JSON object has a `t` field. Unknown fields are ignored.
/// An unknown `t` decodes as [`RelayToHost::Unknown`]. The host ignores it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum RelayToHost {
    /// The host socket is open. The host then sends `devices_sync`.
    Ready {
        /// The relay protocol version.
        v: u32,
    },
    /// A control request succeeded.
    Ok {
        /// The request id of the request.
        rid: u32,
    },
    /// A control request or a frame failed.
    Err {
        /// The request id of the request, if the failure has one.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        rid: Option<u32>,
        /// The error code.
        code: String,
        /// The error text.
        #[serde(default)]
        msg: String,
    },
    /// A device socket opened.
    ConnOpen(ConnOpen),
    /// A device socket closed.
    ConnClose {
        /// The connection id.
        conn: ConnId,
        /// The WebSocket close code of the device socket.
        code: u16,
    },
    /// A message with an unknown `t`. It is never sent.
    #[serde(other, skip_serializing)]
    Unknown,
}

impl RelayToHost {
    /// Writes the JSON text frame.
    pub fn to_json(&self) -> Result<String> {
        to_control_json(self)
    }

    /// Reads a JSON text frame.
    pub fn from_json(text: &str) -> Result<RelayToHost> {
        from_control_json(text)
    }
}

fn to_control_json<T: Serialize>(message: &T) -> Result<String> {
    let text =
        serde_json::to_string(message).map_err(|_| Error::Invalid("relay control message"))?;
    if text.len() > MAX_HOST_CONTROL_FRAME {
        return Err(Error::Frame(FrameError::TooLarge));
    }
    Ok(text)
}

fn from_control_json<T: serde::de::DeserializeOwned>(text: &str) -> Result<T> {
    if text.len() > MAX_HOST_CONTROL_FRAME {
        return Err(Error::Frame(FrameError::TooLarge));
    }
    if !crate::is_json_object(text.as_bytes()) {
        return Err(Error::Invalid("relay control message"));
    }
    serde_json::from_str(text).map_err(|_| Error::Invalid("relay control message"))
}

/// A WebSocket close code of section 6.6.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CloseCode {
    /// `4400`: a bad frame or a protocol error. The device reconnects with backoff.
    BadFrame,
    /// `4401`: unauthorized, or the device was revoked.
    /// The device stops automatic retries for 5 minutes and keeps its keys.
    /// It shows "Access revoked" only after the encrypted event `device.revoked`.
    /// Otherwise it shows "Not authorized".
    Unauthorized,
    /// `4408`: the host socket closed, or the host ended the connection.
    /// The device reconnects with backoff.
    HostGone,
    /// `4409`: a newer host socket replaced this one (host sockets only).
    /// The host stops this socket.
    Replaced,
    /// `4410`: pairing finished. The device connects as a paired device.
    PairingDone,
    /// `4413`: a frame is too large. The device reports a bug and reconnects.
    TooLarge,
    /// `4426`: the protocol versions do not overlap.
    /// The device stops and asks the user to update.
    Incompatible,
    /// `4429`: rate limited. The device reconnects with a longer backoff.
    RateLimited,
    /// `4500`: an internal relay error. The device reconnects with backoff.
    Internal,
}

impl CloseCode {
    /// Every close code of section 6.6.
    pub const ALL: [CloseCode; 9] = [
        CloseCode::BadFrame,
        CloseCode::Unauthorized,
        CloseCode::HostGone,
        CloseCode::Replaced,
        CloseCode::PairingDone,
        CloseCode::TooLarge,
        CloseCode::Incompatible,
        CloseCode::RateLimited,
        CloseCode::Internal,
    ];

    /// Returns the numeric close code.
    pub const fn code(self) -> u16 {
        match self {
            CloseCode::BadFrame => 4400,
            CloseCode::Unauthorized => 4401,
            CloseCode::HostGone => 4408,
            CloseCode::Replaced => 4409,
            CloseCode::PairingDone => 4410,
            CloseCode::TooLarge => 4413,
            CloseCode::Incompatible => 4426,
            CloseCode::RateLimited => 4429,
            CloseCode::Internal => 4500,
        }
    }

    /// Returns the close code for a number, or `None` for a number that section 6.6 does not define.
    pub fn from_code(code: u16) -> Option<CloseCode> {
        CloseCode::ALL
            .into_iter()
            .find(|close| close.code() == code)
    }
}

/// An HTTP error of a WebSocket upgrade (section 6.6).
///
/// The body is the JSON `{"code": "<code>"}`.
/// Section 6.6 names the codes for `401` and `503`.
/// For the other statuses this crate uses `bad_request`, `not_found`, `upgrade_required`,
/// and `rate_limited`, and a reader uses only the status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UpgradeError {
    /// `400 bad_request`: a header is missing or malformed.
    BadRequest,
    /// `401 unauthorized`: a token or a host key is not accepted.
    /// A paired device pauses automatic retries for 5 minutes, shows "Not authorized",
    /// and keeps its keys (section 10).
    Unauthorized,
    /// `401 clock_skew`: the host timestamp is too far from the relay clock.
    ClockSkew,
    /// `401 replay`: the host timestamp is not newer than the last one.
    Replay,
    /// `401 invite_expired`: the invite expired.
    InviteExpired,
    /// `404 not_found`: an unknown path, or a host id with a wrong format.
    NotFound,
    /// `426 upgrade_required`: the request has no `Upgrade: websocket`.
    UpgradeRequired,
    /// `429 rate_limited`: too many upgrades, or 8 device sockets are already open.
    RateLimited,
    /// `503 host_offline`: no host socket is open. The answer has `Retry-After: 5`.
    /// The device shows "Desktop offline" and tries again every 10 to 30 seconds (section 10).
    HostOffline,
}

impl UpgradeError {
    /// Returns the HTTP status.
    pub const fn status(self) -> u16 {
        match self {
            UpgradeError::BadRequest => 400,
            UpgradeError::Unauthorized
            | UpgradeError::ClockSkew
            | UpgradeError::Replay
            | UpgradeError::InviteExpired => 401,
            UpgradeError::NotFound => 404,
            UpgradeError::UpgradeRequired => 426,
            UpgradeError::RateLimited => 429,
            UpgradeError::HostOffline => 503,
        }
    }

    /// Returns the `code` of the JSON body.
    pub const fn code(self) -> &'static str {
        match self {
            UpgradeError::BadRequest => "bad_request",
            UpgradeError::Unauthorized => "unauthorized",
            UpgradeError::ClockSkew => "clock_skew",
            UpgradeError::Replay => "replay",
            UpgradeError::InviteExpired => "invite_expired",
            UpgradeError::NotFound => "not_found",
            UpgradeError::UpgradeRequired => "upgrade_required",
            UpgradeError::RateLimited => "rate_limited",
            UpgradeError::HostOffline => "host_offline",
        }
    }

    /// Returns the JSON body `{"code":"<code>"}`.
    pub fn body(self) -> String {
        format!("{{\"code\":\"{}\"}}", self.code())
    }

    /// Reads an upgrade error response.
    ///
    /// It returns `None` for a status that section 6.6 does not define,
    /// and for a `503` without the code `host_offline`.
    /// A `401` with an unknown code gives [`UpgradeError::Unauthorized`].
    pub fn from_response(status: u16, body: &str) -> Option<UpgradeError> {
        #[derive(Deserialize)]
        struct Body {
            #[serde(default)]
            code: Option<String>,
        }

        let code = serde_json::from_str::<Body>(body)
            .ok()
            .and_then(|body| body.code);
        let code = code.as_deref();
        match status {
            400 => Some(UpgradeError::BadRequest),
            401 => Some(match code {
                Some("clock_skew") => UpgradeError::ClockSkew,
                Some("replay") => UpgradeError::Replay,
                Some("invite_expired") => UpgradeError::InviteExpired,
                _ => UpgradeError::Unauthorized,
            }),
            404 => Some(UpgradeError::NotFound),
            426 => Some(UpgradeError::UpgradeRequired),
            429 => Some(UpgradeError::RateLimited),
            503 if code == Some("host_offline") => Some(UpgradeError::HostOffline),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conn(value: u32) -> ConnId {
        ConnId::new(value).unwrap()
    }

    fn device_id() -> DeviceId {
        DeviceId::parse("AAAAAAAAAAAAAAAAAAAAAA").unwrap()
    }

    fn invite_id() -> InviteId {
        InviteId::parse("BBBBBBBBBBBBBBBBBBBBBB").unwrap()
    }

    #[test]
    fn data_frame_round_trip() {
        let frame = encode_data(conn(0x0102_0304), b"noise").unwrap();
        assert_eq!(
            frame,
            [0x01, 0x01, 0x02, 0x03, 0x04, b'n', b'o', b'i', b's', b'e']
        );
        let decoded = decode_data(&frame).unwrap();
        assert_eq!(decoded.conn.get(), 0x0102_0304);
        assert_eq!(decoded.payload, b"noise");
    }

    #[test]
    fn data_frame_size_edges() {
        let largest = vec![7u8; MAX_NOISE_MESSAGE];
        let frame = encode_data(conn(1), &largest).unwrap();
        assert_eq!(frame.len(), MAX_HOST_DATA_FRAME);
        assert_eq!(decode_data(&frame).unwrap().payload.len(), 65_535);
        assert_eq!(
            encode_data(conn(1), &vec![7u8; MAX_NOISE_MESSAGE + 1]).unwrap_err(),
            FrameError::TooLarge
        );
        let mut oversized = frame.clone();
        oversized.push(0);
        assert_eq!(decode_data(&oversized).unwrap_err(), FrameError::TooLarge);
        assert_eq!(
            encode_data(conn(1), b"").unwrap_err(),
            FrameError::EmptyPayload
        );
    }

    #[test]
    fn data_frame_rejects_bad_headers() {
        assert_eq!(decode_data(&[]).unwrap_err(), FrameError::Truncated);
        assert_eq!(
            decode_data(&[0x01, 0, 0]).unwrap_err(),
            FrameError::Truncated
        );
        assert_eq!(
            decode_data(&[0x02, 0, 0, 0, 1, 9]).unwrap_err(),
            FrameError::UnknownType(0x02)
        );
        assert_eq!(
            decode_data(&[0x01, 0, 0, 0, 0, 9]).unwrap_err(),
            FrameError::ZeroConnection
        );
        assert_eq!(
            decode_data(&[0x01, 0, 0, 0, 1]).unwrap_err(),
            FrameError::EmptyPayload
        );
    }

    #[test]
    fn host_control_messages_have_exact_json() {
        let hash = TokenHash::of(b"token");
        let cases = [
            (
                HostToRelay::InvitePut {
                    rid: 1,
                    invite: invite_id(),
                    hash: hash.clone(),
                    exp: 1_760_460_300_000,
                    tries: 5,
                },
                format!(
                    r#"{{"t":"invite_put","rid":1,"invite":"BBBBBBBBBBBBBBBBBBBBBB","hash":"{hash}","exp":1760460300000,"tries":5}}"#
                ),
            ),
            (
                HostToRelay::InviteDel {
                    rid: 2,
                    invite: invite_id(),
                },
                r#"{"t":"invite_del","rid":2,"invite":"BBBBBBBBBBBBBBBBBBBBBB"}"#.to_owned(),
            ),
            (
                HostToRelay::DevicePut {
                    rid: 3,
                    device: device_id(),
                    hash: hash.clone(),
                },
                format!(
                    r#"{{"t":"device_put","rid":3,"device":"AAAAAAAAAAAAAAAAAAAAAA","hash":"{hash}"}}"#
                ),
            ),
            (
                HostToRelay::DeviceDel {
                    rid: 4,
                    device: device_id(),
                },
                r#"{"t":"device_del","rid":4,"device":"AAAAAAAAAAAAAAAAAAAAAA"}"#.to_owned(),
            ),
            (
                HostToRelay::DevicesSync {
                    rid: 5,
                    devices: vec![DeviceEntry {
                        device: device_id(),
                        hash: hash.clone(),
                    }],
                },
                format!(
                    r#"{{"t":"devices_sync","rid":5,"devices":[{{"device":"AAAAAAAAAAAAAAAAAAAAAA","hash":"{hash}"}}]}}"#
                ),
            ),
            (
                HostToRelay::conn_close(conn(9), CloseCode::Unauthorized),
                r#"{"t":"conn_close","conn":9,"code":4401}"#.to_owned(),
            ),
            (
                HostToRelay::ConnClose {
                    conn: conn(9),
                    code: 4410,
                    reason: Some("paired".to_owned()),
                },
                r#"{"t":"conn_close","conn":9,"code":4410,"reason":"paired"}"#.to_owned(),
            ),
        ];
        for (message, json) in cases {
            assert_eq!(message.to_json().unwrap(), json);
            assert_eq!(HostToRelay::from_json(&json).unwrap(), message);
        }
    }

    #[test]
    fn relay_control_messages_have_exact_json() {
        let cases = [
            (RelayToHost::Ready { v: 1 }, r#"{"t":"ready","v":1}"#),
            (RelayToHost::Ok { rid: 7 }, r#"{"t":"ok","rid":7}"#),
            (
                RelayToHost::Err {
                    rid: Some(7),
                    code: "limit".to_owned(),
                    msg: "too many invites".to_owned(),
                },
                r#"{"t":"err","rid":7,"code":"limit","msg":"too many invites"}"#,
            ),
            (
                RelayToHost::Err {
                    rid: None,
                    code: "bad_frame".to_owned(),
                    msg: String::new(),
                },
                r#"{"t":"err","code":"bad_frame","msg":""}"#,
            ),
            (
                RelayToHost::ConnOpen(ConnOpen {
                    conn: conn(3),
                    peer: ConnPeer::Device(device_id()),
                }),
                r#"{"t":"conn_open","conn":3,"kind":"device","device":"AAAAAAAAAAAAAAAAAAAAAA"}"#,
            ),
            (
                RelayToHost::ConnOpen(ConnOpen {
                    conn: conn(4),
                    peer: ConnPeer::Invite(invite_id()),
                }),
                r#"{"t":"conn_open","conn":4,"kind":"invite","invite":"BBBBBBBBBBBBBBBBBBBBBB"}"#,
            ),
            (
                RelayToHost::ConnClose {
                    conn: conn(4),
                    code: 1006,
                },
                r#"{"t":"conn_close","conn":4,"code":1006}"#,
            ),
        ];
        for (message, json) in cases {
            assert_eq!(message.to_json().unwrap(), json);
            assert_eq!(RelayToHost::from_json(json).unwrap(), message);
        }
    }

    #[test]
    fn control_messages_ignore_unknown_fields_and_types() {
        let message = RelayToHost::from_json(r#"{"x":[1,{"y":2}],"t":"ok","rid":7,"z":null}"#);
        assert_eq!(message.unwrap(), RelayToHost::Ok { rid: 7 });
        assert_eq!(
            RelayToHost::from_json(r#"{"t":"future","rid":1}"#).unwrap(),
            RelayToHost::Unknown
        );
        assert_eq!(
            HostToRelay::from_json(r#"{"t":"future"}"#).unwrap(),
            HostToRelay::Unknown
        );
        assert!(RelayToHost::Unknown.to_json().is_err());
        assert!(HostToRelay::Unknown.to_json().is_err());
        // An `err` without `msg` is accepted.
        assert_eq!(
            RelayToHost::from_json(r#"{"t":"err","code":"x"}"#).unwrap(),
            RelayToHost::Err {
                rid: None,
                code: "x".to_owned(),
                msg: String::new()
            }
        );
    }

    #[test]
    fn control_messages_reject_bad_values() {
        for json in [
            r#"{"rid":1}"#,
            r#"{"t":"ok"}"#,
            r#"{"t":"ok","rid":-1}"#,
            r#"{"t":"conn_close","conn":0,"code":4401}"#,
            r#"{"t":"conn_open","conn":3,"kind":"device"}"#,
            r#"{"t":"conn_open","conn":3,"kind":"invite","device":"AAAAAAAAAAAAAAAAAAAAAA"}"#,
            r#"{"t":"conn_open","conn":3,"kind":"observer","device":"AAAAAAAAAAAAAAAAAAAAAA"}"#,
            r#"{"t":"conn_open","conn":3,"kind":"device","device":"short"}"#,
            r#"{"t":"ok","rid":1,"rid":2}"#,
            r#"["ok",7]"#,
            "not json",
        ] {
            assert!(RelayToHost::from_json(json).is_err(), "{json}");
        }
        assert!(HostToRelay::from_json(
            r#"{"t":"device_put","rid":1,"device":"AAAAAAAAAAAAAAAAAAAAAA","hash":"raw-token"}"#
        )
        .is_err());
        let oversized = format!(r#"{{"t":"ok","rid":1,"pad":"{}"}}"#, "x".repeat(65_536));
        assert_eq!(
            RelayToHost::from_json(&oversized).unwrap_err(),
            Error::Frame(FrameError::TooLarge)
        );
    }

    #[test]
    fn host_messages_are_validated_before_sending() {
        let invite = |tries| HostToRelay::InvitePut {
            rid: 1,
            invite: invite_id(),
            hash: TokenHash::of(b"x"),
            exp: 0,
            tries,
        };
        assert!(invite(1).to_json().is_ok());
        assert!(invite(0).to_json().is_err());
        assert!(invite(6).to_json().is_err());
        let close = |code, reason: Option<String>| HostToRelay::ConnClose {
            conn: conn(1),
            code,
            reason,
        };
        assert!(close(3999, None).to_json().is_err());
        assert!(close(5000, None).to_json().is_err());
        assert!(close(4999, Some("r".repeat(123))).to_json().is_ok());
        assert!(close(4999, Some("r".repeat(124))).to_json().is_err());
        let sync = |count| HostToRelay::DevicesSync {
            rid: 1,
            devices: (0..count)
                .map(|_| DeviceEntry {
                    device: device_id(),
                    hash: TokenHash::of(b"x"),
                })
                .collect(),
        };
        assert!(sync(32).to_json().is_ok());
        assert!(sync(33).to_json().is_err());
    }

    #[test]
    fn close_codes() {
        let codes: Vec<u16> = CloseCode::ALL.iter().map(|close| close.code()).collect();
        assert_eq!(
            codes,
            [4400, 4401, 4408, 4409, 4410, 4413, 4426, 4429, 4500]
        );
        for close in CloseCode::ALL {
            assert_eq!(CloseCode::from_code(close.code()), Some(close));
        }
        assert_eq!(CloseCode::from_code(1000), None);
        assert_eq!(CloseCode::from_code(4402), None);
    }

    #[test]
    fn upgrade_errors() {
        let all = [
            (UpgradeError::BadRequest, 400, "bad_request"),
            (UpgradeError::Unauthorized, 401, "unauthorized"),
            (UpgradeError::ClockSkew, 401, "clock_skew"),
            (UpgradeError::Replay, 401, "replay"),
            (UpgradeError::InviteExpired, 401, "invite_expired"),
            (UpgradeError::NotFound, 404, "not_found"),
            (UpgradeError::UpgradeRequired, 426, "upgrade_required"),
            (UpgradeError::RateLimited, 429, "rate_limited"),
            (UpgradeError::HostOffline, 503, "host_offline"),
        ];
        for (error, status, code) in all {
            assert_eq!(error.status(), status);
            assert_eq!(error.code(), code);
            assert_eq!(error.body(), format!("{{\"code\":\"{code}\"}}"));
            assert_eq!(
                UpgradeError::from_response(status, &error.body()),
                Some(error)
            );
        }
        assert_eq!(
            UpgradeError::from_response(401, "{\"code\":\"new_reason\"}"),
            Some(UpgradeError::Unauthorized)
        );
        assert_eq!(
            UpgradeError::from_response(401, "<html>"),
            Some(UpgradeError::Unauthorized)
        );
        assert_eq!(UpgradeError::from_response(503, "busy"), None);
        assert_eq!(UpgradeError::from_response(500, "{}"), None);
    }

    #[test]
    fn endpoints() {
        let host_id = HostId::parse("nYqEorAQnfAioCLxARz8mA").unwrap();
        assert_eq!(
            host_socket_path(&host_id),
            "/v1/hosts/nYqEorAQnfAioCLxARz8mA/host"
        );
        assert_eq!(
            device_socket_url("wss://relay.halv.ai", &host_id),
            "wss://relay.halv.ai/v1/hosts/nYqEorAQnfAioCLxARz8mA/device"
        );
        assert_eq!(
            host_socket_url("ws://127.0.0.1:8787", &host_id),
            "ws://127.0.0.1:8787/v1/hosts/nYqEorAQnfAioCLxARz8mA/host"
        );
    }
}
