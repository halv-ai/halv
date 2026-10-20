//! The Noise handshakes (section 7).
//!
//! The device is the initiator. The host is the responder.
//! The device knows the host static key before the handshake.
//!
//! | Use | Noise protocol | Device type | Host type |
//! | --- | --- | --- | --- |
//! | Pairing | `Noise_IKpsk1_25519_ChaChaPoly_BLAKE2s` | [`PairingInitiator`] | [`PairingResponder`] |
//! | Session | `Noise_IK_25519_ChaChaPoly_BLAKE2s` | [`SessionInitiator`] | [`SessionResponder`] |
//!
//! Each step consumes the handshake value, so a failed step ends the handshake.
//! The host reads message 1 into an [`IncomingHandshake`].
//! It shows the device static key, so the host can check its device registry
//! before it writes message 2. To reject the device, the host drops the value.
//! A successful handshake gives an [`Established`] with a [`Transport`].
//!
//! Both sides reject a low-order static key with [`HandshakeFailure::LowOrderKey`].
//! `snow` accepts such a key, but a Diffie-Hellman with it gives zero, so it proves nothing.
//!
//! `snow` keeps internal copies of the keys. This crate cannot zeroize them.

use std::fmt;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use snow::HandshakeState;

use crate::b64u;
use crate::error::{Error, HandshakeFailure, Result};
use crate::framing::Transport;
use crate::ids::HostId;
use crate::keys::{is_low_order_x25519, DeviceKeys, HostIdentity};
use crate::pairing::{PairingOffer, Psk};
use crate::relay_wire::MAX_NOISE_MESSAGE;

/// The Noise protocol of the pairing handshake.
pub const PAIRING_PROTOCOL: &str = "Noise_IKpsk1_25519_ChaChaPoly_BLAKE2s";

/// The Noise protocol of the session handshake.
pub const SESSION_PROTOCOL: &str = "Noise_IK_25519_ChaChaPoly_BLAKE2s";

/// The PSK position of the pairing handshake (`psk1`).
pub const PSK_POSITION: u8 = 1;

/// The domain prefix of the prologue (section 7.2).
pub const PROLOGUE_DOMAIN: &[u8] = b"halv-remote/noise/v1";

/// The domain prefix of the confirmation code (section 7.3.1).
pub const CONFIRMATION_DOMAIN: &[u8] = b"halv-remote/sas/v1";

/// The highest protocol version of this crate.
pub const PROTOCOL_VERSION: u64 = 1;

/// The lowest protocol version of this crate.
pub const MIN_PROTOCOL_VERSION: u64 = 1;

/// The CBOR nesting limit of a handshake payload.
const PAYLOAD_RECURSION_LIMIT: usize = 16;

/// The handshake mode in the prologue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Mode {
    /// The pairing handshake (`Noise_IKpsk1`).
    Pair,
    /// The session handshake (`Noise_IK`).
    Session,
}

impl Mode {
    /// Returns the mode text of the prologue: `pair` or `session`.
    pub const fn as_str(self) -> &'static str {
        match self {
            Mode::Pair => "pair",
            Mode::Session => "session",
        }
    }

    /// Returns the Noise protocol name of the mode.
    pub const fn protocol(self) -> &'static str {
        match self {
            Mode::Pair => PAIRING_PROTOCOL,
            Mode::Session => SESSION_PROTOCOL,
        }
    }
}

/// Returns the prologue of section 7.2:
///
/// `"halv-remote/noise/v1" || 0x00 || mode || 0x00 || host_id`
pub fn prologue(mode: Mode, host_id: &HostId) -> Vec<u8> {
    let mode = mode.as_str().as_bytes();
    let host_id = host_id.as_str().as_bytes();
    let mut out = Vec::with_capacity(PROLOGUE_DOMAIN.len() + mode.len() + host_id.len() + 2);
    out.extend_from_slice(PROLOGUE_DOMAIN);
    out.push(0x00);
    out.extend_from_slice(mode);
    out.push(0x00);
    out.extend_from_slice(host_id);
    out
}

/// A handshake payload: P1 from the device, P2 from the host (section 7.5).
///
/// The CBOR map is `{ "v": 1, "min": 1, "app": "halv-mobile/0.1.0" }`.
/// Unknown fields are ignored.
/// P1 has no forward secrecy and an attacker can replay it, so it never holds secrets or commands.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HandshakePayload {
    /// The highest protocol version of the sender.
    pub v: u64,
    /// The lowest protocol version of the sender.
    pub min: u64,
    /// The app name and version, for example `halv-desktop/0.8.0`.
    pub app: String,
}

impl HandshakePayload {
    /// Makes a payload with the version window of this crate.
    pub fn new(app: impl Into<String>) -> HandshakePayload {
        HandshakePayload {
            v: PROTOCOL_VERSION,
            min: MIN_PROTOCOL_VERSION,
            app: app.into(),
        }
    }

    /// Writes the CBOR map.
    pub fn to_cbor(&self) -> Result<Vec<u8>> {
        let mut out = Vec::new();
        ciborium::into_writer(self, &mut out).map_err(|_| Error::Cbor("handshake payload"))?;
        Ok(out)
    }

