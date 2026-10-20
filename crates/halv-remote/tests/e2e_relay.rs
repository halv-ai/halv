//! Conformance item 9 of section 13: the host engine and the client engine through the real
//! relay code of `relay/`.
//!
//! `scripts/remote-e2e.mjs` starts the relay with `wrangler dev` and runs this file with
//! `HALV_RELAY_E2E_URL` set, for example to `ws://127.0.0.1:8791`.
//! Without the variable, each test returns at once and passes as skipped.
//!
//! The host engine dials the relay through a TCP proxy in this file.
//! The proxy can cut the host socket and refuse new ones, as a network failure of the desktop does.
//!
//! A `wss://` URL, such as the deployed relay, also works. The proxy cannot carry TLS,
//! so the test then skips the network failure step.

#![cfg(feature = "testing")]

use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use halv_remote::client::{
    self, ClientConfig, ClientError, ClientHandle, ClientSession, ClientStatus, DeviceInfo,
    PairError, PairedHost, TerminalSubscription, TerminalUpdate,
};
use halv_remote::host::{HostConfig, HostEngine, HostHandle, HostStatus};
use halv_remote::relay_auth::bearer_header_value;
use halv_remote::relay_wire::{device_socket_url, UpgradeError};
use halv_remote::rpc::Platform;
use halv_remote::testing::{Approval, MemoryBackend, MemoryDeviceStore, TestApprover};
use halv_remote::{ErrorCode, HostId, HostIdentity, PairingOffer, PairingSecret};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;
use tokio_tungstenite::tungstenite;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;

/// The variable that names the relay under test.
const RELAY_URL_VAR: &str = "HALV_RELAY_E2E_URL";

/// The longest wait for one step. The relay runs in workerd, which is slower than the mock relay.
const WAIT: Duration = Duration::from_secs(20);

/// Returns the relay URL, or `None` when the variable is not set.
fn relay_url(test: &str) -> Option<String> {
    match std::env::var(RELAY_URL_VAR) {
        Ok(url) if !url.trim().is_empty() => Some(url.trim().trim_end_matches('/').to_owned()),
        _ => {
            eprintln!("{test}: skipped, because {RELAY_URL_VAR} is not set");
            None
        }
    }
}

/// Returns the socket address of a `ws://` relay URL.
async fn relay_addr(relay: &str) -> SocketAddr {
    let authority = relay
        .strip_prefix("ws://")
        .unwrap_or_else(|| panic!("{RELAY_URL_VAR} must be a ws:// URL of a local relay"));
    tokio::net::lookup_host(authority)
        .await
        .expect("the relay address resolves")
        .next()
        .expect("the relay address has an IP address")
}

fn host_config(relay: &str, identity: Arc<HostIdentity>) -> HostConfig {
    let mut config = HostConfig::new(relay, identity);
    config.backoff_min = Duration::from_millis(50);
    config.backoff_max = Duration::from_millis(300);
    config.approval_timeout = Duration::from_secs(10);
    config.pairing_close_delay = Duration::from_millis(500);
    config
}

/// The client settings of the test.
///
/// The retries are short, but each one is an upgrade, and the relay allows 30 upgrades
/// for each client IP and path kind in 60 seconds (section 6.7).
fn client_config() -> ClientConfig {
    let mut config = ClientConfig::new("halv-mobile/0.1.0", Platform::Ios);
    config.backoff_min = Duration::from_millis(50);
    config.backoff_max = Duration::from_millis(300);
    config.host_offline_retry_min = Duration::from_millis(250);
    config.host_offline_retry_max = Duration::from_millis(500);
    config.not_authorized_pause = Duration::from_secs(5);
    config
}

fn device_info() -> DeviceInfo {
    DeviceInfo {
        name: "E2E phone".to_owned(),
        platform: Platform::Ios,
        model: Some("iPhone".to_owned()),
        app: "halv-mobile/0.1.0".to_owned(),
    }
}

async fn wait_host(host: &HostHandle, check: impl Fn(&HostStatus) -> bool) {
    let mut status = host.subscribe_status();
    tokio::time::timeout(WAIT, status.wait_for(|status| check(status)))
        .await
        .unwrap_or_else(|_| panic!("host status not reached, last: {:?}", host.status()))
        .unwrap();
}

