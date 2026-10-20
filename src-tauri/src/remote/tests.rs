//! Gateway tests through the mock relay of `halv-remote`, with the client engine as the phone.
//!
//! The tests use a fake keychain, a temporary database, and fake terminals.
//! They never touch the system keychain or a real relay.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use halv_remote::client::{
    self, ClientConfig, ClientHandle, ClientSession, ClientStatus, DeviceInfo, PairedHost,
    TerminalSubscription, TerminalUpdate,
};
use halv_remote::host::HostConfig;
use halv_remote::rpc::{PaneKind, Platform};
use halv_remote::testing::MockRelay;
use halv_remote::{HostId, HostIdentity};
use tokio::sync::mpsc;

use halv_remote::host::DeviceStore;

use super::backend::tests::{session, FakeTerminals, RecordingActivity};
use super::backend::NoAgentStatus;
use super::gateway::{Gateway, GatewayDeps, RemoteEvents, RemoteSettings};
use super::identity::tests::FakeKeychain;
use super::store::tests::{new_device, TestDatabase, HOST_A};
use super::store::SqliteDeviceStore;
use super::{RemoteConnection, RemotePairingRequest, RemoteStatus};

const WAIT: Duration = Duration::from_secs(15);

struct MemorySettings {
    enabled: Mutex<bool>,
    relay_url: Mutex<Option<String>>,
}

impl RemoteSettings for MemorySettings {
    fn enabled(&self) -> bool {
        *self.enabled.lock().unwrap()
    }

    fn relay_url(&self) -> Option<String> {
        self.relay_url.lock().unwrap().clone()
    }

    fn set_enabled(&self, enabled: bool) -> Result<(), String> {
        *self.enabled.lock().unwrap() = enabled;
        Ok(())
    }

    fn set_relay_url(&self, relay_url: Option<String>) -> Result<(), String> {
        *self.relay_url.lock().unwrap() = relay_url;
        Ok(())
    }
}

struct RecordingEvents {
    statuses: Mutex<Vec<RemoteStatus>>,
    requests: mpsc::UnboundedSender<RemotePairingRequest>,
}

impl RemoteEvents for RecordingEvents {
    fn changed(&self, status: &RemoteStatus) {
        self.statuses.lock().unwrap().push(status.clone());
    }

    fn pairing_request(&self, request: &RemotePairingRequest) {
        let _ = self.requests.send(request.clone());
    }
}

struct Harness {
    relay: MockRelay,
    _database: TestDatabase,
    store: Arc<SqliteDeviceStore>,
    keychain: Arc<FakeKeychain>,
    terminals: Arc<FakeTerminals>,
    activity: Arc<RecordingActivity>,
    events: Arc<RecordingEvents>,
    requests: mpsc::UnboundedReceiver<RemotePairingRequest>,
    gateway: Gateway,
}

fn fast_config(config: &mut HostConfig) {
    config.backoff_min = Duration::from_millis(20);
    config.backoff_max = Duration::from_millis(200);
    config.approval_timeout = Duration::from_secs(10);
    config.pairing_close_delay = Duration::from_millis(500);
}

impl Harness {
    async fn start(label: &str) -> Harness {
        Harness::start_with(label, Arc::new(FakeKeychain::default())).await
    }

    async fn start_with(label: &str, keychain: Arc<FakeKeychain>) -> Harness {
        let relay = MockRelay::start().await.expect("the mock relay starts");
        let database = TestDatabase::new(label);
        let terminals = Arc::new(FakeTerminals::default());
        let mut agent = session("terminal-1", Some("claude"));
        agent.conversation_id = Some("conversation-1".to_string());
        terminals.add(agent);
        let store = Arc::new(SqliteDeviceStore::new(&database.path));
        let activity = Arc::new(RecordingActivity::default());
        let (requests_sender, requests) = mpsc::unbounded_channel();
        let events = Arc::new(RecordingEvents {
            statuses: Mutex::new(Vec::new()),
            requests: requests_sender,
        });
        let gateway = Gateway::new(GatewayDeps {
            settings: Arc::new(MemorySettings {
                enabled: Mutex::new(false),
                relay_url: Mutex::new(Some(relay.url())),
            }),
            events: events.clone(),
            keychain: keychain.keychain(),
            store: store.clone(),
            terminals: terminals.clone(),
            agent_status: Arc::new(NoAgentStatus),
            conversations: activity.clone(),
            default_relay_url: "wss://relay.halv.ai".to_string(),
            host_name: Arc::new(|| "Studio Mac".to_string()),
            configure: Some(Arc::new(fast_config)),
        });
        // The app activates the gateway when the `mobileRemote` flag is on.
        gateway.set_active(true).await;
        Harness {
            relay,
            _database: database,
            store,
            keychain,
            terminals,
            activity,
            events,
            requests,
            gateway,
        }
    }

