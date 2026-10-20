//! Conformance items 3, 4, 5, 8, and 9 of section 13: the host engine and the client engine
//! through the mock relay, over real WebSockets on 127.0.0.1.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use halv_remote::client::{
    self, ClientConfig, ClientEvent, ClientHandle, ClientSession, ClientStatus, DeviceInfo,
    PairError, PairedHost, TerminalSubscription, TerminalUpdate,
};
use halv_remote::host::{
    encode_paste, DeviceStore, HostConfig, HostEngine, HostHandle, HostStatus, NewDevice,
    OfflineReason, PairingOfferInfo, StoreError, StoredDevice,
};
use halv_remote::net::ConnectError;
use halv_remote::relay_auth::bearer_header_value;
use halv_remote::relay_wire::device_socket_url;
use halv_remote::relay_wire::UpgradeError;
use halv_remote::rpc::Platform;
use halv_remote::testing::{
    Approval, Direction, Fault, MemoryBackend, MemoryDeviceStore, MockRelay, MockRelayConfig,
    TestApprover,
};
use halv_remote::{
    DeviceKeys, HandshakePayload, HostIdentity, PairingInitiator, PairingOffer, PairingSecret,
};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message;

const WAIT: Duration = Duration::from_secs(10);

struct Harness {
    relay: MockRelay,
    identity: Arc<HostIdentity>,
    backend: Arc<MemoryBackend>,
    store: Arc<MemoryDeviceStore>,
    approver: Arc<TestApprover>,
    host: HostHandle,
}

fn host_config(relay: &MockRelay, identity: Arc<HostIdentity>) -> HostConfig {
    let mut config = HostConfig::new(relay.url(), identity);
    config.backoff_min = Duration::from_millis(20);
    config.backoff_max = Duration::from_millis(200);
    config.approval_timeout = Duration::from_secs(5);
    config.pairing_close_delay = Duration::from_millis(500);
    config
}

fn client_config() -> ClientConfig {
    let mut config = ClientConfig::new("halv-mobile/0.1.0", Platform::Ios);
    config.backoff_min = Duration::from_millis(20);
    config.backoff_max = Duration::from_millis(200);
    config.host_offline_retry_min = Duration::from_millis(50);
    config.host_offline_retry_max = Duration::from_millis(100);
    config.not_authorized_pause = Duration::from_millis(300);
    config
}

fn device_info(name: &str) -> DeviceInfo {
    DeviceInfo {
        name: name.to_owned(),
        platform: Platform::Ios,
        model: Some("iPhone".to_owned()),
        app: "halv-mobile/0.1.0".to_owned(),
    }
}

impl Harness {
    async fn start() -> Harness {
        Harness::start_with(|_| {}).await
    }

    async fn start_with(configure: impl FnOnce(&mut HostConfig)) -> Harness {
        Harness::build(MockRelayConfig::default(), None, configure).await
    }

    /// Starts a harness with relay settings, and with a gated store in front of the registry.
    async fn build(
        relay_config: MockRelayConfig,
        gated: Option<Arc<GatedStore>>,
        configure: impl FnOnce(&mut HostConfig),
    ) -> Harness {
        let relay = MockRelay::start_with(relay_config).await.unwrap();
        let identity = Arc::new(HostIdentity::generate().unwrap());
        let backend = MemoryBackend::new("Studio Mac");
        backend.add_pane("term-1", 80, 24);
        let store = gated
            .as_ref()
            .map_or_else(MemoryDeviceStore::new, |gated| gated.inner.clone());
        let engine_store: Arc<dyn DeviceStore> = match gated {
            Some(gated) => gated,
            None => store.clone(),
        };
        let approver = TestApprover::new(Approval::Allow);
        let mut config = host_config(&relay, identity.clone());
        configure(&mut config);
        let host =
            HostEngine::start(config, backend.clone(), engine_store, approver.clone()).unwrap();
        wait_host_online(&host).await;
        Harness {
            relay,
            identity,
            backend,
            store,
            approver,
            host,
        }
    }

    async fn offer(&self) -> PairingOfferInfo {
        self.host.start_pairing().await.unwrap()
    }

    async fn pair(&self, name: &str) -> PairedHost {
        let offer = self.offer().await;
        client::pair(&offer.uri, device_info(name), &client_config(), |_| {})
            .await
            .unwrap()
    }

    async fn connect(&self, paired: PairedHost) -> (ClientHandle, mpsc::Receiver<ClientEvent>) {
        let (handle, events) = ClientSession::connect(paired, client_config());
        wait_connected(&handle).await;
        (handle, events)
    }
}

async fn wait_host_online(host: &HostHandle) {
    let mut status = host.subscribe_status();
    tokio::time::timeout(
        WAIT,
        status.wait_for(|status| *status == HostStatus::Online),
    )
    .await
    .expect("the host comes online")
    .unwrap();
}

async fn wait_status(handle: &ClientHandle, check: impl Fn(&ClientStatus) -> bool) -> ClientStatus {
    let mut status = handle.subscribe_status();
    let found = tokio::time::timeout(WAIT, status.wait_for(|status| check(status)))
        .await
        .unwrap_or_else(|_| panic!("status not reached, last: {:?}", handle.status()))
        .unwrap()
        .clone();
    found
}

async fn wait_connected(handle: &ClientHandle) {
    wait_status(handle, |status| {
        matches!(status, ClientStatus::Connected(_))
    })
    .await;
}