async fn wait_host_online(host: &HostHandle) {
    wait_host(host, |status| *status == HostStatus::Online).await;
}

async fn wait_status(handle: &ClientHandle, check: impl Fn(&ClientStatus) -> bool) {
    let mut status = handle.subscribe_status();
    tokio::time::timeout(WAIT, status.wait_for(|status| check(status)))
        .await
        .unwrap_or_else(|_| panic!("client status not reached, last: {:?}", handle.status()))
        .unwrap();
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
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

async fn next_update(subscription: &mut TerminalSubscription) -> TerminalUpdate {
    tokio::time::timeout(WAIT, subscription.recv())
        .await
        .expect("a terminal update")
        .expect("the subscription is open")
}

/// Reads updates until the terminal data ends with `expected`. Returns the updates.
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

/// Opens a device socket with `token` as the bearer token, or with no `Authorization` header.
///
/// It returns `Ok` when the relay accepts the socket, and closes the socket.
/// Otherwise it returns the HTTP status and the upgrade error. It never prints the token.
async fn device_upgrade(
    relay: &str,
    host_id: &HostId,
    token: Option<&[u8; 32]>,
) -> Result<(), (u16, Option<UpgradeError>)> {
    let mut request = device_socket_url(relay, host_id)
        .into_client_request()
        .unwrap();
    if let Some(token) = token {
        let value = bearer_header_value(token);
        request
            .headers_mut()
            .insert("Authorization", value.as_str().parse().unwrap());
    }
    match tokio_tungstenite::connect_async(request).await {
        Ok((mut socket, _)) => {
            let _ = socket.close(None).await;
            Ok(())
        }
        Err(tungstenite::Error::Http(response)) => {
            let status = response.status().as_u16();
            let body = response
                .body()
                .as_deref()
                .map(String::from_utf8_lossy)
                .unwrap_or_default();
            Err((status, UpgradeError::from_response(status, &body)))
        }
        Err(error) => panic!("the upgrade failed without an HTTP answer: {error}"),
    }
}

/// A TCP proxy in front of the relay.
///
/// It forwards every socket unchanged. For host sockets it can also cut the open
/// connections and refuse new ones, as a network failure of the desktop does.
struct Link {
    addr: SocketAddr,
    host_blocked: Arc<AtomicBool>,
    cut: watch::Sender<u64>,
    accept: tokio::task::JoinHandle<()>,
}

impl Link {
    async fn start(upstream: SocketAddr) -> Link {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let host_blocked = Arc::new(AtomicBool::new(false));
        let (cut, cut_rx) = watch::channel(0u64);
        let accept = tokio::spawn({
            let host_blocked = host_blocked.clone();
            async move {
                while let Ok((inbound, _)) = listener.accept().await {
                    tokio::spawn(forward(
                        inbound,
                        upstream,
                        host_blocked.clone(),
                        cut_rx.clone(),
                    ));
                }
            }
        });
        Link {
            addr,
            host_blocked,
            cut,
            accept,
        }
    }

    /// The relay URL through the proxy.
    fn url(&self) -> String {
        format!("ws://{}", self.addr)
    }

    /// Cuts every open host socket and refuses new ones until [`Link::restore_host`].
    fn break_host(&self) {
        self.host_blocked.store(true, Ordering::SeqCst);
        self.cut.send_modify(|generation| *generation += 1);
    }

    /// Lets host sockets through again.
    fn restore_host(&self) {
        self.host_blocked.store(false, Ordering::SeqCst);
    }
}

impl Drop for Link {
    fn drop(&mut self) {
        self.accept.abort();
    }
}

/// Forwards one TCP connection to the relay.
async fn forward(
    mut inbound: TcpStream,
    upstream: SocketAddr,
    host_blocked: Arc<AtomicBool>,
    mut cut: watch::Receiver<u64>,
) {
    // Mark the cuts before this connection as seen. A cut after this line ends it.
    cut.borrow_and_update();
    let Some(head) = read_request_line(&mut inbound).await else {
        return;
    };
    let is_host = request_path(&head).is_some_and(|path| path.ends_with("/host"));
    if is_host && host_blocked.load(Ordering::SeqCst) {
        // The engine sees a network error.
        return;
    }
    let Ok(mut outbound) = TcpStream::connect(upstream).await else {
        return;
    };
    if outbound.write_all(&head).await.is_err() {
        return;
    }
    if is_host {
        tokio::select! {
            _ = tokio::io::copy_bidirectional(&mut inbound, &mut outbound) => {}
            _ = cut.changed() => {}
        }
    } else {
        let _ = tokio::io::copy_bidirectional(&mut inbound, &mut outbound).await;
    }
}

/// Reads the bytes of a request up to the end of its first line.
async fn read_request_line(stream: &mut TcpStream) -> Option<Vec<u8>> {
    let mut head = Vec::new();
    let mut buffer = [0u8; 2048];
    while !head.windows(2).any(|pair| pair == b"\r\n") {
        if head.len() > 16 * 1024 {
            return None;
        }
        let read = stream.read(&mut buffer).await.ok()?;
        if read == 0 {
            return None;
        }
        head.extend_from_slice(&buffer[..read]);
    }
    Some(head)
}

/// Returns the path of the request line `GET <path> HTTP/1.1`.
fn request_path(head: &[u8]) -> Option<&str> {
    let line = head.split(|byte| *byte == b'\r').next()?;
    std::str::from_utf8(line).ok()?.split(' ').nth(1)
}

/// The host side of a test: a host engine with two panes in memory.
struct Desktop {
    identity: Arc<HostIdentity>,
    backend: Arc<MemoryBackend>,
    store: Arc<MemoryDeviceStore>,
    approver: Arc<TestApprover>,
    host: HostHandle,
}

impl Desktop {
    async fn start(relay: &str) -> Desktop {
        let identity = Arc::new(HostIdentity::generate().unwrap());
        let backend = MemoryBackend::new("Studio Mac");
        backend.add_pane("term-1", 80, 24);
        backend.add_pane("term-2", 100, 30);
        let store = MemoryDeviceStore::new();
        let approver = TestApprover::new(Approval::Allow);
        let host = HostEngine::start(
            host_config(relay, identity.clone()),
            backend.clone(),
            store.clone(),
            approver.clone(),
        )
        .unwrap();
        wait_host_online(&host).await;
        Desktop {
            identity,
            backend,
            store,
            approver,
            host,
        }
    }
}

// Pair, connect, list panes, stream, type, resume after a host disconnect, and revoke.

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pair_connect_stream_type_resume_and_revoke() {
    let Some(relay) = relay_url("pair_connect_stream_type_resume_and_revoke") else {
        return;
    };
    // The TCP proxy cannot carry TLS. A `wss://` relay, such as a deployed relay,
    // gets a direct host socket, and the test skips the network failure step.
    let link = if relay.starts_with("ws://") {
        Some(Link::start(relay_addr(&relay).await).await)
    } else {
        eprintln!("pair_connect_stream_type_resume_and_revoke: no TCP proxy for {relay}, so the network failure step is skipped");
        None
    };
    let host_url = link.as_ref().map_or_else(|| relay.clone(), Link::url);
    let desktop = Desktop::start(&host_url).await;
    let host_id = desktop.identity.host_id();
    let prompt_seq = desktop.backend.push_output("term-1", b"$ ");
    // Scripted output in the second pane: larger than one Noise message, so the snapshot
    // crosses the relay in several frames of the largest size.
    let history: Vec<u8> = (0..200 * 1024).map(|i| b'a' + (i % 26) as u8).collect();
    let history_seq = desktop.backend.push_output("term-2", &history);

    // Pairing: the approver allows, and both sides show the same code.
    let offer = desktop.host.start_pairing().await.unwrap();
    let pending = client::start_pairing(&offer.uri, device_info(), &client_config())
        .await
        .unwrap();
    let phone_code = pending.confirmation_code();
    assert_eq!(pending.host_name(), Some("Studio Mac"));
    let paired = pending.finish().await.unwrap();
    let requests = desktop.approver.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].code, phone_code);
    assert_eq!(requests[0].name, "E2E phone");
    assert_eq!(requests[0].platform, Platform::Ios);
    assert_eq!(paired.host_id(), &host_id);
    assert_eq!(paired.host_name(), "Studio Mac");
    let stored = desktop.store.devices();
    assert_eq!(stored.len(), 1);
    assert_eq!(&stored[0].id, paired.device_id());
    assert_eq!(stored[0].noise_public, paired.keys().noise_public());
    // The phone keeps the material as JSON.
    let paired = PairedHost::from_json(&paired.to_json()).unwrap();

    // Session: hello through the relay.
    let (handle, _events) = ClientSession::connect(paired.clone(), client_config());
    wait_connected(&handle).await;
    let hello = handle.hello().unwrap();
    assert_eq!(&hello.device, paired.device_id());
    assert_eq!(hello.host.name, "Studio Mac");
    assert!(hello.caps.iter().any(|cap| cap == "terminal.v1"));
    eventually(|| (desktop.host.connected_devices().len() == 1).then_some(())).await;

    // panes.list
    let panes = handle.panes_list().await.unwrap();
    let sessions: Vec<_> = panes
        .panes
        .iter()
        .map(|pane| (pane.session.as_str(), pane.cols, pane.rows))
        .collect();
    assert_eq!(sessions, vec![("term-1", 80, 24), ("term-2", 100, 30)]);

    // A terminal stream: the snapshot, then live output.
    let mut terminal = handle.subscribe_terminal("term-1").await.unwrap();
    assert_eq!(terminal.size(), (80, 24));
    match next_update(&mut terminal).await {
        TerminalUpdate::Snapshot {
            seq,
            cols,
            rows,
            data,
        } => {
            assert_eq!((seq, cols, rows), (prompt_seq, 80, 24));
            assert_eq!(data, b"$ ");
        }
        other => panic!("expected a snapshot, got {other:?}"),
    }
    desktop.backend.push_output("term-1", b"hello ");
    let live_seq = desktop.backend.push_output("term-1", b"world");
    let updates = read_until_data(&mut terminal, b"hello world").await;
    assert!(updates
        .iter()
        .all(|update| matches!(update, TerminalUpdate::Data { .. })));
    assert!(matches!(updates.last(), Some(TerminalUpdate::Data { seq, .. }) if *seq == live_seq));
    assert_eq!(terminal.seq(), prompt_seq);

    // A large snapshot arrives complete.
    let mut second = handle.subscribe_terminal("term-2").await.unwrap();
    match next_update(&mut second).await {
        TerminalUpdate::Snapshot { seq, data, .. } => {
            assert_eq!(seq, history_seq);
            assert!(data == history, "the snapshot has {} bytes", data.len());
        }
        other => panic!("expected a snapshot, got {other:?}"),
    }
    second.unsubscribe();

    // Input and paste reach the backend. The largest paste also crosses the relay
    // in several frames of the largest size, and reaches the PTY in writes of 32 KiB.
    // The output has no bracketed paste switch, so the mode is unknown: text with a line
    // break uses bracketed paste (section 8.8).
    let long_paste = "y".repeat(64 * 1024);
    handle.input("term-1", b"ls\r").await.unwrap();
    handle
        .paste("term-2", "echo a\necho b", true)
        .await
        .unwrap();
    handle.paste("term-1", &long_paste, false).await.unwrap();
    let half = long_paste.as_bytes()[..32 * 1024].to_vec();
    assert_eq!(
        desktop.backend.writes(),
        vec![
            ("term-1".to_owned(), b"ls\r".to_vec()),
            (
                "term-2".to_owned(),
                b"\x1b[200~echo a\necho b\x1b[201~\r".to_vec()
            ),
            ("term-1".to_owned(), half.clone()),
            ("term-1".to_owned(), half),
        ]
    );
    let missing = handle.input("term-9", b"x").await.unwrap_err();
    assert!(matches!(missing, ClientError::Rpc(error) if error.code == ErrorCode::NotFound));

    if let Some(link) = &link {
        // A forced host disconnect. The relay closes the device socket with 4408,
        // and the next device upgrade gets 503 host_offline while the desktop stays away.
        link.break_host();
        wait_host(&desktop.host, |status| {
            matches!(status, HostStatus::Offline { .. })
        })
        .await;
        wait_status(&handle, |status| {
            matches!(status, ClientStatus::HostOffline { .. })
        })
        .await;
        // Output that the phone misses while the desktop is offline.
        desktop.backend.push_output("term-1", b" missed");
        link.restore_host();

        // Both engines connect again by themselves. The phone subscribes again with fromSeq,
        // and the ring still holds the missed chunk: the host replays it as data.
        wait_host_online(&desktop.host).await;
        wait_connected(&handle).await;
        let updates = read_until_data(&mut terminal, b" missed").await;
        assert!(
            updates
                .iter()
                .all(|update| matches!(update, TerminalUpdate::Data { .. })),
            "a resume with fromSeq replays data, not a snapshot: {updates:?}"
        );
        assert!(desktop
            .backend
            .replay_requests()
            .contains(&("term-1".to_owned(), live_seq)));
        // The session works after the resume.
        desktop.backend.push_output("term-1", b" live again");
        read_until_data(&mut terminal, b" live again").await;
    }
    handle.input("term-2", b"\x03").await.unwrap();
    eventually(|| {
        desktop
            .backend
            .writes()
            .contains(&("term-2".to_owned(), b"\x03".to_vec()))
            .then_some(())
    })
    .await;

    // Revoke on the desktop: the phone gets the encrypted event, then the relay closes with 4401.
    desktop
        .host
        .revoke_device(paired.device_id())
        .await
        .unwrap();
    wait_status(&handle, |status| *status == ClientStatus::Revoked).await;
    assert!(desktop.store.get(paired.device_id()).unwrap().is_revoked());
    assert!(tokio::time::timeout(WAIT, terminal.recv())
        .await
        .unwrap()
        .is_none());
    // The relay deleted the token hash: the old relay token now gets 401.
    let refused =
        eventually_refused(&relay, &host_id, paired.keys().relay_token().as_bytes()).await;
    assert_eq!(refused, (401, Some(UpgradeError::Unauthorized)));
    let (again, _events) = ClientSession::connect(paired.clone(), client_config());
    wait_status(&again, |status| {
        matches!(status, ClientStatus::NotAuthorized { .. })
    })
    .await;
    again.close().await;
    desktop.host.shutdown().await;
}