    /// Asserts that no `remote-changed` event held the pairing offer.
    fn assert_events_hold_no_offer(&self) {
        let statuses = self.events.statuses.lock().unwrap();
        assert!(!statuses.is_empty());
        assert!(statuses.iter().all(|status| status.pairing.is_none()));
    }

    async fn enable(&self) -> RemoteStatus {
        let status = self.gateway.set_enabled(true).await.unwrap();
        assert!(status.enabled);
        self.wait_connection(RemoteConnection::Connected).await
    }

    async fn wait_connection(&self, connection: RemoteConnection) -> RemoteStatus {
        eventually(|| {
            let status = self.gateway.status();
            (status.connection == connection).then_some(status)
        })
        .await
    }

    /// Pairs a phone: the offer, the confirmation code, and the approval.
    async fn pair(&mut self, name: &str) -> PairedHost {
        let pairing = self.gateway.pairing_start().await.expect("an offer");
        assert!(pairing.uri.starts_with("halv://pair?p="));
        assert!(pairing
            .qr_svg_data_uri
            .starts_with("data:image/svg+xml;base64,"));
        assert!(self.gateway.status().pairing.is_some());

        let pending = client::start_pairing(&pairing.uri, device_info(name), &client_config())
            .await
            .expect("the handshake succeeds");
        let phone_code = pending.confirmation_code().to_string();
        let finish = tokio::spawn(pending.finish());

        let request = tokio::time::timeout(WAIT, self.requests.recv())
            .await
            .expect("a pairing request")
            .expect("the request channel is open");
        assert_eq!(request.name, name);
        assert_eq!(request.platform, "ios");
        assert_eq!(request.model.as_deref(), Some("iPhone"));
        assert_eq!(request.code.len(), 6);
        assert_eq!(request.code, phone_code, "both sides show the same code");
        assert_eq!(
            self.gateway.status().pending_approval.as_ref(),
            Some(&request)
        );

        let status = self
            .gateway
            .pairing_respond(&request.request_id, true)
            .await
            .unwrap();
        assert!(status.pending_approval.is_none());
        assert!(status.pairing.is_none());
        tokio::time::timeout(WAIT, finish)
            .await
            .expect("the pairing finishes")
            .unwrap()
            .expect("the phone is paired")
    }
}

