//! The Halv remote protocol, version 1: the shared code of the desktop host and the phone.
//!
//! `docs/remote-protocol.md` is the normative contract. ADR 0009 records the decisions.
//! Section numbers in this crate refer to the contract.
//!
//! The crate has no Tauri dependency.
//! The default feature set is pure Rust with no network code, so it builds for iOS and Android
//! without a C toolchain.
//!
//! # Features
//!
//! - `host`: the host engine of the desktop gateway (module `host`).
//! - `client`: the client engine of the phone (module `client`).
//! - `testing`: an in-process relay and in-memory host parts for tests (module `testing`).
//!
//! The `host` and `client` features add tokio, tokio-tungstenite 0.27, and rustls with ring
//! and the webpki roots (module `net`). They never pull aws-lc-rs or native-tls.
//! ring needs a C compiler for each mobile target, for example the Android NDK clang.
//!
//! # Modules
//!
//! - [`keys`]: the host identity, the device keys, and token hashes (section 4).
//! - [`ids`]: the host id and the other ids (sections 4.2 and 4.4).
//! - [`pairing`]: the pairing offer and the values that HKDF derives from it (section 5).
//! - [`relay_auth`]: the relay host signature and the bearer tokens (sections 6.2 and 6.3).
//! - [`relay_wire`]: DATA frames, control messages, close codes, HTTP errors, and limits (section 6).
//! - [`noise`]: the pairing and session handshakes and the confirmation code (section 7).
//! - [`framing`]: the [`Transport`] with the flag byte and chunks (section 7.6).
//! - [`rpc`]: the application messages (section 8).
//! - [`b64u`]: base64url without padding (section 3).
//!
//! # Secrets
//!
//! Secret types zeroize on drop, and their `Debug` output is redacted.
//! Text that holds a secret, such as an offer URI or the keychain JSON, comes in a
//! [`zeroize::Zeroizing`] wrapper. Never log tokens, secrets, keys, pairing URIs,
//! or frame contents (section 11).
//!
//! # Example
//!
//! A pairing from the offer to the first request:
//!
//! ```
//! use halv_remote::rpc::{self, PairRequest, PairRequestParams, Platform};
//! use halv_remote::{
//!     DeviceKeys, HandshakePayload, HostIdentity, Message, PairingInitiator, PairingOffer,
//!     PairingResponder, RequestId,
//! };
//!
//! # fn main() -> halv_remote::Result<()> {
//! let now = 1_760_460_000_000;
//!
//! // Host: make the identity once, then an offer for each pairing.
//! let identity = HostIdentity::generate_at(now)?;
//! let offer = PairingOffer::generate(&identity, "wss://relay.halv.ai", Some("Studio Mac".into()), now)?;
//! let uri = offer.to_uri(); // The desktop shows it as a QR code.
//!
//! // Device: read the scanned offer and make new keys for this host.
//! let scanned = PairingOffer::parse_uri(&uri, now)?;
//! let device = DeviceKeys::generate()?;
//! let p1 = HandshakePayload::new("halv-mobile/0.1.0");
//! let (initiator, message_1) = PairingInitiator::start(&scanned, &device, p1)?;
//!
//! // Host: read message 1 with the PSK of the invite, check the device, and answer.
//! let incoming = PairingResponder::new(&identity, &offer.psk())?.read_message_1(&message_1)?;
//! let (message_2, mut host) = incoming.accept(HandshakePayload::new("halv-desktop/0.8.0"))?;
//!
//! // Device: finish the handshake. Both sides show the same confirmation code.
//! let mut phone = initiator.read_message_2(&message_2)?;
//! assert!(phone.versions_overlap());
//! assert_eq!(phone.confirmation_code(), host.confirmation_code());
//!
//! // Device: send pair.request through the transport.
//! let params = PairRequestParams {
//!     name: "Ana's phone".into(),
//!     platform: Platform::Ios,
//!     model: None,
//!     token: device.relay_token().clone(),
//!     app: "halv-mobile/0.1.0".into(),
//! };
//! let request = rpc::encode_call::<PairRequest>(RequestId::MIN, &params)?;
//! for noise_message in phone.transport.seal(&request)? {
//!     // Host: each Noise message arrives in its own WebSocket frame.
//!     if let Some(bytes) = host.transport.open(&noise_message)? {
//!         let Message::Request(request) = Message::decode(&bytes)? else {
//!             panic!("the device sends requests");
//!         };
//!         let params = PairRequestParams::parse(&request).expect("a valid pair.request");
//!         assert_eq!(params.token.hash(), device.token_hash());
//!     }
//! }
//! # Ok(())
//! # }
//! ```

pub mod b64u;
#[cfg(feature = "client")]
pub mod client;
mod error;
pub mod framing;
#[cfg(feature = "host")]
pub mod host;
pub mod ids;
pub mod keys;
#[cfg(feature = "__net")]
pub mod net;
pub mod noise;
pub mod pairing;
pub mod relay_auth;
pub mod relay_wire;
pub mod rpc;
mod secret;
#[cfg(feature = "testing")]
pub mod testing;

pub use error::{Error, HandshakeFailure, OfferError, Result};
pub use framing::Transport;
pub use ids::{ConnId, DeviceId, HostId, InviteId, RequestId};
pub use keys::{DeviceKeys, HostIdentity, RelayToken, TokenHash};
pub use noise::{
    ConfirmationCode, Established, HandshakePayload, IncomingHandshake, Mode, PairingInitiator,
    PairingResponder, SessionInitiator, SessionResponder, MIN_PROTOCOL_VERSION, PROTOCOL_VERSION,
};
pub use pairing::{derive_invite_token, derive_psk, InviteToken, PairingOffer, PairingSecret, Psk};
pub use relay_auth::{HostAuthError, HostAuthHeaders};
pub use relay_wire::{
    CloseCode, ConnOpen, ConnPeer, DataFrame, FrameError, HostToRelay, RelayToHost, UpgradeError,
};
pub use rpc::{ErrorCode, Message, RpcError};

/// Returns true when JSON text starts with `{`.
///
/// Serde can read a struct, and an enum with a `t` tag, from a JSON array.
/// Every JSON value of the contract is an object, so the readers reject other values first.
pub(crate) fn is_json_object(json: &[u8]) -> bool {
    json.iter().find(|byte| !byte.is_ascii_whitespace()) == Some(&b'{')
}

/// Returns the current time in epoch milliseconds. A clock before 1970 gives 0.
pub fn unix_time_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The network engines move these values between async tasks.
    #[test]
    fn protocol_state_is_send_and_sync() {
        fn check<T: Send + Sync>() {}
        check::<HostIdentity>();
        check::<DeviceKeys>();
        check::<PairingOffer>();
        check::<PairingInitiator>();
        check::<PairingResponder>();
        check::<SessionInitiator>();
        check::<SessionResponder>();
        check::<IncomingHandshake>();
        check::<Established>();
        check::<Transport>();
        check::<Message>();
        check::<Error>();
    }
}