    /// Reads the CBOR map. Unknown fields are ignored. Bytes after the map fail.
    pub fn from_cbor(bytes: &[u8]) -> Result<HandshakePayload> {
        let mut reader = bytes;
        let payload =
            ciborium::de::from_reader_with_recursion_limit(&mut reader, PAYLOAD_RECURSION_LIMIT)
                .map_err(|_| Error::Cbor("handshake payload"))?;
        if !reader.is_empty() {
            return Err(Error::Cbor("bytes after the handshake payload"));
        }
        Ok(payload)
    }

    /// Returns true when the version windows overlap (section 7.5):
    /// `peer.v >= self.min` and `self.v >= peer.min`.
    ///
    /// A window with `min` greater than `v` is empty and overlaps nothing.
    pub fn overlaps(&self, peer: &HandshakePayload) -> bool {
        self.min <= self.v && peer.min <= peer.v && peer.v >= self.min && self.v >= peer.min
    }

    /// Returns the highest version in both windows, or `None` when they do not overlap.
    pub fn common_version(&self, peer: &HandshakePayload) -> Option<u64> {
        self.overlaps(peer).then(|| self.v.min(peer.v))
    }
}

/// The 6-digit confirmation code of a pairing (section 7.3.1).
///
/// `code = decimal(u32_be(SHA-256("halv-remote/sas/v1" || 0x00 || handshake_hash)[0..4]) mod 1,000,000)`
///
/// Both peers compute it after the pairing handshake. The user checks that the codes match.
/// `Display` writes 6 digits with leading zeros, for example `028809`.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct ConfirmationCode(u32);

impl ConfirmationCode {
    /// Computes the code from the Noise handshake hash after message 2.
    pub fn from_handshake_hash(handshake_hash: &[u8]) -> ConfirmationCode {
        let digest = Sha256::new()
            .chain_update(CONFIRMATION_DOMAIN)
            .chain_update([0x00])
            .chain_update(handshake_hash)
            .finalize();
        let prefix = u32::from_be_bytes([digest[0], digest[1], digest[2], digest[3]]);
        ConfirmationCode(prefix % 1_000_000)
    }

    /// Returns the code as a number below 1,000,000.
    pub const fn value(self) -> u32 {
        self.0
    }

    /// Returns the code as two groups of 3 digits for the screen, for example `028 809`.
    pub fn grouped(self) -> String {
        format!("{:03} {:03}", self.0 / 1000, self.0 % 1000)
    }
}

impl fmt::Display for ConfirmationCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:06}", self.0)
    }
}

impl fmt::Debug for ConfirmationCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ConfirmationCode({:06})", self.0)
    }
}

/// The result of a successful handshake.
pub struct Established {
    /// The encrypted channel.
    pub transport: Transport,
    /// The handshake payload that this side sent.
    pub local_payload: HandshakePayload,
    /// The handshake payload of the peer.
    pub peer_payload: HandshakePayload,
    /// The static public key of the peer.
    /// For the host it is the device key. For the device it is the pinned host key.
    pub remote_static: [u8; 32],
    /// The Noise handshake hash after message 2.
    pub handshake_hash: [u8; 32],
}

impl Established {
    /// Returns true when the version windows of both payloads overlap (section 7.5).
    ///
    /// If they do not overlap, the host closes the connection with `4426`
    /// after it sends message 2, and the device asks the user to update.
    pub fn versions_overlap(&self) -> bool {
        self.local_payload.overlaps(&self.peer_payload)
    }

    /// Returns the confirmation code of section 7.3.1. It is used after a pairing handshake.
    pub fn confirmation_code(&self) -> ConfirmationCode {
        ConfirmationCode::from_handshake_hash(&self.handshake_hash)
    }
}

impl fmt::Debug for Established {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Established")
            .field("transport", &self.transport)
            .field("local_payload", &self.local_payload)
            .field("peer_payload", &self.peer_payload)
            .field("remote_static", &b64u::encode(&self.remote_static))
            .finish_non_exhaustive()
    }
}

fn failure(kind: HandshakeFailure) -> Error {
    Error::Handshake(kind)
}

/// Makes a handshake state. `remote_public` is set for the initiator only.
fn build(
    mode: Mode,
    host_id: &HostId,
    local_secret: &[u8; 32],
    remote_public: Option<&[u8; 32]>,
    psk: Option<&Psk>,
) -> Result<HandshakeState> {
    let params = mode
        .protocol()
        .parse()
        .map_err(|_| failure(HandshakeFailure::Internal))?;
    let prologue = prologue(mode, host_id);
    let mut builder = snow::Builder::new(params)
        .prologue(&prologue)?
        .local_private_key(local_secret)?;
    if let Some(psk) = psk {
        builder = builder.psk(PSK_POSITION, psk.as_bytes())?;
    }
    let state = match remote_public {
        Some(remote) => builder.remote_public_key(remote)?.build_initiator()?,
        None => builder.build_responder()?,
    };
    Ok(state)
}

fn write_message(state: &mut HandshakeState, payload: &HandshakePayload) -> Result<Vec<u8>> {
    let payload = payload.to_cbor()?;
    let mut message = vec![0u8; MAX_NOISE_MESSAGE];
    let len = state.write_message(&payload, &mut message)?;
    message.truncate(len);
    Ok(message)
}