fn device_info(name: &str) -> DeviceInfo {
    DeviceInfo {
        name: name.to_string(),
        platform: Platform::Ios,
        model: Some("iPhone".to_string()),
        app: "halv-mobile/0.1.0".to_string(),
    }
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

async fn eventually<T>(mut check: impl FnMut() -> Option<T>) -> T {
    let deadline = tokio::time::Instant::now() + WAIT;
    loop {
        if let Some(value) = check() {
            return value;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the condition was not reached"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

async fn wait_client(handle: &ClientHandle, check: impl Fn(&ClientStatus) -> bool) {
    let mut status = handle.subscribe_status();
    tokio::time::timeout(WAIT, status.wait_for(|status| check(status)))
        .await
        .unwrap_or_else(|_| panic!("client status not reached, last: {:?}", handle.status()))
        .unwrap();
}

async fn next_update(subscription: &mut TerminalSubscription) -> TerminalUpdate {
    tokio::time::timeout(WAIT, subscription.recv())
        .await
        .expect("a terminal update")
        .expect("the subscription is open")
}

/// Reads updates until the terminal shows `expected` at its end.
async fn read_until(subscription: &mut TerminalSubscription, expected: &[u8]) {
    let mut screen = Vec::new();
    while !screen.ends_with(expected) {
        match next_update(subscription).await {
            TerminalUpdate::Snapshot { data, .. } => screen = data,
            TerminalUpdate::Data { data, .. } => screen.extend_from_slice(&data),
            _ => {}
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_phone_pairs_streams_types_and_is_revoked() {
    let mut harness = Harness::start("e2e").await;
    let status = harness.enable().await;
    let host_id = status.host_id.clone().expect("the identity loaded");
    assert_eq!(status.host_name, "Studio Mac");
    assert!(!status.relay_url_is_default);
    assert!(harness.terminals.has_tap(), "the tap runs with the engine");
    assert!(
        harness.keychain.stored().is_some(),
        "the identity is in the keychain"
    );
    let stored = HostIdentity::from_keychain_json(&harness.keychain.stored().unwrap()).unwrap();
    assert_eq!(stored.host_id().to_string(), host_id);

    harness.terminals.push_output("terminal-1", "$ ready\r\n");
    let paired = harness.pair("Ana's phone").await;
    assert_eq!(paired.host_name(), "Studio Mac");
    let devices = harness.gateway.devices().unwrap();
    assert_eq!(devices.len(), 1);
    assert_eq!(devices[0].name, "Ana's phone");
    assert_eq!(devices[0].id, paired.device_id().to_string());
    let host = HostId::parse(&host_id).unwrap();
    assert_eq!(harness.relay.devices(&host).len(), 1);

    let (phone, _events) = ClientSession::connect(paired.clone(), client_config());
    wait_client(&phone, |status| {
        matches!(status, ClientStatus::Connected(_))
    })
    .await;
    let hello = phone.hello().expect("a hello result");
    assert_eq!(hello.host.name, "Studio Mac");
    assert!(hello.host.app.starts_with("halv-desktop/"));

    let panes = phone.panes_list().await.unwrap();
    assert_eq!(panes.panes.len(), 1);
    let pane = &panes.panes[0];
    assert_eq!(pane.session, "terminal-1");
    assert_eq!(pane.kind, PaneKind::Claude);
    assert_eq!(pane.title, "Claude Code");
    assert_eq!(pane.project.as_deref(), Some("halv"));
    assert_eq!((pane.cols, pane.rows), (100, 30));

    let connected = eventually(|| {
        let status = harness.gateway.status();
        status
            .devices
            .first()
            .filter(|device| device.connected && device.last_seen_at.is_some())
            .cloned()
    })
    .await;
    assert_eq!(connected.name, "Ana's phone");

    // The snapshot holds the ring, and live output follows through the tap.
    let mut terminal = phone.subscribe_terminal("terminal-1").await.unwrap();
    read_until(&mut terminal, b"$ ready\r\n").await;
    harness
        .terminals
        .push_output("terminal-1", "hello from the desktop");
    read_until(&mut terminal, b"hello from the desktop").await;

    // Input reaches the terminal source and updates the conversation, like `terminal_write`.
    phone.input("terminal-1", b"ls\r").await.unwrap();
    assert_eq!(
        harness.activity.touched.lock().unwrap().as_slice(),
        ["conversation-1".to_string()]
    );
    // A Claude Code pane always uses bracketed paste. The backend wins over the output,
    // so a multi-line paste never submits line by line.
    harness.terminals.push_output("terminal-1", "\x1b[?2004l");
    phone
        .paste("terminal-1", "echo one\necho two", true)
        .await
        .unwrap();
    let writes = harness.terminals.writes.lock().unwrap().clone();
    assert_eq!(writes[0], ("terminal-1".to_string(), b"ls\r".to_vec()));
    assert_eq!(
        writes[1],
        (
            "terminal-1".to_string(),
            b"\x1b[200~echo one\necho two\x1b[201~\r".to_vec()
        )
    );

    // A revoke sends `device.revoked`, and the registry hides the device.
    let device_id = paired.device_id().to_string();
    let status = harness.gateway.device_revoke(&device_id).await.unwrap();
    assert!(status.devices.is_empty());
    wait_client(&phone, |status| *status == ClientStatus::Revoked).await;
    eventually(|| harness.relay.devices(&host).is_empty().then_some(())).await;
    assert!(harness.gateway.device_revoke(&device_id).await.is_ok());
    assert!(harness.gateway.device_revoke("not-a-device").await.is_err());

    // Turning the gateway off closes the host socket and removes the tap.
    let status = harness.gateway.set_enabled(false).await.unwrap();
    assert_eq!(status.connection, RemoteConnection::Off);
    assert!(!harness.terminals.has_tap());
    eventually(|| (!harness.relay.host_connected(&host)).then_some(())).await;
    harness.assert_events_hold_no_offer();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn events_never_hold_the_pairing_offer() {
    let harness = Harness::start("event-offer").await;
    harness.enable().await;
    let pairing = harness.gateway.pairing_start().await.unwrap();
    // The command result and the status hold the offer. The event does not.
    assert_eq!(
        harness.gateway.status().pairing.map(|offer| offer.uri),
        Some(pairing.uri)
    );
    let last = harness
        .events
        .statuses
        .lock()
        .unwrap()
        .last()
        .cloned()
        .unwrap();
    assert!(last.pairing.is_none());
    harness.assert_events_hold_no_offer();
    harness.gateway.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn last_seen_updates_when_a_phone_disconnects() {
    let mut harness = Harness::start("last-seen").await;
    harness.enable().await;
    let paired = harness.pair("Ana's phone").await;
    let (phone, _events) = ClientSession::connect(paired, client_config());
    wait_client(&phone, |status| {
        matches!(status, ClientStatus::Connected(_))
    })
    .await;
    let started = eventually(|| {
        let device = harness.gateway.status().devices.first().cloned()?;
        (device.connected && device.last_seen_at.is_some()).then_some(device)
    })
    .await
    .last_seen_at
    .unwrap();

    tokio::time::sleep(Duration::from_millis(30)).await;
    let before_close = harness.events.statuses.lock().unwrap().len();
    phone.close().await;
    // The end of the session updates last seen.
    let ended = eventually(|| {
        let device = harness.gateway.status().devices.first().cloned()?;
        (!device.connected && device.last_seen_at > Some(started)).then_some(device)
    })
    .await
    .last_seen_at;
    eventually(|| {
        let statuses = harness.events.statuses.lock().unwrap();
        let device = statuses.last()?.devices.first()?.clone();
        (!device.connected && device.last_seen_at == ended).then_some(())
    })
    .await;
    // No event after the close shows the disconnect with the time of the session start.
    let statuses = harness.events.statuses.lock().unwrap().clone();
    assert!(!statuses[before_close..].iter().any(|status| {
        status
            .devices
            .first()
            .is_some_and(|device| !device.connected && device.last_seen_at == Some(started))
    }));
    harness.gateway.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_denied_phone_is_not_paired() {
    let mut harness = Harness::start("deny").await;
    harness.enable().await;
    let pairing = harness.gateway.pairing_start().await.unwrap();
    let pending = client::start_pairing(&pairing.uri, device_info("Guest"), &client_config())
        .await
        .unwrap();
    let finish = tokio::spawn(pending.finish());
    let request = tokio::time::timeout(WAIT, harness.requests.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(harness
        .gateway
        .pairing_respond("other-id", true)
        .await
        .is_err());
    harness
        .gateway
        .pairing_respond(&request.request_id, false)
        .await
        .unwrap();
    let result = tokio::time::timeout(WAIT, finish).await.unwrap().unwrap();
    assert!(matches!(result, Err(client::PairError::Denied)));
    assert!(harness.gateway.devices().unwrap().is_empty());
    harness.gateway.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_identity_reset_revokes_devices_and_changes_the_host_id() {
    let mut harness = Harness::start("reset").await;
    let before = harness.enable().await.host_id.unwrap();
    let paired = harness.pair("Old phone").await;
    let (phone, _events) = ClientSession::connect(paired, client_config());
    wait_client(&phone, |status| {
        matches!(status, ClientStatus::Connected(_))
    })
    .await;

    let status = harness.gateway.identity_reset().await.unwrap();
    let after = status.host_id.clone().unwrap();
    assert_ne!(before, after);
    assert!(status.devices.is_empty());
    wait_client(&phone, |status| *status == ClientStatus::Revoked).await;
    let stored = HostIdentity::from_keychain_json(&harness.keychain.stored().unwrap()).unwrap();
    assert_eq!(stored.host_id().to_string(), after);
    let status = harness.wait_connection(RemoteConnection::Connected).await;
    assert_eq!(status.host_id.as_deref(), Some(after.as_str()));
    assert!(harness
        .relay
        .host_connected(&HostId::parse(&after).unwrap()));
    harness.gateway.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_identity_reset_ends_a_waiting_pairing_first() {
    let mut harness = Harness::start("reset-pairing").await;
    let old_host_id = harness.enable().await.host_id.unwrap();
    let pairing = harness.gateway.pairing_start().await.unwrap();
    let pending = client::start_pairing(&pairing.uri, device_info("Late phone"), &client_config())
        .await
        .unwrap();
    let finish = tokio::spawn(pending.finish());
    let request = tokio::time::timeout(WAIT, harness.requests.recv())
        .await
        .unwrap()
        .unwrap();

    let status = harness.gateway.identity_reset().await.unwrap();
    assert!(status.pairing.is_none());
    assert!(status.pending_approval.is_none());
    // The request ended with the offer, so a late "allow" pairs nothing.
    assert!(harness
        .gateway
        .pairing_respond(&request.request_id, true)
        .await
        .is_err());
    let result = tokio::time::timeout(WAIT, finish).await.unwrap().unwrap();
    assert!(result.is_err(), "the phone is not paired");
    assert!(harness.gateway.devices().unwrap().is_empty());
    assert!(harness
        .store
        .for_host(old_host_id)
        .list()
        .unwrap()
        .is_empty());
    harness.gateway.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn devices_of_another_host_identity_stay_apart() {
    let mut harness = Harness::start("host-scope").await;
    // A device that a build with another identity paired, in the same database.
    let other = harness.store.for_host(HOST_A);
    let other_phone = other.insert(new_device(42, 1_000)).unwrap();
    assert!(
        harness.gateway.status().devices.is_empty(),
        "before the identity loads, no device is known"
    );
    assert!(harness
        .gateway
        .device_revoke(other_phone.id.as_str())
        .await
        .is_err());

    let host_id = harness.enable().await.host_id.unwrap();
    assert!(harness.gateway.devices().unwrap().is_empty());
    let paired = harness.pair("Own phone").await;
    let devices = harness.gateway.devices().unwrap();
    assert_eq!(devices.len(), 1);
    assert_eq!(devices[0].id, paired.device_id().to_string());
    // The relay gets only the devices of this host.
    let host = HostId::parse(&host_id).unwrap();
    assert_eq!(harness.relay.devices(&host).len(), 1);
    // A revoke of a device of another host fails, and the device stays active.
    assert!(harness
        .gateway
        .device_revoke(other_phone.id.as_str())
        .await
        .is_err());

    // A reset revokes only the devices of the old identity.
    harness.gateway.identity_reset().await.unwrap();
    let old = harness.store.for_host(host_id.as_str());
    assert!(old.active().unwrap().is_empty());
    assert_eq!(old.list().unwrap().len(), 1);
    assert_eq!(other.active().unwrap(), vec![other_phone]);
    harness.gateway.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_keychain_read_error_keeps_the_gateway_off() {
    let keychain = Arc::new(FakeKeychain::default());
    *keychain.read_error.lock().unwrap() = Some("the keychain is locked".to_string());
    let harness = Harness::start_with("keychain-error", keychain).await;
    let status = harness.gateway.set_enabled(true).await.unwrap();
    assert!(status.enabled);
    assert_eq!(status.connection, RemoteConnection::Error);
    assert!(status.error.as_deref().unwrap().contains("locked"));
    assert!(status.host_id.is_none());
    assert_eq!(*harness.keychain.writes.lock().unwrap(), 0);
    assert!(!harness.terminals.has_tap());
    assert!(harness.gateway.pairing_start().await.is_err());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_disabled_gateway_makes_no_relay_connection() {
    let keychain = Arc::new(FakeKeychain::default());
    let identity = HostIdentity::generate().unwrap();
    super::identity::store(&keychain.keychain(), &identity).unwrap();
    let harness = Harness::start_with("disabled", keychain).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let status = harness.gateway.status();
    assert!(!status.enabled);
    assert_eq!(status.connection, RemoteConnection::Off);
    assert!(
        status.host_id.is_none(),
        "the identity loads only when enabled"
    );
    assert!(!harness.relay.host_connected(&identity.host_id()));
    assert!(!harness.terminals.has_tap());

    // The relay URL setter checks the URL.
    assert!(harness
        .gateway
        .set_relay_url(Some("ws://relay.example.test".to_string()))
        .await
        .is_err());
    let status = harness.gateway.set_relay_url(None).await.unwrap();
    assert!(status.relay_url_is_default);
    assert_eq!(status.relay_url, "wss://relay.halv.ai");
    assert!(!harness.relay.host_connected(&identity.host_id()));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_inactive_gateway_ignores_the_stored_preference() {
    let keychain = Arc::new(FakeKeychain::default());
    let identity = HostIdentity::generate().unwrap();
    super::identity::store(&keychain.keychain(), &identity).unwrap();
    let harness = Harness::start_with("inactive", keychain).await;

    // The flag turns off: the gateway is inactive, and the preference turns on.
    harness.gateway.set_active(false).await;
    let status = harness.gateway.set_enabled(true).await.unwrap();
    assert!(status.enabled);
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(harness.gateway.status().connection, RemoteConnection::Off);
    assert!(!harness.relay.host_connected(&identity.host_id()));
    assert!(harness.gateway.pairing_start().await.is_err());

    // The flag turns on: the stored preference starts the engine.
    harness.gateway.set_active(true).await;
    harness.wait_connection(RemoteConnection::Connected).await;
    eventually(|| {
        harness
            .relay
            .host_connected(&identity.host_id())
            .then_some(())
    })
    .await;

    // The flag turns off again: the engine stops, and the preference stays on.
    let status = harness.gateway.set_active(false).await;
    assert!(status.enabled);
    assert_eq!(status.connection, RemoteConnection::Off);
    eventually(|| (!harness.relay.host_connected(&identity.host_id())).then_some(())).await;
    assert!(!harness.terminals.has_tap());
}