/// Waits until the relay refuses the token. The `device_del` of a revoke can still be on its way.
async fn eventually_refused(
    relay: &str,
    host_id: &HostId,
    token: &[u8; 32],
) -> (u16, Option<UpgradeError>) {
    let deadline = tokio::time::Instant::now() + WAIT;
    loop {
        match device_upgrade(relay, host_id, Some(token)).await {
            Err(refused) => return refused,
            Ok(()) => {
                assert!(
                    tokio::time::Instant::now() < deadline,
                    "the relay still admits the revoked token"
                );
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
        }
    }
}

// An invalid invite token gets 401.

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_invalid_invite_token_gets_401() {
    let Some(relay) = relay_url("an_invalid_invite_token_gets_401") else {
        return;
    };
    let desktop = Desktop::start(&relay).await;
    let host_id = desktop.identity.host_id();
    let info = desktop.host.start_pairing().await.unwrap();
    let offer = PairingOffer::parse_uri(&info.uri, halv_remote::unix_time_ms()).unwrap();

    // An offer with another secret: the relay has no invite for its token.
    let forged = PairingOffer::new(
        offer.relay_url(),
        offer.host_id().clone(),
        *offer.host_noise_public(),
        PairingSecret::generate().unwrap(),
        offer.expires_at_ms(),
        None,
    )
    .unwrap();
    assert_eq!(
        device_upgrade(&relay, &host_id, Some(forged.invite_token().as_bytes())).await,
        Err((401, Some(UpgradeError::Unauthorized)))
    );
    // No token at all also gets 401.
    assert_eq!(
        device_upgrade(&relay, &host_id, None).await,
        Err((401, Some(UpgradeError::Unauthorized)))
    );
    // The client engine reports the 401 of a pairing as NotAuthorized.
    let error = client::pair(&forged.to_uri(), device_info(), &client_config(), |_| {})
        .await
        .unwrap_err();
    assert_eq!(error, PairError::NotAuthorized);
    assert!(desktop.approver.requests().is_empty());

    // The refused tokens did not use a try of the real invite: the real offer still pairs.
    let paired = client::pair(&info.uri, device_info(), &client_config(), |_| {})
        .await
        .unwrap();
    assert_eq!(paired.host_id(), &host_id);
    assert_eq!(desktop.store.devices().len(), 1);
    desktop.host.shutdown().await;
}