fn read_message(state: &mut HandshakeState, message: &[u8]) -> Result<HandshakePayload> {
    if message.len() > MAX_NOISE_MESSAGE {
        return Err(failure(HandshakeFailure::Malformed));
    }
    let mut payload = vec![0u8; MAX_NOISE_MESSAGE];
    let len = state.read_message(message, &mut payload)?;
    HandshakePayload::from_cbor(&payload[..len]).map_err(|_| failure(HandshakeFailure::Payload))
}

fn remote_static(state: &HandshakeState) -> Result<[u8; 32]> {
    state
        .get_remote_static()
        .and_then(|key| <[u8; 32]>::try_from(key).ok())
        .ok_or(failure(HandshakeFailure::Internal))
}

/// Ends a handshake after its last message.
fn establish(
    state: HandshakeState,
    local_payload: HandshakePayload,
    peer_payload: HandshakePayload,
    remote_static: [u8; 32],
) -> Result<Established> {
    if !state.is_handshake_finished() {
        return Err(failure(HandshakeFailure::State));
    }
    let handshake_hash: [u8; 32] = state
        .get_handshake_hash()
        .try_into()
        .map_err(|_| failure(HandshakeFailure::Internal))?;
    let transport = Transport::new(state.into_transport_mode()?);
    Ok(Established {
        transport,
        local_payload,
        peer_payload,
        remote_static,
        handshake_hash,
    })
}

/// The device side of a handshake after message 1.
struct InitiatorState {
    state: HandshakeState,
    local_payload: HandshakePayload,
    host_noise_public: [u8; 32],
}

impl InitiatorState {
    fn start(
        mode: Mode,
        host_id: &HostId,
        host_noise_public: &[u8; 32],
        device: &DeviceKeys,
        psk: Option<&Psk>,
        payload: HandshakePayload,
    ) -> Result<(InitiatorState, Vec<u8>)> {
        if is_low_order_x25519(host_noise_public) {
            return Err(failure(HandshakeFailure::LowOrderKey));
        }
        let mut state = build(
            mode,
            host_id,
            device.noise_secret(),
            Some(host_noise_public),
            psk,
        )?;
        let message = write_message(&mut state, &payload)?;
        Ok((
            InitiatorState {
                state,
                local_payload: payload,
                host_noise_public: *host_noise_public,
            },
            message,
        ))
    }

    fn read_message_2(mut self, message: &[u8]) -> Result<Established> {
        let peer_payload = read_message(&mut self.state, message)?;
        establish(
            self.state,
            self.local_payload,
            peer_payload,
            self.host_noise_public,
        )
    }
}

/// The device side of the pairing handshake (section 7.3).
pub struct PairingInitiator(InitiatorState);

impl PairingInitiator {
    /// Writes message 1 (`e, es, s, ss, psk`) with the payload P1.
    ///
    /// The offer gives the host id, the pinned host key, and the PSK.
    /// `device` holds the new keys that the device made for this host.
    pub fn start(
        offer: &PairingOffer,
        device: &DeviceKeys,
        payload: HandshakePayload,
    ) -> Result<(PairingInitiator, Vec<u8>)> {
        let psk = offer.psk();
        let (state, message) = InitiatorState::start(
            Mode::Pair,
            offer.host_id(),
            offer.host_noise_public(),
            device,
            Some(&psk),
            payload,
        )?;
        Ok((PairingInitiator(state), message))
    }

    /// Reads message 2 (`e, ee, se`) with the payload P2.
    ///
    /// Then the device shows [`Established::confirmation_code`] and sends `pair.request`.
    pub fn read_message_2(self, message: &[u8]) -> Result<Established> {
        self.0.read_message_2(message)
    }
}

impl fmt::Debug for PairingInitiator {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PairingInitiator").finish_non_exhaustive()
    }
}

/// The device side of the session handshake (section 7.4).
pub struct SessionInitiator(InitiatorState);

impl SessionInitiator {
    /// Writes message 1 (`e, es, s, ss`) with the payload P1.
    ///
    /// `host_noise_public` is the host key that the device pinned at pairing.
    /// A low-order host key fails with [`HandshakeFailure::LowOrderKey`].
    pub fn start(
        host_id: &HostId,
        host_noise_public: &[u8; 32],
        device: &DeviceKeys,
        payload: HandshakePayload,
    ) -> Result<(SessionInitiator, Vec<u8>)> {
        let (state, message) = InitiatorState::start(
            Mode::Session,
            host_id,
            host_noise_public,
            device,
            None,
            payload,
        )?;
        Ok((SessionInitiator(state), message))
    }

    /// Reads message 2 (`e, ee, se`) with the payload P2.
    ///
    /// Then the device sends `hello` first.
    pub fn read_message_2(self, message: &[u8]) -> Result<Established> {
        self.0.read_message_2(message)
    }
}

impl fmt::Debug for SessionInitiator {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SessionInitiator").finish_non_exhaustive()
    }
}

/// Reads message 1 on the host side.
///
/// `snow` accepts a low-order device key. With such a key the `ss` and `se` results are
/// zero, so anyone could finish the handshake with it. This function rejects the key.
fn read_message_1(
    mut state: HandshakeState,
    mode: Mode,
    message: &[u8],
) -> Result<IncomingHandshake> {
    let peer_payload = read_message(&mut state, message)?;
    let remote_static = remote_static(&state)?;
    if is_low_order_x25519(&remote_static) {
        return Err(failure(HandshakeFailure::LowOrderKey));
    }
    Ok(IncomingHandshake {
        state,
        mode,
        peer_payload,
        remote_static,
    })
}