/// Waits until `check` returns a value.
async fn eventually<T>(mut check: impl FnMut() -> Option<T>) -> T {
    let deadline = tokio::time::Instant::now() + WAIT;
    loop {
        if let Some(value) = check() {
            return value;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "condition not reached"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

async fn next_update(subscription: &mut TerminalSubscription) -> TerminalUpdate {
    tokio::time::timeout(WAIT, subscription.recv())
        .await
        .expect("a terminal update")
        .expect("the subscription is open")
}

/// Reads updates until the data `expected` arrived. Returns the updates.
async fn read_until_data(
    subscription: &mut TerminalSubscription,
    expected: &[u8],
) -> Vec<TerminalUpdate> {
    let mut updates = Vec::new();
    let mut data = Vec::new();
    while !data.ends_with(expected) {
        let update = next_update(subscription).await;
        match &update {
            TerminalUpdate::Data { data: chunk, .. } => data.extend_from_slice(chunk),
            TerminalUpdate::Snapshot { data: snapshot, .. } => {
                data.clear();
                data.extend_from_slice(snapshot);
            }
            _ => {}
        }
        updates.push(update);
    }
    updates
}

/// Opens a raw device socket with a bearer token.
async fn raw_device_socket(
    relay: &MockRelay,
    offer: &PairingOffer,
    token: &[u8; 32],
) -> tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>> {
    let url = device_socket_url(&relay.url(), offer.host_id());
    let mut request = url.into_client_request().unwrap();
    request.headers_mut().insert(
        "Authorization",
        bearer_header_value(token).as_str().parse().unwrap(),
    );
    tokio_tungstenite::connect_async(request).await.unwrap().0
}

/// Sends message 1 of a pairing with the PSK of `psk_offer` and returns the close code.
async fn pair_with_wrong_psk(relay: &MockRelay, offer: &PairingOffer) -> Option<u16> {
    let wrong = PairingOffer::new(
        offer.relay_url(),
        offer.host_id().clone(),
        *offer.host_noise_public(),
        PairingSecret::generate().unwrap(),
        offer.expires_at_ms(),
        None,
    )
    .unwrap();
    let mut socket = raw_device_socket(relay, offer, offer.invite_token().as_bytes()).await;
    let keys = DeviceKeys::generate().unwrap();
    let (_, message_1) =
        PairingInitiator::start(&wrong, &keys, HandshakePayload::new("halv-mobile/0.1.0")).unwrap();
    socket
        .send(Message::Binary(message_1.into()))
        .await
        .unwrap();
    loop {
        match tokio::time::timeout(WAIT, socket.next()).await.unwrap() {
            Some(Ok(Message::Close(frame))) => return frame.map(|frame| frame.code.into()),
            Some(Ok(Message::Binary(_))) => panic!("a wrong PSK must not get message 2"),
            Some(Ok(_)) => continue,
            Some(Err(_)) | None => return None,
        }
    }
}

// Item 3 and item 9: pairing, then a session with panes, streaming, and input.

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pair_connect_list_stream_type_and_revoke() {
    let harness = Harness::start().await;
    let host_id = harness.identity.host_id();
    let offer = harness.offer().await;
    assert_eq!(harness.relay.invites(&host_id).len(), 1);

    let pending = client::start_pairing(&offer.uri, device_info("Ana's phone"), &client_config())
        .await
        .unwrap();
    let phone_code = pending.confirmation_code();
    assert_eq!(pending.host_name(), Some("Studio Mac"));
    let paired = pending.finish().await.unwrap();

    // Both sides show the same code, and the dialog shows the device facts.
    let requests = harness.approver.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].code, phone_code);
    assert_eq!(requests[0].name, "Ana's phone");
    assert_eq!(requests[0].platform, Platform::Ios);
    assert_eq!(paired.host_name(), "Studio Mac");
    assert_eq!(paired.host_id(), &host_id);

    // The host stored the device, registered its token hash, and deleted the invite.
    let stored = harness.store.devices();
    assert_eq!(stored.len(), 1);
    assert_eq!(&stored[0].id, paired.device_id());
    assert_eq!(stored[0].noise_public, paired.keys().noise_public());
    assert_eq!(
        harness.relay.devices(&host_id),
        vec![(paired.device_id().clone(), paired.keys().token_hash())]
    );
    assert!(harness.relay.invites(&host_id).is_empty());
    // The material survives a JSON round trip.
    let paired = PairedHost::from_json(&paired.to_json()).unwrap();

    let (handle, _events) = harness.connect(paired.clone()).await;
    let hello = handle.hello().unwrap();
    assert_eq!(&hello.device, paired.device_id());
    assert_eq!(hello.host.name, "Studio Mac");
    assert!(hello.caps.iter().any(|cap| cap == "terminal.v1"));
    assert_eq!(harness.host.connected_devices().len(), 1);

    let panes = handle.panes_list().await.unwrap();
    assert_eq!(panes.panes.len(), 1);
    assert_eq!(panes.panes[0].session, "term-1");

    let mut terminal = handle.subscribe_terminal("term-1").await.unwrap();
    assert_eq!(terminal.size(), (80, 24));
    assert!(matches!(
        next_update(&mut terminal).await,
        TerminalUpdate::Snapshot { .. }
    ));
    harness.backend.push_output("term-1", b"hello ");
    harness.backend.push_output("term-1", b"world");
    read_until_data(&mut terminal, b"hello world").await;

    handle.input("term-1", b"ls\r").await.unwrap();
    // The output has no bracketed paste switch, so the mode is unknown.
    // Text with a line break then uses bracketed paste (section 8.8).
    handle
        .paste("term-1", "echo a\necho b", true)
        .await
        .unwrap();
    assert_eq!(
        harness.backend.writes(),
        vec![
            ("term-1".to_owned(), b"ls\r".to_vec()),
            (
                "term-1".to_owned(),
                b"\x1b[200~echo a\necho b\x1b[201~\r".to_vec()
            ),
        ]
    );
    let missing = handle.input("term-9", b"x").await.unwrap_err();
    assert!(
        matches!(missing, client::ClientError::Rpc(error) if error.code == halv_remote::ErrorCode::NotFound)
    );
    assert_eq!(
        handle.input("term-1", &[0u8; 4097]).await.unwrap_err(),
        client::ClientError::TooLarge
    );

    // A revoke on the desktop reaches the phone as the encrypted event.
    harness
        .host
        .revoke_device(paired.device_id())
        .await
        .unwrap();
    wait_status(&handle, |status| *status == ClientStatus::Revoked).await;
    assert!(harness.store.devices()[0].revoked_at_ms.is_some());
    assert!(harness.relay.devices(&host_id).is_empty());
    // The subscription ends with the session.
    assert!(tokio::time::timeout(WAIT, terminal.recv())
        .await
        .unwrap()
        .is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pairing_deny_ends_the_offer() {
    let harness = Harness::start().await;
    harness.approver.set_answer(Approval::Deny);
    let offer = harness.offer().await;
    let error = client::pair(&offer.uri, device_info("Phone"), &client_config(), |_| {})
        .await
        .unwrap_err();
    assert_eq!(error, PairError::Denied);
    assert!(harness.store.devices().is_empty());
    let host_id = harness.identity.host_id();
    eventually(|| harness.relay.invites(&host_id).is_empty().then_some(())).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pairing_approval_times_out() {
    let harness = Harness::start_with(|config| {
        config.approval_timeout = Duration::from_millis(300);
    })
    .await;
    harness.approver.set_answer(Approval::Never);
    let offer = harness.offer().await;
    let error = client::pair(&offer.uri, device_info("Phone"), &client_config(), |_| {})
        .await
        .unwrap_err();
    assert_eq!(error, PairError::Timeout);
    assert!(harness.store.devices().is_empty());
}

// Item 4: a wrong PSK, an unknown device key, and a revoked device key each fail.

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wrong_psk_is_closed_with_4401() {
    let harness = Harness::start().await;
    let info = harness.offer().await;
    let offer = PairingOffer::parse_uri(&info.uri, halv_remote::unix_time_ms()).unwrap();
    assert_eq!(
        pair_with_wrong_psk(&harness.relay, &offer).await,
        Some(4401)
    );
    assert!(harness.approver.requests().is_empty());
    // The offer stays usable for the real device.
    let paired = client::pair(&info.uri, device_info("Phone"), &client_config(), |_| {}).await;
    assert!(paired.is_ok());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn invite_expiry_and_tries() {
    let harness = Harness::start().await;
    let host_id = harness.identity.host_id();

    // Expiry on the relay clock.
    let info = harness.offer().await;
    harness.relay.advance_clock(Duration::from_secs(6 * 60));
    let error = client::pair(&info.uri, device_info("Phone"), &client_config(), |_| {})
        .await
        .unwrap_err();
    assert_eq!(error, PairError::Expired);
    harness.relay.rewind_clock(Duration::from_secs(6 * 60));

    // Five tries: each failed handshake uses one, and the sixth admission fails.
    let info = harness.offer().await;
    let offer = PairingOffer::parse_uri(&info.uri, halv_remote::unix_time_ms()).unwrap();
    for _ in 0..5 {
        assert_eq!(
            pair_with_wrong_psk(&harness.relay, &offer).await,
            Some(4401)
        );
    }
    assert_eq!(harness.relay.invites(&host_id)[0].tries, 0);
    let error = client::pair(&info.uri, device_info("Phone"), &client_config(), |_| {})
        .await
        .unwrap_err();
    assert_eq!(error, PairError::NotAuthorized);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn revoked_device_fails_at_message_1_even_when_the_relay_admits_it() {
    let harness = Harness::start().await;
    let paired = harness.pair("Phone").await;
    // Revoke in the registry only. The relay still has the token hash.
    harness
        .store
        .revoke(paired.device_id(), halv_remote::unix_time_ms())
        .unwrap();
    assert_eq!(harness.relay.devices(&harness.identity.host_id()).len(), 1);
    let (handle, _events) = ClientSession::connect(paired, client_config());
    wait_status(&handle, |status| {
        matches!(status, ClientStatus::NotAuthorized { .. })
    })
    .await;
    handle.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unknown_device_key_is_closed_with_4401() {
    let harness = Harness::start().await;
    let paired = harness.pair("Phone").await;
    // The relay token of the paired device, with a Noise key that the host does not know.
    let stranger = DeviceKeys::from_parts(
        *DeviceKeys::generate().unwrap().noise_secret(),
        paired.keys().relay_token().clone(),
    );
    let forged = PairedHost::new(
        paired.relay_url(),
        paired.host_id().clone(),
        *paired.host_noise_public(),
        paired.host_name(),
        paired.device_id().clone(),
        stranger,
    );
    let (handle, _events) = ClientSession::connect(forged, client_config());
    wait_status(&handle, |status| {
        matches!(status, ClientStatus::NotAuthorized { .. })
    })
    .await;
    handle.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn device_id_must_match_the_noise_key() {
    let harness = Harness::start().await;
    let first = harness.pair("First").await;
    let second = harness.pair("Second").await;
    // The relay token of the first device admits the socket as the first device,
    // but the Noise key belongs to the second device.
    let mixed = PairedHost::new(
        first.relay_url(),
        first.host_id().clone(),
        *first.host_noise_public(),
        first.host_name(),
        first.device_id().clone(),
        DeviceKeys::from_parts(
            *second.keys().noise_secret(),
            first.keys().relay_token().clone(),
        ),
    );
    let (handle, _events) = ClientSession::connect(mixed, client_config());
    wait_status(&handle, |status| {
        matches!(status, ClientStatus::NotAuthorized { .. })
    })
    .await;
    handle.close().await;
    // Both real devices still connect.
    let (handle, _events) = harness.connect(second).await;
    handle.close().await;
}

// Item 5: a changed, replayed, or reordered frame ends the connection. The client reconnects.

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn broken_frames_end_the_connection_and_the_client_reconnects() {
    let harness = Harness::start().await;
    let paired = harness.pair("Phone").await;
    let (handle, mut events) = harness.connect(paired).await;
    let cases = [
        (Direction::ToDevice, Fault::Corrupt),
        (Direction::ToDevice, Fault::Replay),
        (Direction::ToDevice, Fault::Reorder),
        (Direction::ToHost, Fault::Corrupt),
        (Direction::ToHost, Fault::Replay),
        (Direction::ToHost, Fault::Reorder),
    ];
    for (direction, fault) in cases {
        while events.try_recv().is_ok() {}
        harness.relay.inject_fault(direction, fault);
        // Two requests at once make two frames in a row in each direction,
        // so a held frame goes out right after the next one.
        let (first, second) = tokio::join!(handle.panes_list(), handle.panes_list());
        assert!(
            first.is_err() || second.is_err(),
            "{direction:?} {fault:?}: a broken frame must end the connection"
        );
        // The engine reports the loss and connects again with a new handshake.
        let mut disconnected = false;
        loop {
            let event = tokio::time::timeout(WAIT, events.recv())
                .await
                .unwrap()
                .unwrap();
            match event {
                ClientEvent::Status(ClientStatus::Disconnected { .. }) => disconnected = true,
                ClientEvent::Status(ClientStatus::Connected(_)) if disconnected => break,
                _ => {}
            }
        }
        assert!(handle.panes_list().await.is_ok(), "{direction:?} {fault:?}");
    }
    handle.close().await;
}

// Item 8: terminal streaming.

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn host_restart_then_resume_with_from_seq() {
    let harness = Harness::start().await;
    let host_id = harness.identity.host_id();
    let paired = harness.pair("Phone").await;
    let (handle, _events) = harness.connect(paired).await;
    let mut terminal = handle.subscribe_terminal("term-1").await.unwrap();
    assert!(matches!(
        next_update(&mut terminal).await,
        TerminalUpdate::Snapshot { .. }
    ));
    harness.backend.push_output("term-1", b"one ");
    let seq = harness.backend.push_output("term-1", b"two ");
    read_until_data(&mut terminal, b"two ").await;

    // A relay restart drops the host socket. The devices get 4408.
    harness.relay.disconnect_host(&host_id, 1012);
    wait_status(&handle, |status| {
        matches!(status, ClientStatus::Disconnected { .. })
    })
    .await;
    harness.backend.push_output("term-1", b"three");
    wait_host_online(&harness.host).await;
    wait_connected(&handle).await;

    // The client resumes with fromSeq, and the ring still holds the missed chunk: a replay.
    let updates = read_until_data(&mut terminal, b"three").await;
    assert!(updates
        .iter()
        .all(|update| matches!(update, TerminalUpdate::Data { .. })));
    assert!(harness
        .backend
        .replay_requests()
        .contains(&("term-1".to_owned(), seq)));
    handle.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn resume_after_the_ring_moved_on_gets_a_snapshot() {
    let harness = Harness::start().await;
    harness.backend.set_ring_bytes(64);
    let paired = harness.pair("Phone").await;
    let (handle, _events) = harness.connect(paired).await;
    let mut terminal = handle.subscribe_terminal("term-1").await.unwrap();
    assert!(matches!(
        next_update(&mut terminal).await,
        TerminalUpdate::Snapshot { .. }
    ));
    harness.backend.push_output("term-1", b"first");
    read_until_data(&mut terminal, b"first").await;

    harness
        .relay
        .close_device_sockets(&harness.identity.host_id(), 4408);
    wait_status(&handle, |status| {
        matches!(status, ClientStatus::Disconnected { .. })
    })
    .await;
    // More output than the ring holds: the chunks after fromSeq are gone.
    for _ in 0..10 {
        harness.backend.push_output("term-1", &[b'x'; 32]);
    }
    let last = harness.backend.push_output("term-1", b"tail");
    wait_connected(&handle).await;
    let update = next_update(&mut terminal).await;
    match update {
        TerminalUpdate::Snapshot { seq, data, .. } => {
            assert_eq!(seq, last);
            assert!(data.ends_with(b"tail"));
            assert!(data.len() <= 64);
        }
        other => panic!("expected a snapshot, got {other:?}"),
    }
    handle.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn flow_control_pauses_then_sends_a_new_snapshot_after_acks() {
    let harness = Harness::start().await;
    let paired = harness.pair("Phone").await;
    let (handle, _events) = harness.connect(paired).await;
    let mut terminal = handle.subscribe_terminal("term-1").await.unwrap();
    // The phone does not read, so it sends no acks.
    let chunk = vec![b'z'; 4096];
    let mut last = 0;
    for _ in 0..256 {
        last = harness.backend.push_output("term-1", &chunk);
    }
    tokio::time::sleep(Duration::from_millis(500)).await;
    // Only the window arrived: about 512 KiB of the 1 MiB.
    let mut received = 0;
    let mut snapshots = 0;
    let mut seen_last = false;
    while let Some(update) = terminal.try_recv() {
        match update {
            TerminalUpdate::Data { data, seq } => {
                received += data.len();
                seen_last |= seq == last;
            }
            TerminalUpdate::Snapshot { .. } => snapshots += 1,
            _ => {}
        }
    }
    assert_eq!(snapshots, 1, "only the first snapshot before the pause");
    assert!(received >= 512 * 1024, "received {received}");
    assert!(received <= 600 * 1024, "received {received}");
    assert!(!seen_last);
    // The reads above were acknowledged. The host resumes with a new snapshot of the ring.
    match next_update(&mut terminal).await {
        TerminalUpdate::Snapshot { seq, data, .. } => {
            assert_eq!(seq, last);
            assert_eq!(data.len(), 256 * 1024);
        }
        other => panic!("expected a snapshot, got {other:?}"),
    }
    // Live output continues after the snapshot.
    harness.backend.push_output("term-1", b"live");
    read_until_data(&mut terminal, b"live").await;
    handle.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn many_tiny_writes_give_few_events() {
    let harness = Harness::start().await;
    let paired = harness.pair("Phone").await;
    let (handle, _events) = harness.connect(paired).await;
    let mut terminal = handle.subscribe_terminal("term-1").await.unwrap();
    assert!(matches!(
        next_update(&mut terminal).await,
        TerminalUpdate::Snapshot { .. }
    ));
    for _ in 0..500 {
        harness.backend.push_output("term-1", b".");
    }
    let mut events = 0;
    let mut bytes = 0;
    while bytes < 500 {
        match next_update(&mut terminal).await {
            TerminalUpdate::Data { data, .. } => {
                events += 1;
                bytes += data.len();
            }
            other => panic!("unexpected update {other:?}"),
        }
    }
    assert_eq!(bytes, 500);
    assert!(events <= 10, "{events} events for 500 writes");
    handle.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bracketed_paste_follows_the_output_across_chunks() {
    let harness = Harness::start().await;
    let paired = harness.pair("Phone").await;
    let (handle, _events) = harness.connect(paired).await;
    let mut terminal = handle.subscribe_terminal("term-1").await.unwrap();
    assert!(matches!(
        next_update(&mut terminal).await,
        TerminalUpdate::Snapshot { .. }
    ));

    // No switch yet: the mode is unknown. Text with a line break uses bracketed paste,
    // and text without a line break does not need it.
    handle.paste("term-1", "a\nb", false).await.unwrap();
    handle.paste("term-1", "one line", false).await.unwrap();
    // The switch arrives split over two chunks, then leaves the small ring:
    // only the scanner of the live output still knows it.
    harness.backend.set_ring_bytes(16);
    harness.backend.push_output("term-1", b"\x1b[?20");
    harness.backend.push_output("term-1", b"04h prompt");
    harness.backend.push_output("term-1", &[b'.'; 64]);
    read_until_data(&mut terminal, &[b'.'; 64]).await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    handle
        .paste("term-1", "x\x1b[201~y\nz", true)
        .await
        .unwrap();
    // Off again.
    harness.backend.push_output("term-1", b"\x1b[?2004l");
    read_until_data(&mut terminal, b"\x1b[?2004l").await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    handle.paste("term-1", "c\r\nd", false).await.unwrap();

    let writes: Vec<Vec<u8>> = harness
        .backend
        .writes()
        .into_iter()
        .map(|(_, bytes)| bytes)
        .collect();
    assert_eq!(
        writes,
        vec![
            b"\x1b[200~a\nb\x1b[201~".to_vec(),
            b"one line".to_vec(),
            b"\x1b[200~xy\nz\x1b[201~\r".to_vec(),
            b"c\rd".to_vec(),
        ]
    );
    assert_eq!(writes[2], encode_paste("x\x1b[201~y\nz", true, true));
    handle.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn terminal_events_and_pane_changes() {
    let harness = Harness::start().await;
    let paired = harness.pair("Phone").await;
    let (handle, mut events) = harness.connect(paired).await;
    let list = handle.subscribe_panes().await.unwrap();
    assert_eq!(list.panes.len(), 1);
    harness.backend.add_pane("term-2", 100, 30);
    let changed = loop {
        match tokio::time::timeout(WAIT, events.recv())
            .await
            .unwrap()
            .unwrap()
        {
            ClientEvent::PanesChanged(changed) => break changed,
            _ => continue,
        }
    };
    assert_eq!(changed.panes.len(), 2);
    assert!(changed.rev > list.rev);

    let mut terminal = handle.subscribe_terminal("term-2").await.unwrap();
    assert!(matches!(
        next_update(&mut terminal).await,
        TerminalUpdate::Snapshot {
            cols: 100,
            rows: 30,
            ..
        }
    ));
    harness.backend.resize("term-2", 120, 40);
    assert_eq!(
        next_update(&mut terminal).await,
        TerminalUpdate::Resized {
            cols: 120,
            rows: 40
        }
    );
    harness.backend.exit("term-2", Some(3));
    assert_eq!(
        next_update(&mut terminal).await,
        TerminalUpdate::Exit {
            code: Some(3),
            signal: None
        }
    );
    harness.backend.remove_pane("term-2");
    assert!(matches!(
        next_update(&mut terminal).await,
        TerminalUpdate::Closed { .. }
    ));
    assert!(tokio::time::timeout(WAIT, terminal.recv())
        .await
        .unwrap()
        .is_none());

    // A missing pane, and the limit of 8 subscriptions.
    assert!(handle.subscribe_terminal("term-9").await.is_err());
    let mut subscriptions = Vec::new();
    for _ in 0..8 {
        subscriptions.push(handle.subscribe_terminal("term-1").await.unwrap());
    }
    let error = handle.subscribe_terminal("term-1").await.unwrap_err();
    assert!(
        matches!(error, client::ClientError::Rpc(error) if error.code == halv_remote::ErrorCode::Busy)
    );
    // An ended subscription frees its place.
    drop(subscriptions.pop());
    let replacement = eventually_subscribe(&handle).await;
    drop(replacement);
    handle.close().await;
}

async fn eventually_subscribe(handle: &ClientHandle) -> TerminalSubscription {
    let deadline = tokio::time::Instant::now() + WAIT;
    loop {
        match handle.subscribe_terminal("term-1").await {
            Ok(subscription) => return subscription,
            Err(_) if tokio::time::Instant::now() < deadline => {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            Err(error) => panic!("no free subscription: {error}"),
        }
    }
}

// Section 7.5: windows that do not overlap.

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn version_mismatch_closes_with_4426() {
    // A host with a newer window refuses the pairing.
    let harness = Harness::start_with(|config| {
        config.min_version = 2;
        config.max_version = 2;
    })
    .await;
    let offer = harness.offer().await;
    let error = client::pair(&offer.uri, device_info("Phone"), &client_config(), |_| {})
        .await
        .unwrap_err();
    assert_eq!(error, PairError::Incompatible);

    // A device with a newer window stops its session with Incompatible.
    let harness = Harness::start().await;
    let paired = harness.pair("Phone").await;
    let mut config = client_config();
    config.min_version = 2;
    config.max_version = 2;
    let (handle, _events) = ClientSession::connect(paired, config);
    wait_status(&handle, |status| *status == ClientStatus::Incompatible).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn device_unpair_revokes_the_device() {
    let harness = Harness::start().await;
    let paired = harness.pair("Phone").await;
    let (handle, _events) = harness.connect(paired.clone()).await;
    handle.unpair().await.unwrap();
    wait_status(&handle, |status| *status == ClientStatus::Revoked).await;
    let stored = harness.store.get(paired.device_id()).unwrap();
    assert!(stored.is_revoked());
    let host_id = harness.identity.host_id();
    eventually(|| harness.relay.devices(&host_id).is_empty().then_some(())).await;
    // The relay no longer admits the token: the device stays out.
    let (handle, _events) = ClientSession::connect(paired, client_config());
    wait_status(&handle, |status| {
        matches!(status, ClientStatus::NotAuthorized { .. })
    })
    .await;
    handle.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn host_offline_and_cancelled_offer() {
    let harness = Harness::start().await;
    let paired = harness.pair("Phone").await;
    harness.host.shutdown().await;
    assert_eq!(harness.host.status(), HostStatus::Stopped);
    // No host socket: 503 host_offline.
    let (handle, _events) = ClientSession::connect(paired, client_config());
    wait_status(&handle, |status| {
        matches!(status, ClientStatus::HostOffline { .. })
    })
    .await;
    handle.close().await;
    assert_eq!(handle.status(), ClientStatus::Closed);
    assert!(harness.host.start_pairing().await.is_err());

    // A cancelled offer deletes its invite.
    let harness = Harness::start().await;
    let host_id = harness.identity.host_id();
    let offer = harness.offer().await;
    harness.host.cancel_pairing().await;
    assert!(harness.relay.invites(&host_id).is_empty());
    let error = client::pair(&offer.uri, device_info("Phone"), &client_config(), |_| {})
        .await
        .unwrap_err();
    assert_eq!(error, PairError::NotAuthorized);
}

// Host socket life cycle.

/// Waits for a host state.
async fn wait_host(host: &HostHandle, check: impl Fn(&HostStatus) -> bool) -> HostStatus {
    let mut status = host.subscribe_status();
    let found = tokio::time::timeout(WAIT, status.wait_for(|status| check(status)))
        .await
        .unwrap_or_else(|_| panic!("host status not reached, last: {:?}", host.status()))
        .unwrap()
        .clone();
    found
}

fn offline_after_4409(status: &HostStatus) -> bool {
    matches!(
        status,
        HostStatus::Offline {
            reason: OfflineReason::Closed(Some(4409)),
            ..
        }
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_second_host_socket_replaces_the_first() {
    let short_wait = |config: &mut HostConfig| {
        config.replaced_retry_delay = Duration::from_millis(300);
    };
    let harness = Harness::start_with(short_wait).await;
    let mut config = host_config(&harness.relay, harness.identity.clone());
    short_wait(&mut config);
    let second = HostEngine::start(
        config,
        harness.backend.clone(),
        harness.store.clone(),
        harness.approver.clone(),
    )
    .unwrap();
    wait_host_online(&second).await;
    // The first engine gets 4409. The relay is not trusted, so it waits and tries once more.
    let first = wait_host(&harness.host, offline_after_4409).await;
    let HostStatus::Offline { retry_in, .. } = first else {
        unreachable!()
    };
    assert_eq!(retry_in, Duration::from_millis(300));
    // Its retry replaces the second engine once. The retry of the second engine then
    // gives the first engine its second 4409, and it stops, so two desktops do not fight.
    wait_host(&harness.host, |status| *status == HostStatus::Replaced).await;
    wait_host_online(&second).await;
    tokio::time::sleep(Duration::from_millis(700)).await;
    assert_eq!(second.status(), HostStatus::Online);
    assert_eq!(harness.host.status(), HostStatus::Replaced);
    second.shutdown().await;
}

// Section 6.6: close codes come from the untrusted relay.

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_false_4409_waits_then_the_second_one_stops_the_host() {
    let harness = Harness::start_with(|config| {
        config.replaced_retry_delay = Duration::from_millis(300);
    })
    .await;
    let host_id = harness.identity.host_id();
    // The relay sends 4409 with no other desktop. The host waits, then connects again.
    harness.relay.disconnect_host(&host_id, 4409);
    let status = wait_host(&harness.host, offline_after_4409).await;
    assert!(
        matches!(status, HostStatus::Offline { retry_in, .. } if retry_in == Duration::from_millis(300))
    );
    wait_host_online(&harness.host).await;
    // A second 4409 stops the host until the user turns the gateway on again.
    harness.relay.disconnect_host(&host_id, 4409);
    wait_host(&harness.host, |status| *status == HostStatus::Replaced).await;
    tokio::time::sleep(Duration::from_millis(600)).await;
    assert_eq!(harness.host.status(), HostStatus::Replaced);
    assert!(!harness.relay.host_connected(&host_id));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_false_4426_does_not_stop_the_phone() {
    let harness = Harness::start().await;
    let paired = harness.pair("Phone").await;
    let (handle, mut events) = harness.connect(paired).await;
    // The own handshake showed that the versions overlap, so 4426 is like 4400.
    harness
        .relay
        .close_device_sockets(&harness.identity.host_id(), 4426);
    let mut disconnected = false;
    loop {
        let event = tokio::time::timeout(WAIT, events.recv())
            .await
            .unwrap()
            .unwrap();
        match event {
            ClientEvent::Status(ClientStatus::Disconnected { reason, .. }) => {
                assert!(reason.contains("4426"), "{reason}");
                disconnected = true;
            }
            ClientEvent::Status(ClientStatus::Incompatible) => {
                panic!("a relay close code alone must not stop the phone")
            }
            ClientEvent::Status(ClientStatus::Connected(_)) if disconnected => break,
            _ => {}
        }
    }
    handle.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn host_clock_skew_keeps_the_host_offline() {
    let relay = MockRelay::start().await.unwrap();
    relay.advance_clock(Duration::from_secs(10 * 60));
    let identity = Arc::new(HostIdentity::generate().unwrap());
    let host = HostEngine::start(
        host_config(&relay, identity),
        MemoryBackend::new("Studio Mac"),
        MemoryDeviceStore::new(),
        TestApprover::new(Approval::Allow),
    )
    .unwrap();
    let mut status = host.subscribe_status();
    tokio::time::timeout(
        WAIT,
        status.wait_for(|status| {
            matches!(
                status,
                HostStatus::Offline {
                    reason: OfflineReason::Connect(ConnectError::Upgrade {
                        error: UpgradeError::ClockSkew,
                        ..
                    }),
                    ..
                }
            )
        }),
    )
    .await
    .unwrap()
    .unwrap();
    relay.rewind_clock(Duration::from_secs(10 * 60));
    wait_host_online(&host).await;
    host.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sessions_end_at_the_lifetime_limit() {
    let harness = Harness::start_with(|config| {
        config.connection_lifetime = Duration::from_millis(400);
    })
    .await;
    let paired = harness.pair("Phone").await;
    let (handle, mut events) = harness.connect(paired).await;
    // The host closes the session with 4408, and the device connects again.
    let mut closed = false;
    loop {
        match tokio::time::timeout(WAIT, events.recv())
            .await
            .unwrap()
            .unwrap()
        {
            ClientEvent::Status(ClientStatus::Disconnected { reason, .. }) => {
                assert!(reason.contains("4408"), "{reason}");
                closed = true;
            }
            ClientEvent::Status(ClientStatus::Connected(_)) if closed => break,
            _ => {}
        }
    }
    handle.close().await;
}

// The host side of the session rules, with a raw Noise session.

struct RawSession {
    socket: tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    transport: halv_remote::Transport,
    next_id: u32,
}

impl RawSession {
    async fn open(relay: &MockRelay, paired: &PairedHost) -> RawSession {
        let url = device_socket_url(&relay.url(), paired.host_id());
        let mut request = url.into_client_request().unwrap();
        request.headers_mut().insert(
            "Authorization",
            bearer_header_value(paired.keys().relay_token().as_bytes())
                .as_str()
                .parse()
                .unwrap(),
        );
        let mut socket = tokio_tungstenite::connect_async(request).await.unwrap().0;
        let (initiator, message_1) = halv_remote::SessionInitiator::start(
            paired.host_id(),
            paired.host_noise_public(),
            paired.keys(),
            HandshakePayload::new("halv-mobile/0.1.0"),
        )
        .unwrap();
        socket
            .send(Message::Binary(message_1.into()))
            .await
            .unwrap();
        let message_2 = match tokio::time::timeout(WAIT, socket.next()).await.unwrap() {
            Some(Ok(Message::Binary(bytes))) => bytes,
            other => panic!("expected message 2, got {other:?}"),
        };
        let established = initiator.read_message_2(&message_2).unwrap();
        RawSession {
            socket,
            transport: established.transport,
            next_id: 0,
        }
    }

    async fn send(&mut self, bytes: &[u8]) {
        for noise_message in self.transport.seal(bytes).unwrap() {
            self.socket
                .send(Message::Binary(noise_message.into()))
                .await
                .unwrap();
        }
    }

    /// Sends a request and returns its response. Events before it are skipped.
    async fn call<P: serde::Serialize>(
        &mut self,
        method: &str,
        params: &P,
    ) -> Result<ciborium::Value, halv_remote::RpcError> {
        self.next_id += 1;
        let id = halv_remote::RequestId::new(self.next_id).unwrap();
        let bytes = halv_remote::rpc::encode_request(id, method, params).unwrap();
        self.send(&bytes).await;
        loop {
            let frame = match tokio::time::timeout(WAIT, self.socket.next())
                .await
                .unwrap()
            {
                Some(Ok(Message::Binary(bytes))) => bytes,
                Some(Ok(_)) => continue,
                other => panic!("the session ended: {other:?}"),
            };
            let Some(message) = self.transport.open(&frame).unwrap() else {
                continue;
            };
            if let halv_remote::Message::Response(response) =
                halv_remote::Message::decode(&message).unwrap()
            {
                assert_eq!(response.id, id);
                return response.outcome;
            }
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn host_session_rules() {
    use halv_remote::rpc::{method, Empty, HelloParams, TerminalInputParams, TerminalPasteParams};
    use halv_remote::ErrorCode;

    let harness = Harness::start().await;
    let paired = harness.pair("Phone").await;
    let mut session = RawSession::open(&harness.relay, &paired).await;

    // Hello comes first.
    let error = session
        .call(method::PANES_LIST, &Empty {})
        .await
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::BadRequest);
    let hello = HelloParams {
        app: "halv-mobile/0.1.0".to_owned(),
        platform: Platform::Android,
        caps: vec!["future.v9".to_owned()],
    };
    assert!(session.call(method::HELLO, &hello).await.is_ok());
    assert!(session.call(method::PANES_LIST, &Empty {}).await.is_ok());

    // An unknown method, and pair.request inside a session.
    let error = session.call("files.read", &Empty {}).await.unwrap_err();
    assert_eq!(error.code, ErrorCode::UnknownMethod);
    let error = session
        .call(method::PAIR_REQUEST, &Empty {})
        .await
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::UnknownMethod);

    // Input and paste limits on the host side.
    let input = TerminalInputParams {
        session: "term-1".to_owned(),
        data: vec![b'x'; 4097],
    };
    let error = session
        .call(method::TERMINAL_INPUT, &input)
        .await
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::TooLarge);
    let paste = TerminalPasteParams {
        session: "term-1".to_owned(),
        text: "y".repeat(64 * 1024 + 1),
        submit: false,
    };
    let error = session
        .call(method::TERMINAL_PASTE, &paste)
        .await
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::TooLarge);
    assert!(harness.backend.writes().is_empty());

    // An unknown subscription.
    let error = session
        .call(
            method::TERMINAL_UNSUBSCRIBE,
            &halv_remote::rpc::TerminalUnsubscribeParams { sub: 77 },
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::NotFound);

    // Garbage instead of a Noise message ends the connection with 4400.
    session
        .socket
        .send(Message::Binary(vec![0u8; 40].into()))
        .await
        .unwrap();
    let code = loop {
        match tokio::time::timeout(WAIT, session.socket.next())
            .await
            .unwrap()
        {
            Some(Ok(Message::Close(frame))) => break frame.map(|frame| u16::from(frame.code)),
            Some(Ok(_)) => continue,
            _ => break None,
        }
    };
    assert_eq!(code, Some(4400));
}

// The fixes of the review: a gated store, raw sockets, and one test for each fix.

/// Holds one blocking store call until the test opens it.
#[derive(Default)]
struct Gate {
    armed: AtomicBool,
    entered: AtomicBool,
    open: Mutex<bool>,
    opened: Condvar,
}

impl Gate {
    /// Makes the next call wait.
    fn arm(&self) {
        *self.open.lock().unwrap() = false;
        self.entered.store(false, Ordering::SeqCst);
        self.armed.store(true, Ordering::SeqCst);
    }

    /// Lets the waiting call go on.
    fn release(&self) {
        self.armed.store(false, Ordering::SeqCst);
        *self.open.lock().unwrap() = true;
        self.opened.notify_all();
    }

    fn entered(&self) -> bool {
        self.entered.load(Ordering::SeqCst)
    }

    /// Runs in the store call: waits while the gate is armed.
    fn pass(&self) {
        if !self.armed.load(Ordering::SeqCst) {
            return;
        }
        self.entered.store(true, Ordering::SeqCst);
        let mut open = self.open.lock().unwrap();
        while !*open {
            open = self.opened.wait(open).unwrap();
        }
    }
}

/// A registry that can hold `insert` and `revoke`, so a test can act in the middle of them.
#[derive(Default)]
struct GatedStore {
    inner: Arc<MemoryDeviceStore>,
    insert: Gate,
    revoke: Gate,
}

impl DeviceStore for GatedStore {
    fn list(&self) -> Result<Vec<StoredDevice>, StoreError> {
        self.inner.list()
    }

    fn find_by_noise_key(&self, key: &[u8; 32]) -> Result<Option<StoredDevice>, StoreError> {
        self.inner.find_by_noise_key(key)
    }

    fn insert(&self, device: NewDevice) -> Result<StoredDevice, StoreError> {
        self.insert.pass();
        self.inner.insert(device)
    }

    fn revoke(&self, id: &halv_remote::DeviceId, at_ms: u64) -> Result<bool, StoreError> {
        self.revoke.pass();
        self.inner.revoke(id, at_ms)
    }

    fn touch_last_seen(&self, id: &halv_remote::DeviceId, at_ms: u64) -> Result<(), StoreError> {
        self.inner.touch_last_seen(id, at_ms)
    }
}

type RawSocket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// Opens a device socket with the relay token of a paired device. It sends nothing.
async fn paired_socket(relay: &MockRelay, paired: &PairedHost) -> RawSocket {
    let url = device_socket_url(&relay.url(), paired.host_id());
    let mut request = url.into_client_request().unwrap();
    request.headers_mut().insert(
        "Authorization",
        bearer_header_value(paired.keys().relay_token().as_bytes())
            .as_str()
            .parse()
            .unwrap(),
    );
    tokio_tungstenite::connect_async(request).await.unwrap().0
}

/// Reads a socket until it closes and returns the close code.
async fn read_close(socket: &mut RawSocket) -> Option<u16> {
    loop {
        match tokio::time::timeout(WAIT, socket.next()).await.unwrap() {
            Some(Ok(Message::Close(frame))) => return frame.map(|frame| u16::from(frame.code)),
            Some(Ok(_)) => continue,
            Some(Err(_)) | None => return None,
        }
    }
}

impl RawSession {
    async fn hello(&mut self) -> Result<ciborium::Value, halv_remote::RpcError> {
        let hello = halv_remote::rpc::HelloParams {
            app: "halv-mobile/0.1.0".to_owned(),
            platform: Platform::Ios,
            caps: Vec::new(),
        };
        self.call(halv_remote::rpc::method::HELLO, &hello).await
    }

    /// Sends a request and does not wait for the response.
    async fn send_request<P: serde::Serialize>(&mut self, method: &str, params: &P) {
        self.next_id += 1;
        let id = halv_remote::RequestId::new(self.next_id).unwrap();
        let bytes = halv_remote::rpc::encode_request(id, method, params).unwrap();
        self.send(&bytes).await;
    }

    /// Reads until the socket closes. Returns the names of the events and the close code.
    /// A response before the close is a test failure.
    async fn read_until_close(&mut self) -> (Vec<String>, Option<u16>) {
        let mut events = Vec::new();
        loop {
            let frame = match tokio::time::timeout(WAIT, self.socket.next())
                .await
                .unwrap()
            {
                Some(Ok(Message::Binary(bytes))) => bytes,
                Some(Ok(Message::Close(frame))) => {
                    return (events, frame.map(|frame| u16::from(frame.code)))
                }
                Some(Ok(_)) => continue,
                Some(Err(_)) | None => return (events, None),
            };
            let Some(message) = self.transport.open(&frame).unwrap() else {
                continue;
            };
            match halv_remote::Message::decode(&message).unwrap() {
                halv_remote::Message::Event(event) => events.push(event.name),
                other => panic!("expected only events before the close, got {other:?}"),
            }
        }
    }
}

// Finding 1: a replayed message 1 changes no state, and hello has a deadline.

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hello_marks_the_session_and_a_missing_hello_closes_with_4400() {
    let harness = Harness::start_with(|config| {
        config.hello_timeout = Duration::from_millis(500);
    })
    .await;
    let paired = harness.pair("Phone").await;

    // Message 1 and message 2 only, as a replay of message 1 gets: no state changes.
    let mut silent = RawSession::open(&harness.relay, &paired).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(harness.host.connected_devices().is_empty());
    assert_eq!(
        harness
            .store
            .get(paired.device_id())
            .unwrap()
            .last_seen_at_ms,
        None
    );
    // Without hello, the host closes the connection with 4400 after the deadline.
    assert_eq!(silent.read_until_close().await, (Vec::new(), Some(4400)));
    assert!(harness.host.connected_devices().is_empty());
    assert_eq!(
        harness
            .store
            .get(paired.device_id())
            .unwrap()
            .last_seen_at_ms,
        None
    );

    // hello, the first valid transport message, marks the session.
    let mut session = RawSession::open(&harness.relay, &paired).await;
    session.hello().await.unwrap();
    assert_eq!(harness.host.connected_devices().len(), 1);
    eventually(|| {
        harness
            .store
            .get(paired.device_id())
            .unwrap()
            .last_seen_at_ms
    })
    .await;
    // After hello, the deadline no longer applies.
    tokio::time::sleep(Duration::from_millis(700)).await;
    assert!(session
        .call(
            halv_remote::rpc::method::PANES_LIST,
            &halv_remote::rpc::Empty {}
        )
        .await
        .is_ok());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_host_keeps_at_most_4_open_handshakes() {
    let harness = Harness::start().await;
    let paired = harness.pair("Phone").await;
    // Four sockets that send no message 1 hold the four handshakes.
    let mut waiting = Vec::new();
    for _ in 0..4 {
        waiting.push(paired_socket(&harness.relay, &paired).await);
    }
    let mut fifth = paired_socket(&harness.relay, &paired).await;
    assert_eq!(read_close(&mut fifth).await, Some(4429));
    // When a handshake ends, a new connection gets its place.
    drop(waiting.pop());
    let deadline = tokio::time::Instant::now() + WAIT;
    loop {
        let (handle, _events) = ClientSession::connect(paired.clone(), client_config());
        let connected = tokio::time::timeout(Duration::from_secs(2), wait_connected(&handle)).await;
        handle.close().await;
        if connected.is_ok() {
            break;
        }
        assert!(tokio::time::Instant::now() < deadline, "no free handshake");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_host_keeps_at_most_8_connections() {
    // A relay that breaks its own limit of 8 device sockets.
    let relay_config = MockRelayConfig {
        max_device_sockets: 16,
        ..MockRelayConfig::default()
    };
    let harness = Harness::build(relay_config, None, |_| {}).await;
    let paired = harness.pair("Phone").await;
    let mut sessions = Vec::new();
    for _ in 0..8 {
        let mut session = RawSession::open(&harness.relay, &paired).await;
        session.hello().await.unwrap();
        sessions.push(session);
    }
    assert_eq!(harness.host.connected_devices().len(), 8);
    let mut ninth = paired_socket(&harness.relay, &paired).await;
    assert_eq!(read_close(&mut ninth).await, Some(4429));
    assert_eq!(harness.host.connected_devices().len(), 8);
}

// Finding 2: an unknown bracketed paste mode, and finding 8: writes of at most 32 KiB.

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_paste_near_64_kib_goes_out_in_writes_of_32_kib() {
    let harness = Harness::start().await;
    let paired = harness.pair("Phone").await;
    let (handle, _events) = harness.connect(paired).await;
    // The backend knows the mode, and its value wins over the unknown scanner state.
    harness.backend.set_bracketed_paste("term-1", true);
    let text = format!("a\n{}", "y".repeat(64 * 1024 - 2));
    assert_eq!(text.len(), 64 * 1024);
    handle.paste("term-1", &text, true).await.unwrap();
    let writes: Vec<Vec<u8>> = harness
        .backend
        .writes()
        .into_iter()
        .map(|(_, bytes)| bytes)
        .collect();
    assert_eq!(
        writes.iter().map(Vec::len).collect::<Vec<_>>(),
        vec![32 * 1024, 32 * 1024, 13]
    );
    assert_eq!(writes.concat(), encode_paste(&text, true, true));

    // A backend value of "off" also wins: no markers for text with a line break.
    harness.backend.set_bracketed_paste("term-1", false);
    handle.paste("term-1", "c\nd", false).await.unwrap();
    assert_eq!(harness.backend.writes().last().unwrap().1, b"c\rd".to_vec());
    handle.close().await;
}

// Finding 3: a resume applies the new size and gets the exit that it missed.

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn resume_applies_the_new_size_and_the_missed_exit() {
    let harness = Harness::start().await;
    let paired = harness.pair("Phone").await;
    let (handle, _events) = harness.connect(paired).await;
    let mut terminal = handle.subscribe_terminal("term-1").await.unwrap();
    assert!(matches!(
        next_update(&mut terminal).await,
        TerminalUpdate::Snapshot { .. }
    ));
    harness.backend.push_output("term-1", b"one ");
    read_until_data(&mut terminal, b"one ").await;

    harness
        .relay
        .close_device_sockets(&harness.identity.host_id(), 4408);
    wait_status(&handle, |status| {
        matches!(status, ClientStatus::Disconnected { .. })
    })
    .await;
    // While the phone is away: a resize, more output, and the exit of the process.
    harness.backend.resize("term-1", 100, 30);
    harness.backend.push_output("term-1", b"two");
    harness.backend.exit("term-1", Some(7));
    wait_connected(&handle).await;

    // The ring still holds the chunk, so the resume is a replay.
    assert_eq!(
        next_update(&mut terminal).await,
        TerminalUpdate::Resized {
            cols: 100,
            rows: 30
        }
    );
    let updates = read_until_data(&mut terminal, b"two").await;
    assert!(updates
        .iter()
        .all(|update| matches!(update, TerminalUpdate::Data { .. })));
    assert_eq!(
        next_update(&mut terminal).await,
        TerminalUpdate::Exit {
            code: Some(7),
            signal: None
        }
    );
    handle.close().await;
}

// Finding 4: revocation against work in flight.

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_revoke_after_the_message_1_check_ends_the_session_at_hello() {
    let harness = Harness::start().await;
    let paired = harness.pair("Phone").await;
    // The registry check of message 1 passes.
    let mut session = RawSession::open(&harness.relay, &paired).await;
    // Then the device is revoked, before the session is marked.
    harness
        .store
        .revoke(paired.device_id(), halv_remote::unix_time_ms())
        .unwrap();
    session
        .send_request(
            halv_remote::rpc::method::HELLO,
            &halv_remote::rpc::HelloParams {
                app: "halv-mobile/0.1.0".to_owned(),
                platform: Platform::Ios,
                caps: Vec::new(),
            },
        )
        .await;
    // hello gets no answer: the device gets `device.revoked` and 4401.
    assert_eq!(
        session.read_until_close().await,
        (vec!["device.revoked".to_owned()], Some(4401))
    );
    assert!(harness.host.connected_devices().is_empty());
    assert_eq!(
        harness
            .store
            .get(paired.device_id())
            .unwrap()
            .last_seen_at_ms,
        None
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn input_after_a_revoke_started_is_refused() {
    let gated = Arc::new(GatedStore::default());
    let harness = Harness::build(MockRelayConfig::default(), Some(gated.clone()), |_| {}).await;
    let paired = harness.pair("Phone").await;
    let (handle, _events) = harness.connect(paired.clone()).await;

    // The revoke stops in the registry call, before any session hears of it.
    gated.revoke.arm();
    let host = harness.host.clone();
    let device = paired.device_id().clone();
    let revoke = tokio::spawn(async move { host.revoke_device(&device).await });
    eventually(|| gated.revoke.entered().then_some(())).await;

    // Input in this window never reaches the PTY.
    let error = handle.input("term-1", b"rm -rf ~\r").await.unwrap_err();
    assert!(
        matches!(&error, client::ClientError::Rpc(error) if error.code == halv_remote::ErrorCode::Denied),
        "{error:?}"
    );
    let error = handle.paste("term-1", "rm -rf ~", true).await.unwrap_err();
    assert!(matches!(error, client::ClientError::Rpc(_)), "{error:?}");
    assert!(harness.backend.writes().is_empty());

    gated.revoke.release();
    revoke.await.unwrap().unwrap();
    wait_status(&handle, |status| *status == ClientStatus::Revoked).await;
    assert!(harness.backend.writes().is_empty());
}

// Finding 5: an approved pairing that does not finish leaves no active device.

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_aborted_pairing_revokes_the_inserted_device() {
    let gated = Arc::new(GatedStore::default());
    let harness = Harness::build(MockRelayConfig::default(), Some(gated.clone()), |_| {}).await;
    let host_id = harness.identity.host_id();
    let offer = harness.offer().await;

    // The user allows the device, and the insert waits in the registry call.
    gated.insert.arm();
    let uri = offer.uri.clone();
    let pairing = tokio::spawn(async move {
        client::pair(&uri, device_info("Phone"), &client_config(), |_| {}).await
    });
    eventually(|| gated.insert.entered().then_some(())).await;

    // The host socket drops, so the engine aborts the connection task.
    harness.relay.disconnect_host(&host_id, 1012);
    wait_host(&harness.host, |status| *status != HostStatus::Online).await;
    wait_host_online(&harness.host).await;
    assert!(pairing.await.unwrap().is_err());

    // Now the insert ends. Nobody answers the phone, so the device must not stay active.
    gated.insert.release();
    let stored = eventually(|| {
        harness
            .store
            .devices()
            .first()
            .filter(|device| device.is_revoked())
            .cloned()
    })
    .await;
    assert_eq!(harness.store.devices().len(), 1);
    eventually(|| {
        (!harness
            .relay
            .devices(&host_id)
            .iter()
            .any(|(device, _)| *device == stored.id))
        .then_some(())
    })
    .await;
    // The next devices_sync does not bring it back.
    harness.host.sync_devices().await.unwrap();
    assert!(harness.relay.devices(&host_id).is_empty());
}

// Finding 7: a snapshot above the window does not make a loop of snapshots.

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_snapshot_above_the_window_is_sent_once() {
    let harness = Harness::start().await;
    harness.backend.set_ring_bytes(1024 * 1024);
    let history = vec![b'h'; 700 * 1024];
    for chunk in history.chunks(4096) {
        harness.backend.push_output("term-1", chunk);
    }
    let paired = harness.pair("Phone").await;
    let (handle, _events) = harness.connect(paired).await;
    let mut terminal = handle.subscribe_terminal("term-1").await.unwrap();
    // A full batch of live output right after the snapshot, before the device acknowledges it.
    let live = vec![b'l'; 32 * 1024];
    harness.backend.push_output("term-1", &live);
    match next_update(&mut terminal).await {
        TerminalUpdate::Snapshot { data, .. } => assert_eq!(data.len(), history.len()),
        other => panic!("expected a snapshot, got {other:?}"),
    }
    // The snapshot does not count toward the window, so the batch arrives as data.
    let updates = read_until_data(&mut terminal, &live).await;
    assert!(
        updates
            .iter()
            .all(|update| matches!(update, TerminalUpdate::Data { .. })),
        "{updates:?}"
    );
    // Live output continues as data. No second snapshot comes.
    for round in 0..5 {
        tokio::time::sleep(Duration::from_millis(300)).await;
        let text = format!("live {round}");
        harness.backend.push_output("term-1", text.as_bytes());
        let updates = read_until_data(&mut terminal, text.as_bytes()).await;
        assert!(
            updates
                .iter()
                .all(|update| matches!(update, TerminalUpdate::Data { .. })),
            "round {round}: {updates:?}"
        );
    }
    handle.close().await;
}

// Finding 10: reconnect_now and retry_unauthorized.

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reconnect_now_keeps_the_unauthorized_pause_and_retry_unauthorized_ends_it() {
    let harness = Harness::start().await;
    let paired = harness.pair("Phone").await;
    harness
        .store
        .revoke(paired.device_id(), halv_remote::unix_time_ms())
        .unwrap();
    let mut config = client_config();
    config.not_authorized_pause = Duration::from_secs(60);
    let (handle, mut events) = ClientSession::connect(paired, config);
    wait_status(&handle, |status| {
        matches!(status, ClientStatus::NotAuthorized { .. })
    })
    .await;
    while events.try_recv().is_ok() {}

    // The foreground hook calls reconnect_now. The pause stays.
    handle.reconnect_now();
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(
        events.try_recv().is_err(),
        "reconnect_now must not end the pause"
    );
    assert!(matches!(
        handle.status(),
        ClientStatus::NotAuthorized { .. }
    ));

    // A tap on "Try again" ends it: the engine connects, and the host refuses again.
    handle.retry_unauthorized();
    let mut connecting = false;
    loop {
        match tokio::time::timeout(WAIT, events.recv())
            .await
            .unwrap()
            .unwrap()
        {
            ClientEvent::Status(ClientStatus::Connecting) => connecting = true,
            ClientEvent::Status(ClientStatus::NotAuthorized { .. }) if connecting => break,
            _ => {}
        }
    }
    handle.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reconnect_now_probes_an_open_session() {
    let harness = Harness::start().await;
    let paired = harness.pair("Phone").await;
    let mut config = client_config();
    config.probe_timeout = Duration::from_millis(300);
    let (handle, mut events) = ClientSession::connect(paired, config);
    wait_connected(&handle).await;
    while events.try_recv().is_ok() {}

    // A working socket answers the ping. The session stays.
    handle.reconnect_now();
    tokio::time::sleep(Duration::from_millis(800)).await;
    assert!(events.try_recv().is_err(), "a working session must stay");
    assert!(handle.panes_list().await.is_ok());

    // A socket that looks open but is dead, as after a resume on iOS.
    harness
        .relay
        .stall_device_sockets(&harness.identity.host_id());
    let started = tokio::time::Instant::now();
    handle.reconnect_now();
    let mut disconnected = false;
    loop {
        match tokio::time::timeout(WAIT, events.recv())
            .await
            .unwrap()
            .unwrap()
        {
            ClientEvent::Status(ClientStatus::Disconnected { reason, .. }) => {
                assert!(reason.contains("did not answer"), "{reason}");
                disconnected = true;
            }
            ClientEvent::Status(ClientStatus::Connected(_)) if disconnected => break,
            _ => {}
        }
    }
    // Far less than the idle timeout of 60 seconds.
    assert!(started.elapsed() < Duration::from_secs(5));
    assert!(handle.panes_list().await.is_ok());
    handle.close().await;
}
