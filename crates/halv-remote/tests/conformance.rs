//! Conformance tests of section 13 (items 1 to 6) and the shared vectors of section 13.1.
//!
//! These tests use only the public API. The module tests cover each rule in more detail.

use halv_remote::framing::MAX_CHUNK_LEN;
use halv_remote::relay_wire::{decode_data, encode_data};
use halv_remote::rpc::{
    self, caps, HelloParams, HelloResult, HostEvent, HostInfo, PairRequest, PairRequestParams,
    PairRequestResult, Pane, PaneKind, PaneList, PaneState, Platform, SessionRequest,
};
use halv_remote::{
    derive_invite_token, derive_psk, relay_auth, CloseCode, ConfirmationCode, ConnId, DeviceId,
    DeviceKeys, Error, HandshakeFailure, HandshakePayload, HostId, HostIdentity, Message,
    OfferError, PairingInitiator, PairingOffer, PairingResponder, PairingSecret, RelayToken,
    RequestId, SessionInitiator, SessionResponder, TokenHash, Transport,
};

const NOW: u64 = 1_760_460_000_000;
const RELAY: &str = "wss://relay.halv.ai";

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn p1() -> HandshakePayload {
    HandshakePayload::new("halv-mobile/0.1.0")
}

fn p2() -> HandshakePayload {
    HandshakePayload::new("halv-desktop/0.8.0")
}

fn request_id(value: u32) -> RequestId {
    RequestId::new(value).unwrap()
}

/// Sends one application message through a transport pair and returns what arrives.
fn send(from: &mut Transport, to: &mut Transport, message: &[u8]) -> Vec<u8> {
    let mut received = None;
    for noise_message in from.seal(message).unwrap() {
        assert!(
            received.is_none(),
            "a message completes only at its last chunk"
        );
        received = to.open(&noise_message).unwrap();
    }
    received.expect("the last Noise message completes the message")
}

/// Wraps a Noise message in a host DATA frame and reads it back, as the relay forwards it.
fn through_relay(conn: ConnId, noise_message: &[u8]) -> Vec<u8> {
    let frame = encode_data(conn, noise_message).unwrap();
    let data = decode_data(&frame).unwrap();
    assert_eq!(data.conn, conn);
    data.payload.to_vec()
}

fn decode_request(bytes: &[u8]) -> rpc::Request {
    match Message::decode(bytes).unwrap() {
        Message::Request(request) => request,
        other => panic!("expected a request, got {other:?}"),
    }
}

fn decode_response(bytes: &[u8]) -> rpc::Response {
    match Message::decode(bytes).unwrap() {
        Message::Response(response) => response,
        other => panic!("expected a response, got {other:?}"),
    }
}

// Section 13.1: shared test vectors.

#[test]
fn vector_v1_host_id() {
    let public: [u8; 32] = std::array::from_fn(|index| index as u8);
    assert_eq!(
        HostId::from_relay_public(&public).as_str(),
        "nYqEorAQnfAioCLxARz8mA"
    );
}

#[test]
fn vector_v2_invite_token_and_psk() {
    let secret = PairingSecret::from_bytes([0x42; 32]);
    assert_eq!(
        hex(derive_invite_token(&secret).as_bytes()),
        "70c315d483322856d11594f882598c861c3900dd734726428bb806d57c86c247"
    );
    assert_eq!(
        hex(derive_psk(&secret).as_bytes()),
        "08ae975f36336b98f5d6f415b0706f143f283d5e5d52d8d0219c8aa9e417a0e2"
    );
}

#[test]
fn vector_v3_token_hash() {
    let token: [u8; 32] = std::array::from_fn(|index| 0x20 + index as u8);
    assert_eq!(
        RelayToken::from_bytes(token).hash().as_str(),
        "ctu3M2x2eAAj-D2kw1Xy7uqFczsT00d2l5F3kMEikIQ"
    );
    assert_eq!(
        TokenHash::of(&token).as_str(),
        "ctu3M2x2eAAj-D2kw1Xy7uqFczsT00d2l5F3kMEikIQ"
    );
}