/// The host side of the pairing handshake (section 7.3).
pub struct PairingResponder(HandshakeState);

impl PairingResponder {
    /// Prepares to read message 1 with the PSK of the invite of the connection.
    pub fn new(identity: &HostIdentity, psk: &Psk) -> Result<PairingResponder> {
        let state = build(
            Mode::Pair,
            &identity.host_id(),
            identity.noise_secret(),
            None,
            Some(psk),
        )?;
        Ok(PairingResponder(state))
    }

    /// Reads message 1. A wrong PSK fails here with [`HandshakeFailure::Decrypt`].
    /// A low-order device key fails with [`HandshakeFailure::LowOrderKey`].
    /// The host then closes the connection with `4401`.
    pub fn read_message_1(self, message: &[u8]) -> Result<IncomingHandshake> {
        read_message_1(self.0, Mode::Pair, message)
    }
}

impl fmt::Debug for PairingResponder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PairingResponder").finish_non_exhaustive()
    }
}

/// The host side of the session handshake (section 7.4).
pub struct SessionResponder(HandshakeState);

impl SessionResponder {
    /// Prepares to read message 1.
    pub fn new(identity: &HostIdentity) -> Result<SessionResponder> {
        let state = build(
            Mode::Session,
            &identity.host_id(),
            identity.noise_secret(),
            None,
            None,
        )?;
        Ok(SessionResponder(state))
    }

    /// Reads message 1 and learns the device static key.
    ///
    /// The host must then check [`IncomingHandshake::remote_static`] in its device registry.
    /// A low-order device key fails with [`HandshakeFailure::LowOrderKey`].
    pub fn read_message_1(self, message: &[u8]) -> Result<IncomingHandshake> {
        read_message_1(self.0, Mode::Session, message)
    }
}

impl fmt::Debug for SessionResponder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SessionResponder").finish_non_exhaustive()
    }
}

/// A handshake after the host read message 1.
///
/// The host checks the device before it writes message 2:
///
/// - In a session (section 7.4), [`IncomingHandshake::remote_static`] must belong to a paired
///   device that is not revoked, and that device id must equal the device id of `conn_open`.
///   Otherwise the host drops this value and closes the connection with `4401`.
/// - In a pairing, the PSK already proved the invite. The key is the new device key.
///
/// [`IncomingHandshake::accept`] writes message 2.
/// The host writes message 2 also when the version windows do not overlap,
/// and then closes the connection with `4426` (section 7.5).
pub struct IncomingHandshake {
    state: HandshakeState,
    mode: Mode,
    peer_payload: HandshakePayload,
    remote_static: [u8; 32],
}

impl IncomingHandshake {
    /// Returns the handshake mode.
    pub fn mode(&self) -> Mode {
        self.mode
    }

    /// Returns the static public key of the device.
    pub fn remote_static(&self) -> &[u8; 32] {
        &self.remote_static
    }

    /// Returns the payload P1 of the device.
    pub fn peer_payload(&self) -> &HandshakePayload {
        &self.peer_payload
    }

    /// Writes message 2 (`e, ee, se`) with the payload P2 and ends the handshake.
    pub fn accept(mut self, payload: HandshakePayload) -> Result<(Vec<u8>, Established)> {
        let message = write_message(&mut self.state, &payload)?;
        let established = establish(self.state, payload, self.peer_payload, self.remote_static)?;
        Ok((message, established))
    }
}

impl fmt::Debug for IncomingHandshake {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("IncomingHandshake")
            .field("mode", &self.mode)
            .field("peer_payload", &self.peer_payload)
            .field("remote_static", &b64u::encode(&self.remote_static))
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use snow::resolvers::{CryptoResolver, DefaultResolver};

    use super::*;
    use crate::pairing::PairingSecret;

    const NOW: u64 = 1_760_460_000_000;

    fn host() -> HostIdentity {
        HostIdentity::from_secrets([0x11; 32], [0x22; 32], NOW)
    }

    fn offer_for(identity: &HostIdentity, secret: [u8; 32]) -> PairingOffer {
        PairingOffer::new(
            "wss://relay.halv.ai",
            identity.host_id(),
            identity.noise_public(),
            PairingSecret::from_bytes(secret),
            NOW + 300_000,
            None,
        )
        .unwrap()
    }

    fn p1() -> HandshakePayload {
        HandshakePayload::new("halv-mobile/0.1.0")
    }

    fn p2() -> HandshakePayload {
        HandshakePayload::new("halv-desktop/0.8.0")
    }

    #[test]
    fn prologue_has_the_contract_bytes() {
        let host_id = HostId::parse("nYqEorAQnfAioCLxARz8mA").unwrap();
        assert_eq!(
            prologue(Mode::Pair, &host_id),
            b"halv-remote/noise/v1\0pair\0nYqEorAQnfAioCLxARz8mA".to_vec()
        );
        assert_eq!(
            prologue(Mode::Session, &host_id),
            b"halv-remote/noise/v1\0session\0nYqEorAQnfAioCLxARz8mA".to_vec()
        );
    }

    #[test]
    fn protocol_names_parse() {
        for mode in [Mode::Pair, Mode::Session] {
            assert!(mode.protocol().parse::<snow::params::NoiseParams>().is_ok());
        }
    }

    #[test]
    fn confirmation_code_matches_vector_v5() {
        let code = ConfirmationCode::from_handshake_hash(&[0x11; 32]);
        assert_eq!(code.value(), 280_809);
        assert_eq!(code.to_string(), "280809");
        assert_eq!(code.grouped(), "280 809");
        assert_eq!(ConfirmationCode(28_809).to_string(), "028809");
        assert_eq!(ConfirmationCode(28_809).grouped(), "028 809");
        assert_eq!(ConfirmationCode(7).grouped(), "000 007");
    }

    #[test]
    fn payload_cbor_round_trip_and_unknown_fields() {
        let payload = p1();
        let bytes = payload.to_cbor().unwrap();
        assert_eq!(HandshakePayload::from_cbor(&bytes).unwrap(), payload);

        #[derive(Serialize)]
        struct Future<'a> {
            extra: Vec<u8>,
            v: u64,
            min: u64,
            app: &'a str,
            more: bool,
        }
        let mut future = Vec::new();
        ciborium::into_writer(
            &Future {
                extra: vec![1, 2],
                v: 3,
                min: 1,
                app: "halv-mobile/9.0.0",
                more: true,
            },
            &mut future,
        )
        .unwrap();
        let decoded = HandshakePayload::from_cbor(&future).unwrap();
        assert_eq!((decoded.v, decoded.min), (3, 1));