#[test]
fn vector_v4_signed_message() {
    let public: [u8; 32] = std::array::from_fn(|index| index as u8);
    let host_id = HostId::from_relay_public(&public);
    assert_eq!(
        relay_auth::host_auth_message(&host_id, 1_760_460_000_000),
        "halv-relay/host-auth/v1\nnYqEorAQnfAioCLxARz8mA\n1760460000000"
    );
}

#[test]
fn vector_v5_confirmation_code() {
    let code = ConfirmationCode::from_handshake_hash(&[0x11; 32]);
    assert_eq!(code.to_string(), "280809");
    assert_eq!(code.grouped(), "280 809");
}

// Item 2: pairing URI round trip, and rejection of each invalid field.

#[test]
fn item_2_pairing_uri() {
    let identity = HostIdentity::generate_at(NOW).unwrap();
    let offer = PairingOffer::generate(&identity, RELAY, Some("Studio Mac".into()), NOW).unwrap();
    let parsed = PairingOffer::parse_uri(&offer.to_uri(), NOW).unwrap();
    assert_eq!(parsed.host_id(), &identity.host_id());
    assert_eq!(parsed.host_noise_public(), &identity.noise_public());
    assert_eq!(parsed.psk().as_bytes(), offer.psk().as_bytes());
    assert_eq!(parsed.host_name(), Some("Studio Mac"));

    // The module tests change each field. This test checks the error kinds.
    assert_eq!(
        PairingOffer::parse_uri("halv://pair?p=", NOW).unwrap_err(),
        Error::InvalidOffer(OfferError::Json)
    );
    assert_eq!(
        PairingOffer::parse_uri(&offer.to_uri(), NOW + 300_000 + 60_001).unwrap_err(),
        Error::OfferExpired
    );
    assert_eq!(
        PairingOffer::parse_uri(&offer.to_uri(), NOW - 60_001).unwrap_err(),
        Error::InvalidOffer(OfferError::Expiry)
    );
    assert!(PairingOffer::generate(&identity, "ws://relay.halv.ai", None, NOW).is_err());
}

// Item 3: pairing and session handshakes, end to end through relay DATA frames.

#[test]
fn item_3_pairing_then_session() {
    let identity = HostIdentity::generate_at(NOW).unwrap();
    let offer = PairingOffer::generate(&identity, RELAY, Some("Studio Mac".into()), NOW).unwrap();
    let invite_hash = offer.invite_token().hash();
    let conn = ConnId::new(1).unwrap();

    // Device: scan, make keys, and connect with the invite token.
    let scanned = PairingOffer::parse_uri(&offer.to_uri(), NOW + 1_000).unwrap();
    let bearer = relay_auth::bearer_header_value(scanned.invite_token().as_bytes());
    assert_eq!(relay_auth::bearer_token_hash(&bearer), Some(invite_hash));
    let device = DeviceKeys::generate().unwrap();
    let (initiator, message_1) = PairingInitiator::start(&scanned, &device, p1()).unwrap();

    // Host: read message 1 with the PSK of the invite, then answer.
    let incoming = PairingResponder::new(&identity, &offer.psk())
        .unwrap()
        .read_message_1(&through_relay(conn, &message_1))
        .unwrap();
    assert_eq!(incoming.remote_static(), &device.noise_public());
    let (message_2, mut host) = incoming.accept(p2()).unwrap();
    let mut phone = initiator
        .read_message_2(&through_relay(conn, &message_2))
        .unwrap();
    assert_eq!(phone.confirmation_code(), host.confirmation_code());
    assert!(phone.versions_overlap() && host.versions_overlap());

    // Device: pair.request. Host: allow, store the device, and answer.
    let params = PairRequestParams {
        name: "Ana's phone".into(),
        platform: Platform::Ios,
        model: Some("iPhone".into()),
        token: device.relay_token().clone(),
        app: "halv-mobile/0.1.0".into(),
    };
    let bytes = rpc::encode_call::<PairRequest>(request_id(1), &params).unwrap();
    let bytes = send(&mut phone.transport, &mut host.transport, &bytes);
    let request = decode_request(&bytes);
    let received = PairRequestParams::parse(&request).unwrap();
    let device_id = DeviceId::generate().unwrap();
    let registry = [(device_id.clone(), host.remote_static, received.token.hash())];
    assert_eq!(registry[0].2, device.token_hash());
    let result = PairRequestResult {
        device: device_id.clone(),
        host: "Studio Mac".into(),
    };
    let bytes = rpc::encode_response(request.id, &result).unwrap();
    let bytes = send(&mut host.transport, &mut phone.transport, &bytes);
    let response = decode_response(&bytes);
    assert_eq!(response.id, request_id(1));
    assert_eq!(response.decode_result::<PairRequest>().unwrap(), Ok(result));

    // Session: the device connects again with its relay token and the pinned host key.
    let conn = ConnId::new(2).unwrap();
    let (initiator, message_1) = SessionInitiator::start(
        &identity.host_id(),
        scanned.host_noise_public(),
        &device,
        p1(),
    )
    .unwrap();
    let incoming = SessionResponder::new(&identity)
        .unwrap()
        .read_message_1(&through_relay(conn, &message_1))
        .unwrap();
    // The host finds the key in its registry before it writes message 2.
    let entry = registry
        .iter()
        .find(|(_, key, _)| key == incoming.remote_static())
        .expect("a paired device");
    assert_eq!(entry.0, device_id);
    let (message_2, mut host) = incoming.accept(p2()).unwrap();
    let mut phone = initiator
        .read_message_2(&through_relay(conn, &message_2))
        .unwrap();

    // Device: hello first.
    let hello = HelloParams {
        app: "halv-mobile/0.1.0".into(),
        platform: Platform::Ios,
        caps: vec![],
    };
    let bytes = SessionRequest::Hello(hello.clone())
        .encode(request_id(1))
        .unwrap();
    let bytes = send(&mut phone.transport, &mut host.transport, &bytes);
    let request = decode_request(&bytes);
    assert_eq!(
        SessionRequest::parse(&request).unwrap(),
        SessionRequest::Hello(hello)
    );
    let result = HelloResult {
        host: HostInfo {
            name: "Studio Mac".into(),
            app: "halv-desktop/0.8.0".into(),
            os: "macos".into(),
        },
        device: device_id,
        caps: caps::HOST_V1.iter().map(|cap| (*cap).to_owned()).collect(),
        time: NOW,
    };
    let bytes = rpc::encode_response(request.id, &result).unwrap();
    let bytes = send(&mut host.transport, &mut phone.transport, &bytes);
    assert_eq!(
        decode_response(&bytes)
            .decode_result::<rpc::Hello>()
            .unwrap(),
        Ok(result)
    );

    // Host: an event, and the encrypted revocation.
    let pane = Pane {
        session: "term-1".into(),
        title: "codex".into(),
        kind: PaneKind::Codex,
        project: None,
        cwd: None,
        cols: 80,
        rows: 24,
        state: PaneState::Running,
        exit_code: None,
        last_output_at: None,
        status: None,
    };
    for event in [
        HostEvent::PanesChanged(PaneList {
            panes: vec![pane],
            rev: 1,
        }),
        HostEvent::DeviceRevoked,
    ] {
        let bytes = send(
            &mut host.transport,
            &mut phone.transport,
            &event.encode().unwrap(),
        );
        let Message::Event(received) = Message::decode(&bytes).unwrap() else {
            panic!("expected an event");
        };
        assert_eq!(HostEvent::parse(&received).unwrap(), Some(event));
    }
}

// Item 4: a wrong PSK, an unknown device key, and a revoked device key each fail.