        let mut trailing = bytes.clone();
        trailing.push(0);
        assert!(HandshakePayload::from_cbor(&trailing).is_err());
        assert!(HandshakePayload::from_cbor(&bytes[..bytes.len() - 1]).is_err());
        assert!(HandshakePayload::from_cbor(&[0xa0]).is_err());
    }

    #[test]
    fn version_windows() {
        let window = |v, min| HandshakePayload {
            v,
            min,
            app: String::new(),
        };
        let this = window(1, 1);
        assert!(this.overlaps(&window(1, 1)));
        assert!(this.overlaps(&window(3, 1)));
        assert_eq!(this.common_version(&window(3, 1)), Some(1));
        assert!(!this.overlaps(&window(3, 2)));
        assert!(!window(3, 2).overlaps(&this));
        assert_eq!(this.common_version(&window(3, 2)), None);
        assert!(window(4, 2).overlaps(&window(2, 1)));
        assert_eq!(window(4, 2).common_version(&window(3, 1)), Some(3));
        // An empty window overlaps nothing.
        assert!(!window(5, 1).overlaps(&window(2, 3)));
        assert!(!window(2, 3).overlaps(&window(5, 1)));
    }

    #[test]
    fn pairing_handshake_works() {
        let identity = host();
        let offer = offer_for(&identity, [0x42; 32]);
        let device = DeviceKeys::generate().unwrap();

        let (initiator, message_1) = PairingInitiator::start(&offer, &device, p1()).unwrap();
        let responder = PairingResponder::new(&identity, &offer.psk()).unwrap();
        let incoming = responder.read_message_1(&message_1).unwrap();
        assert_eq!(incoming.mode(), Mode::Pair);
        assert_eq!(incoming.remote_static(), &device.noise_public());
        assert_eq!(incoming.peer_payload(), &p1());

        let (message_2, mut host_side) = incoming.accept(p2()).unwrap();
        let mut device_side = initiator.read_message_2(&message_2).unwrap();

        assert_eq!(device_side.peer_payload, p2());
        assert_eq!(device_side.remote_static, identity.noise_public());
        assert_eq!(host_side.remote_static, device.noise_public());
        assert_eq!(device_side.handshake_hash, host_side.handshake_hash);
        assert_eq!(
            device_side.confirmation_code(),
            host_side.confirmation_code()
        );
        assert!(device_side.versions_overlap());
        assert!(host_side.versions_overlap());

        let sealed = device_side.transport.seal(b"pair.request").unwrap();
        assert_eq!(
            host_side.transport.open(&sealed[0]).unwrap(),
            Some(b"pair.request".to_vec())
        );
        let sealed = host_side.transport.seal(b"answer").unwrap();
        assert_eq!(
            device_side.transport.open(&sealed[0]).unwrap(),
            Some(b"answer".to_vec())
        );
    }

    #[test]
    fn session_handshake_works() {
        let identity = host();
        let device = DeviceKeys::generate().unwrap();

        let (initiator, message_1) =
            SessionInitiator::start(&identity.host_id(), &identity.noise_public(), &device, p1())
                .unwrap();
        let incoming = SessionResponder::new(&identity)
            .unwrap()
            .read_message_1(&message_1)
            .unwrap();
        assert_eq!(incoming.mode(), Mode::Session);
        assert_eq!(incoming.remote_static(), &device.noise_public());

        let (message_2, mut host_side) = incoming.accept(p2()).unwrap();
        let mut device_side = initiator.read_message_2(&message_2).unwrap();
        assert_eq!(device_side.handshake_hash, host_side.handshake_hash);

        let sealed = device_side.transport.seal(b"hello").unwrap();
        assert_eq!(
            host_side.transport.open(&sealed[0]).unwrap(),
            Some(b"hello".to_vec())
        );
    }

    #[test]
    fn message_sizes() {
        let identity = host();
        let device = DeviceKeys::generate().unwrap();
        let payload_len = p1().to_cbor().unwrap().len();
        let (initiator, message_1) =
            SessionInitiator::start(&identity.host_id(), &identity.noise_public(), &device, p1())
                .unwrap();
        // e (32) + encrypted s (48) + encrypted payload (payload + 16).
        assert_eq!(message_1.len(), 96 + payload_len);
        let incoming = SessionResponder::new(&identity)
            .unwrap()
            .read_message_1(&message_1)
            .unwrap();
        let (message_2, _) = incoming.accept(p2()).unwrap();
        // e (32) + encrypted payload (payload + 16).
        assert_eq!(message_2.len(), 48 + p2().to_cbor().unwrap().len());
        assert!(initiator.read_message_2(&message_2).is_ok());
    }

    #[test]
    fn wrong_psk_fails_at_message_1() {
        let identity = host();
        let device_offer = offer_for(&identity, [0x42; 32]);
        let host_offer = offer_for(&identity, [0x43; 32]);
        let device = DeviceKeys::generate().unwrap();

        let (_, message_1) = PairingInitiator::start(&device_offer, &device, p1()).unwrap();
        let error = PairingResponder::new(&identity, &host_offer.psk())
            .unwrap()
            .read_message_1(&message_1)
            .unwrap_err();
        assert_eq!(error, Error::Handshake(HandshakeFailure::Decrypt));
    }

    #[test]
    fn wrong_host_key_fails_at_message_1() {
        let identity = host();
        let other = HostIdentity::from_secrets([0x33; 32], [0x22; 32], NOW);
        let device = DeviceKeys::generate().unwrap();
        // The device pins the key of another host, with the right host id.
        let (_, message_1) =
            SessionInitiator::start(&identity.host_id(), &other.noise_public(), &device, p1())
                .unwrap();
        let error = SessionResponder::new(&identity)
            .unwrap()
            .read_message_1(&message_1)
            .unwrap_err();
        assert_eq!(error, Error::Handshake(HandshakeFailure::Decrypt));
    }

    #[test]
    fn wrong_prologue_fails_at_message_1() {
        let identity = host();
        let device = DeviceKeys::generate().unwrap();
        let wrong_host_id = HostId::parse("AAAAAAAAAAAAAAAAAAAAAA").unwrap();
        let (_, message_1) =
            SessionInitiator::start(&wrong_host_id, &identity.noise_public(), &device, p1())
                .unwrap();
        assert_eq!(
            SessionResponder::new(&identity)
                .unwrap()
                .read_message_1(&message_1)
                .unwrap_err(),
            Error::Handshake(HandshakeFailure::Decrypt)
        );
    }

    #[test]
    fn modes_do_not_mix() {
        let identity = host();
        let offer = offer_for(&identity, [0x42; 32]);
        let device = DeviceKeys::generate().unwrap();

        // A session message 1 does not open a pairing responder.
        let (_, session_1) =
            SessionInitiator::start(&identity.host_id(), &identity.noise_public(), &device, p1())
                .unwrap();
        assert!(PairingResponder::new(&identity, &offer.psk())
            .unwrap()
            .read_message_1(&session_1)
            .is_err());

        // A pairing message 1 does not open a session responder.
        let (_, pairing_1) = PairingInitiator::start(&offer, &device, p1()).unwrap();
        assert!(SessionResponder::new(&identity)
            .unwrap()
            .read_message_1(&pairing_1)
            .is_err());
    }

    #[test]
    fn unknown_device_key_is_visible_before_message_2() {
        let identity = host();
        let paired = DeviceKeys::generate().unwrap();
        let stranger = DeviceKeys::generate().unwrap();
        let registry = [paired.noise_public()];

        let (initiator, message_1) = SessionInitiator::start(
            &identity.host_id(),
            &identity.noise_public(),
            &stranger,
            p1(),
        )
        .unwrap();
        let incoming = SessionResponder::new(&identity)
            .unwrap()
            .read_message_1(&message_1)
            .unwrap();
        // The host sees the unknown key before it writes anything, and drops the handshake.
        assert_eq!(incoming.remote_static(), &stranger.noise_public());
        assert!(!registry.contains(incoming.remote_static()));
        drop(incoming);

        // Without message 2, the device cannot finish: a made-up message 2 fails.
        assert!(initiator.read_message_2(&[0u8; 64]).is_err());
    }

    /// A `snow` resolver whose static key has the public value 0, a point of order 2.
    /// The ephemeral key stays real. A device that follows the protocol never has such a key.
    struct LowOrderStaticResolver;

    struct LowOrderStaticDh {
        inner: Box<dyn snow::types::Dh>,
        fake: bool,
    }

    impl snow::types::Dh for LowOrderStaticDh {
        fn name(&self) -> &'static str {
            self.inner.name()
        }

        fn pub_len(&self) -> usize {
            self.inner.pub_len()
        }

        fn priv_len(&self) -> usize {
            self.inner.priv_len()
        }

        fn set(&mut self, privkey: &[u8]) {
            // `snow` sets only the static key from bytes. It generates the ephemeral key.
            self.inner.set(privkey);
            self.fake = true;
        }

        fn generate(&mut self, rng: &mut dyn snow::types::Random) -> Result<(), snow::Error> {
            self.inner.generate(rng)
        }

        fn pubkey(&self) -> &[u8] {
            if self.fake {
                &[0u8; 32]
            } else {
                self.inner.pubkey()
            }
        }

        fn privkey(&self) -> &[u8] {
            self.inner.privkey()
        }

        fn dh(&self, pubkey: &[u8], out: &mut [u8]) -> Result<(), snow::Error> {
            if self.fake {
                // A Diffie-Hellman with a low-order key gives zero for every peer key.
                out[..32].fill(0);
                Ok(())
            } else {
                self.inner.dh(pubkey, out)
            }
        }
    }

    impl CryptoResolver for LowOrderStaticResolver {
        fn resolve_rng(&self) -> Option<Box<dyn snow::types::Random>> {
            DefaultResolver.resolve_rng()
        }

        fn resolve_dh(&self, choice: &snow::params::DHChoice) -> Option<Box<dyn snow::types::Dh>> {
            let inner = DefaultResolver.resolve_dh(choice)?;
            Some(Box::new(LowOrderStaticDh { inner, fake: false }))
        }

        fn resolve_hash(
            &self,
            choice: &snow::params::HashChoice,
        ) -> Option<Box<dyn snow::types::Hash>> {
            DefaultResolver.resolve_hash(choice)
        }

        fn resolve_cipher(
            &self,
            choice: &snow::params::CipherChoice,
        ) -> Option<Box<dyn snow::types::Cipher>> {
            DefaultResolver.resolve_cipher(choice)
        }
    }

    /// Writes message 1 with a low-order device static key.
    fn low_order_message_1(mode: Mode, identity: &HostIdentity, psk: Option<&Psk>) -> Vec<u8> {
        let prologue = prologue(mode, &identity.host_id());
        let host_key = identity.noise_public();
        let resolver = Box::new(LowOrderStaticResolver);
        let mut builder = snow::Builder::with_resolver(mode.protocol().parse().unwrap(), resolver)
            .prologue(&prologue)
            .unwrap()
            .local_private_key(&[0x33; 32])
            .unwrap()
            .remote_public_key(&host_key)
            .unwrap();
        if let Some(psk) = psk {
            builder = builder.psk(PSK_POSITION, psk.as_bytes()).unwrap();
        }
        let mut state = builder.build_initiator().unwrap();
        let mut message = vec![0u8; 1024];
        let len = state
            .write_message(&p1().to_cbor().unwrap(), &mut message)
            .unwrap();
        message.truncate(len);
        message
    }

    #[test]
    fn low_order_device_key_fails_at_message_1() {
        let identity = host();

        // Plain `snow` accepts the key, so the check of this crate is what rejects it.
        let message_1 = low_order_message_1(Mode::Session, &identity, None);
        let mut raw = build(
            Mode::Session,
            &identity.host_id(),
            identity.noise_secret(),
            None,
            None,
        )
        .unwrap();
        let mut payload = vec![0u8; 1024];
        raw.read_message(&message_1, &mut payload).unwrap();
        assert_eq!(raw.get_remote_static(), Some(&[0u8; 32][..]));

        assert_eq!(
            SessionResponder::new(&identity)
                .unwrap()
                .read_message_1(&message_1)
                .unwrap_err(),
            Error::Handshake(HandshakeFailure::LowOrderKey)
        );

        // In a pairing, the PSK proves the invite, but the key must not enter the registry.
        let offer = offer_for(&identity, [0x42; 32]);
        let message_1 = low_order_message_1(Mode::Pair, &identity, Some(&offer.psk()));
        assert_eq!(
            PairingResponder::new(&identity, &offer.psk())
                .unwrap()
                .read_message_1(&message_1)
                .unwrap_err(),
            Error::Handshake(HandshakeFailure::LowOrderKey)
        );
    }

    #[test]
    fn low_order_host_key_fails_before_message_1() {
        let identity = host();
        let device = DeviceKeys::generate().unwrap();
        // The low-order points u = 0 and u = 1.
        let mut u_one = [0u8; 32];
        u_one[0] = 1;
        for key in [[0u8; 32], u_one] {
            assert_eq!(
                SessionInitiator::start(&identity.host_id(), &key, &device, p1()).unwrap_err(),
                Error::Handshake(HandshakeFailure::LowOrderKey)
            );
        }
    }

    #[test]
    fn revoked_device_key_is_rejected_by_the_registry() {
        struct Entry {
            key: [u8; 32],
            revoked: bool,
        }
        let identity = host();
        let device = DeviceKeys::generate().unwrap();
        let registry = [Entry {
            key: device.noise_public(),
            revoked: true,
        }];
        let allowed = |key: &[u8; 32]| {
            registry
                .iter()
                .any(|entry| &entry.key == key && !entry.revoked)
        };

        let (_, message_1) =
            SessionInitiator::start(&identity.host_id(), &identity.noise_public(), &device, p1())
                .unwrap();
        let incoming = SessionResponder::new(&identity)
            .unwrap()
            .read_message_1(&message_1)
            .unwrap();
        // The key is known but revoked, so the host never accepts it.
        assert!(!allowed(incoming.remote_static()));
    }

    #[test]
    fn changed_handshake_messages_fail() {
        let identity = host();
        let device = DeviceKeys::generate().unwrap();
        let start = || {
            SessionInitiator::start(&identity.host_id(), &identity.noise_public(), &device, p1())
                .unwrap()
        };
        let responder = || SessionResponder::new(&identity).unwrap();

        // Message 1: a changed byte in the encrypted static key or the payload, or a cut.
        let (_, message_1) = start();
        for index in [40, message_1.len() - 1] {
            let mut changed = message_1.clone();
            changed[index] ^= 1;
            assert_eq!(
                responder().read_message_1(&changed).unwrap_err(),
                Error::Handshake(HandshakeFailure::Decrypt)
            );
        }
        assert_eq!(
            responder().read_message_1(&message_1[..20]).unwrap_err(),
            Error::Handshake(HandshakeFailure::Malformed)
        );
        assert!(responder().read_message_1(&[]).is_err());
        assert!(responder()
            .read_message_1(&vec![0u8; MAX_NOISE_MESSAGE + 1])
            .is_err());

        // Message 2: a changed byte fails on the device.
        let (initiator, message_1) = start();
        let (message_2, _) = responder()
            .read_message_1(&message_1)
            .unwrap()
            .accept(p2())
            .unwrap();
        let mut changed = message_2.clone();
        changed[message_2.len() - 1] ^= 1;
        assert_eq!(
            initiator.read_message_2(&changed).unwrap_err(),
            Error::Handshake(HandshakeFailure::Decrypt)
        );
    }

    #[test]
    fn replayed_message_1_cannot_finish() {
        let identity = host();
        let device = DeviceKeys::generate().unwrap();
        let (initiator, message_1) =
            SessionInitiator::start(&identity.host_id(), &identity.noise_public(), &device, p1())
                .unwrap();
        // The first answer completes the real handshake.
        let (message_2, _) = SessionResponder::new(&identity)
            .unwrap()
            .read_message_1(&message_1)
            .unwrap()
            .accept(p2())
            .unwrap();
        // A replay of message 1 gets a new message 2 with a new ephemeral key.
        let (replay_2, mut replay_host) = SessionResponder::new(&identity)
            .unwrap()
            .read_message_1(&message_1)
            .unwrap()
            .accept(p2())
            .unwrap();
        assert_ne!(message_2, replay_2);
        let mut device_side = initiator.read_message_2(&message_2).unwrap();
        // The replayed session does not share keys with the real device.
        let sealed = replay_host.transport.seal(b"command").unwrap();
        assert_eq!(
            device_side.transport.open(&sealed[0]).unwrap_err(),
            Error::Decrypt
        );
    }

    #[test]
    fn bad_payload_fails_the_handshake() {
        let identity = host();
        let device = DeviceKeys::generate().unwrap();
        // A raw snow initiator that sends a payload that is not CBOR of section 7.5.
        let prologue = prologue(Mode::Session, &identity.host_id());
        let mut state = snow::Builder::new(SESSION_PROTOCOL.parse().unwrap())
            .prologue(&prologue)
            .unwrap()
            .local_private_key(device.noise_secret())
            .unwrap()
            .remote_public_key(&identity.noise_public())
            .unwrap()
            .build_initiator()
            .unwrap();
        let mut message = vec![0u8; 1024];
        let len = state.write_message(b"not cbor", &mut message).unwrap();
        assert_eq!(
            SessionResponder::new(&identity)
                .unwrap()
                .read_message_1(&message[..len])
                .unwrap_err(),
            Error::Handshake(HandshakeFailure::Payload)
        );
    }

    #[test]
    fn host_answers_an_old_device_and_reports_no_overlap() {
        let identity = host();
        let device = DeviceKeys::generate().unwrap();
        let newer = HandshakePayload {
            v: 4,
            min: 3,
            app: "halv-mobile/9.0.0".to_owned(),
        };
        let (initiator, message_1) = SessionInitiator::start(
            &identity.host_id(),
            &identity.noise_public(),
            &device,
            newer,
        )
        .unwrap();
        let incoming = SessionResponder::new(&identity)
            .unwrap()
            .read_message_1(&message_1)
            .unwrap();
        assert!(!p2().overlaps(incoming.peer_payload()));
        // The host still writes message 2, then closes with 4426.
        let (message_2, host_side) = incoming.accept(p2()).unwrap();
        assert!(!host_side.versions_overlap());
        let device_side = initiator.read_message_2(&message_2).unwrap();
        assert!(!device_side.versions_overlap());
    }

    #[test]
    fn debug_output_has_no_secrets() {
        let identity = host();
        let offer = offer_for(&identity, [0x42; 32]);
        let device =
            DeviceKeys::from_parts([0x77; 32], crate::keys::RelayToken::from_bytes([0x66; 32]));
        let (initiator, message_1) = PairingInitiator::start(&offer, &device, p1()).unwrap();
        let incoming = PairingResponder::new(&identity, &offer.psk())
            .unwrap()
            .read_message_1(&message_1)
            .unwrap();
        let debug = format!("{initiator:?} {incoming:?}");
        assert!(!debug.contains(&b64u::encode(&[0x77; 32])));
        assert!(!debug.contains(&b64u::encode(offer.psk().as_bytes())));
        assert!(debug.contains(&b64u::encode(&device.noise_public())));
    }
}