#[test]
fn item_4_wrong_psk_unknown_key_revoked_key() {
    let identity = HostIdentity::generate_at(NOW).unwrap();
    let device = DeviceKeys::generate().unwrap();

    // A wrong PSK fails at message 1.
    let offer = PairingOffer::generate(&identity, RELAY, None, NOW).unwrap();
    let other_offer = PairingOffer::generate(&identity, RELAY, None, NOW).unwrap();
    let (_, message_1) = PairingInitiator::start(&offer, &device, p1()).unwrap();
    assert_eq!(
        PairingResponder::new(&identity, &other_offer.psk())
            .unwrap()
            .read_message_1(&message_1)
            .unwrap_err(),
        Error::Handshake(HandshakeFailure::Decrypt)
    );

    // An unknown device key and a revoked device key: the host sees the key before
    // message 2, finds no allowed entry, and closes with 4401.
    let revoked = DeviceKeys::generate().unwrap();
    let registry = [(revoked.noise_public(), true)];
    for keys in [&device, &revoked] {
        let (initiator, message_1) =
            SessionInitiator::start(&identity.host_id(), &identity.noise_public(), keys, p1())
                .unwrap();
        let incoming = SessionResponder::new(&identity)
            .unwrap()
            .read_message_1(&message_1)
            .unwrap();
        let allowed = registry
            .iter()
            .any(|(key, is_revoked)| key == incoming.remote_static() && !is_revoked);
        assert!(!allowed);
        drop(incoming);
        assert_eq!(CloseCode::Unauthorized.code(), 4401);
        // The device never gets a valid message 2.
        assert!(initiator.read_message_2(&[0u8; 48]).is_err());
    }
}

// Item 5: a replayed, reordered, or changed transport frame ends the connection.

fn session_pair() -> (Transport, Transport) {
    let identity = HostIdentity::generate_at(NOW).unwrap();
    let device = DeviceKeys::generate().unwrap();
    let (initiator, message_1) =
        SessionInitiator::start(&identity.host_id(), &identity.noise_public(), &device, p1())
            .unwrap();
    let (message_2, host) = SessionResponder::new(&identity)
        .unwrap()
        .read_message_1(&message_1)
        .unwrap()
        .accept(p2())
        .unwrap();
    let phone = initiator.read_message_2(&message_2).unwrap();
    (phone.transport, host.transport)
}

#[test]
fn item_5_bad_frames_end_the_connection() {
    // Replayed.
    let (mut phone, mut host) = session_pair();
    let sealed = phone.seal(b"terminal.input").unwrap();
    assert!(host.open(&sealed[0]).unwrap().is_some());
    assert_eq!(host.open(&sealed[0]).unwrap_err(), Error::Decrypt);
    assert!(host.is_poisoned());

    // Reordered.
    let (mut phone, mut host) = session_pair();
    let first = phone.seal(b"first").unwrap();
    let second = phone.seal(b"second").unwrap();
    assert_eq!(host.open(&second[0]).unwrap_err(), Error::Decrypt);
    assert_eq!(host.open(&first[0]).unwrap_err(), Error::Poisoned);

    // Changed.
    let (mut phone, mut host) = session_pair();
    let mut sealed = phone.seal(b"changed").unwrap();
    sealed[0][0] ^= 0x80;
    assert_eq!(host.open(&sealed[0]).unwrap_err(), Error::Decrypt);

    // Truncated.
    let (mut phone, mut host) = session_pair();
    let sealed = phone.seal(b"truncated").unwrap();
    assert_eq!(
        host.open(&sealed[0][..sealed[0].len() - 1]).unwrap_err(),
        Error::Decrypt
    );
    assert_eq!(host.seal(b"reply").unwrap_err(), Error::Poisoned);
}

// Item 6: chunking of messages of 0 bytes, 65,518 bytes, 65,519 bytes, and 1 MiB.

#[test]
fn item_6_chunking() {
    let (mut phone, mut host) = session_pair();
    for (len, count) in [(0, 1), (65_518, 1), (65_519, 2), (1 << 20, 17)] {
        let message: Vec<u8> = (0..len).map(|index| (index % 253) as u8).collect();
        let sealed = phone.seal(&message).unwrap();
        assert_eq!(sealed.len(), count, "{len} bytes");
        assert!(sealed
            .iter()
            .all(|noise_message| noise_message.len() <= 65_535));
        assert_eq!(send(&mut host, &mut phone, b"ack"), b"ack");
        let mut received = None;
        for noise_message in &sealed {
            assert!(received.is_none());
            received = host
                .open(&through_relay(ConnId::MIN, noise_message))
                .unwrap();
        }
        assert_eq!(received.unwrap(), message, "{len} bytes");
    }
    assert_eq!(MAX_CHUNK_LEN, 65_518);
}
