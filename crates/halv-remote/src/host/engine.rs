//! The engine task: the host socket, the control messages, the pairing offer, and revocation.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::time::Duration;

use tokio::sync::{broadcast, mpsc, oneshot, watch};
use tokio::task::{JoinHandle, JoinSet};
use tokio::time::Instant;
use tokio_tungstenite::tungstenite::http::{HeaderName, HeaderValue};
use tokio_tungstenite::tungstenite::{Bytes, Message};

use super::conn::{self, ConnCommand, ConnTask, PairingStart};
use super::panes::PaneCache;
use super::paste::{scan_mode, PasteMode, PasteModes};
use super::{
    ConnectedDevice, DeviceStore, HostBackend, HostConfig, HostError, HostStatus, OfflineReason,
    OutputEvent, PairingApprover, PairingOfferInfo, StoreError,
};
use crate::ids::{ConnId, DeviceId, InviteId};
use crate::net::{self, Backoff, ConnectError, Incoming, Outgoing, WsStream};
use crate::pairing::{PairingOffer, PairingSecret, Psk, MAX_HOST_NAME_CHARS, OFFER_LIFETIME_MS};
use crate::relay_auth::{HostAuthHeaders, HEADER_CLIENT};
use crate::relay_wire::{
    self, CloseCode, ConnOpen, ConnPeer, DeviceEntry, HostToRelay, RelayToHost, UpgradeError,
    MAX_INVITE_TRIES, MAX_PAIRED_DEVICES,
};
use crate::rpc::{ErrorCode, PaneList};
use crate::unix_time_ms;

/// The Noise messages that can wait for one connection task.
/// A device that sends faster than the host reads gets `4429`.
const INBOX: usize = 256;

/// A connection that lasted this long resets the backoff (section 9).
const BACKOFF_RESET_AFTER: Duration = Duration::from_secs(60);

/// The time that a revoke waits for a session to queue `device.revoked`.
const REVOKE_EVENT_WAIT: Duration = Duration::from_secs(2);

/// The time that a stopping engine gives the writer to send the close frame.
const CLOSE_WAIT: Duration = Duration::from_secs(2);

/// The most device connections of one host socket (section 7.4, step 8).
const MAX_CONNECTIONS: usize = 8;

/// The most connections that have not sent their first valid transport message (section 7.4, step 8).
const MAX_OPEN_HANDSHAKES: usize = 4;

/// The `4409` closes after which the engine stops until the user turns the gateway on again.
const MAX_REPLACED: u32 = 2;

/// The state that the engine task, the connection tasks, and the handles share.
pub(crate) struct Shared {
    pub(crate) config: HostConfig,
    pub(crate) backend: Arc<dyn HostBackend>,
    pub(crate) store: Arc<dyn DeviceStore>,
    pub(crate) approver: Arc<dyn PairingApprover>,
    pub(crate) status: watch::Sender<HostStatus>,
    pub(crate) panes: PaneCache,
    paste: PasteModes,
    state: Mutex<State>,
    stop: watch::Sender<bool>,
    task: Mutex<Option<JoinHandle<()>>>,
    /// Makes [`Shared::start_pairing`] one step at a time.
    pairing: tokio::sync::Mutex<()>,
    next_offer: AtomicU64,
}

#[derive(Default)]
struct State {
    link: Option<Link>,
    offer: Option<ActiveOffer>,
    conns: HashMap<ConnId, ConnEntry>,
    last_auth_ts: u64,
    /// The devices that a revoke started for. A session of such a device takes no input,
    /// and a new session of it does not start.
    revoked: HashSet<DeviceId>,
}

/// The open host socket.
struct Link {
    generation: u64,
    out: mpsc::Sender<Outgoing>,
    pending: HashMap<u32, oneshot::Sender<Result<(), HostError>>>,
    next_rid: u32,
}

/// The active pairing offer. The PSK stays here, in memory only, until the offer ends.
struct ActiveOffer {
    id: u64,
    invite: InviteId,
    psk: Psk,
    expires_at_ms: u64,
    /// The pairing connection that waits for the user.
    approving: Option<ConnId>,
}

/// One device connection.
struct ConnEntry {
    generation: u64,
    inbox: mpsc::Sender<Bytes>,
    commands: mpsc::UnboundedSender<ConnCommand>,
    /// The device of a session, after `hello`.
    device: Option<DeviceId>,
    since_ms: u64,
    /// True until the first valid transport message. A replayed message 1 never gets past it.
    handshake: bool,
}

/// Why a host socket ended.
enum LinkEnd {
    Stopped,
    Closed(Option<u16>),
    Failed(String),
    Idle,
}

/// Runs a blocking call of the backend or the store on the blocking pool.
pub(crate) async fn blocking<T, F>(call: F) -> Result<T, StoreError>
where
    T: Send + 'static,
    F: FnOnce() -> T + Send + 'static,
{
    tokio::task::spawn_blocking(call)
        .await
        .map_err(|_| StoreError("a blocking call failed".to_owned()))
}

/// Waits until the stop flag is set or the flag can no longer change.
async fn wait_stop(stop: &mut watch::Receiver<bool>) {
    let _ = stop.wait_for(|stopped| *stopped).await;
}

impl Shared {
    /// Makes the shared state and starts the engine task and its helpers.
    pub(crate) fn start(
        config: HostConfig,
        backend: Arc<dyn HostBackend>,
        store: Arc<dyn DeviceStore>,
        approver: Arc<dyn PairingApprover>,
    ) -> Arc<Shared> {
        let (status, _) = watch::channel(HostStatus::Connecting);
        let (stop, _) = watch::channel(false);
        let pane_changes = backend.pane_changes();
        let output = backend.output();
        let shared = Arc::new(Shared {
            config,
            backend,
            store,
            approver,
            status,
            panes: PaneCache::new(unix_time_ms()),
            paste: PasteModes::default(),
            state: Mutex::new(State::default()),
            stop,
            task: Mutex::new(None),
            pairing: tokio::sync::Mutex::new(()),
            next_offer: AtomicU64::new(1),
        });
        tokio::spawn(watch_panes(
            Arc::downgrade(&shared),
            pane_changes,
            shared.stop.subscribe(),
        ));
        tokio::spawn(track_paste_modes(
            Arc::downgrade(&shared),
            output,
            shared.stop.subscribe(),
        ));
        let task = tokio::spawn(run(shared.clone()));
        *shared
            .task
            .lock()
            .unwrap_or_else(|poison| poison.into_inner()) = Some(task);
        shared
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
    }

    pub(crate) fn set_status(&self, status: HostStatus) {
        self.status.send_if_modified(|current| {
            if *current == status {
                false
            } else {
                *current = status;
                true
            }
        });
    }

    /// Asks the engine to stop. Every handle does this when the last clone drops.
    pub(crate) fn request_stop(&self) {
        self.stop.send_replace(true);
    }

    /// Stops the engine and waits for its task.
    pub(crate) async fn shutdown(&self) {
        self.request_stop();
        let task = self
            .task
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .take();
        if let Some(task) = task {
            let _ = task.await;
        }
        // The PSK of an open offer leaves memory now.
        self.lock().offer = None;
    }

    /// Opens the host socket with the signed headers of section 6.2.
    async fn connect(&self) -> Result<WsStream, ConnectError> {
        let identity = &self.config.identity;
        let timestamp = {
            let mut state = self.lock();
            // The relay rejects a timestamp that is not greater than the last one.
            let timestamp = unix_time_ms().max(state.last_auth_ts.saturating_add(1));
            state.last_auth_ts = timestamp;
            timestamp
        };
        let auth = HostAuthHeaders::sign(identity, timestamp);
        let mut headers = Vec::with_capacity(4);
        for (name, value) in auth.pairs() {
            let name = HeaderName::from_bytes(name.as_bytes()).expect("a valid header name");
            let value = HeaderValue::from_str(value).expect("b64u and decimal text are valid");
            headers.push((name, value));
        }
        if let Ok(value) = HeaderValue::from_str(&self.backend.host_info().app) {
            let name =
                HeaderName::from_bytes(HEADER_CLIENT.as_bytes()).expect("a valid header name");
            headers.push((name, value));
        }
        let url = relay_wire::host_socket_url(&self.config.relay_url, &identity.host_id());
        net::connect(&url, headers, self.config.connect_timeout).await
    }

    /// Reads the host socket until it ends.
    async fn run_link(
        self: &Arc<Self>,
        socket: WsStream,
        generation: u64,
        stop: &mut watch::Receiver<bool>,
    ) -> LinkEnd {
        let (mut reader, out, mut writer) = net::spawn_writer(socket, self.config.ping_interval);
        self.lock().link = Some(Link {
            generation,
            out: out.clone(),
            pending: HashMap::new(),
            next_rid: 1,
        });
        let mut tasks = JoinSet::new();
        let idle = self.config.idle_timeout;
        let mut deadline = Instant::now() + idle;
        let mut writer_done = false;
        let end = loop {
            tokio::select! {
                frame = net::read_frame(&mut reader) => {
                    deadline = Instant::now() + idle;
                    match frame {
                        Incoming::Binary(bytes) => self.route_data(bytes, generation, &out),
                        Incoming::Text(text) => self.on_control(&text, generation, &out, &mut tasks),
                        Incoming::Alive => {}
                        Incoming::Closed(code) => break LinkEnd::Closed(code),
                        Incoming::Failed(reason) => break LinkEnd::Failed(reason),
                    }
                }
                _ = tokio::time::sleep_until(deadline) => break LinkEnd::Idle,
                _ = wait_stop(stop) => {
                    let _ = out.try_send(Outgoing::Close(1000, "the host stopped"));
                    break LinkEnd::Stopped;
                }
                _ = &mut writer => {
                    writer_done = true;
                    break LinkEnd::Failed("the socket write failed".to_owned());
                }
                Some(_) = tasks.join_next(), if !tasks.is_empty() => {}
            }
        };

        {
            let mut state = self.lock();
            // Dropping the link fails every waiting control request with `Offline`.
            if state
                .link
                .as_ref()
                .is_some_and(|link| link.generation == generation)
            {
                state.link = None;
            }
            state
                .conns
                .retain(|_, entry| entry.generation != generation);
            if let Some(offer) = state.offer.as_mut() {
                offer.approving = None;
            }
            // The expiry timer skips an offer that waits for the user.
            // Without the connection, an expired offer ends here, and its PSK leaves memory.
            if state
                .offer
                .as_ref()
                .is_some_and(|offer| offer.expires_at_ms <= unix_time_ms())
            {
                state.offer = None;
            }
        }
        tasks.abort_all();
        drop(out);
        if !writer_done && tokio::time::timeout(CLOSE_WAIT, &mut writer).await.is_err() {
            writer.abort();
        }
        end
    }

    /// Sends a DATA frame to its connection task.
    fn route_data(&self, frame: Bytes, generation: u64, out: &mpsc::Sender<Outgoing>) {
        let Ok(data) = relay_wire::decode_data(&frame) else {
            return;
        };
        let conn = data.conn;
        let payload = frame.slice(relay_wire::DATA_HEADER_LEN..);
        let mut state = self.lock();
        let Some(entry) = state.conns.get(&conn) else {
            return;
        };
        if entry.generation != generation {
            return;
        }
        if let Err(mpsc::error::TrySendError::Full(_)) = entry.inbox.try_send(payload) {
            // The device sends faster than the host reads.
            state.conns.remove(&conn);
            drop(state);
            send_conn_close(out.clone(), conn, CloseCode::RateLimited);
        }
    }

    /// Handles a control message of the relay (section 6.4).
    fn on_control(
        self: &Arc<Self>,
        text: &str,
        generation: u64,
        out: &mpsc::Sender<Outgoing>,
        tasks: &mut JoinSet<()>,
    ) {
        let Ok(message) = RelayToHost::from_json(text) else {
            return;
        };
        match message {
            RelayToHost::Ready { .. } => {
                self.set_status(HostStatus::Online);
                let shared = self.clone();
                tasks.spawn(async move {
                    // The next `ready` sends the list again, so a failure needs no retry here.
                    let _ = shared.sync_devices().await;
                });
            }
            RelayToHost::Ok { rid } => self.resolve(generation, rid, Ok(())),
            RelayToHost::Err {
                rid: Some(rid),
                code,
                ..
            } => self.resolve(generation, rid, Err(HostError::Relay { code })),
            RelayToHost::ConnOpen(open) => self.open_conn(open, generation, out, tasks),
            RelayToHost::ConnClose { conn, .. } => self.forget_conn(conn, generation),
            // An error without a request id and an unknown message need no action.
            RelayToHost::Err { rid: None, .. } | RelayToHost::Unknown => {}
        }
    }

    fn resolve(&self, generation: u64, rid: u32, result: Result<(), HostError>) {
        let waiter = {
            let mut state = self.lock();
            match state.link.as_mut() {
                Some(link) if link.generation == generation => link.pending.remove(&rid),
                _ => None,
            }
        };
        if let Some(waiter) = waiter {
            let _ = waiter.send(result);
        }
    }

    /// Starts the task of a new device connection.
    fn open_conn(
        self: &Arc<Self>,
        open: ConnOpen,
        generation: u64,
        out: &mpsc::Sender<Outgoing>,
        tasks: &mut JoinSet<()>,
    ) {
        let (inbox_sender, inbox) = mpsc::channel(INBOX);
        let (command_sender, commands) = mpsc::unbounded_channel();
        let pairing = {
            let mut state = self.lock();
            // The relay limits the device sockets too, but the relay is not trusted (section 7.4, step 8).
            let (open_conns, handshakes) = state
                .conns
                .values()
                .filter(|entry| entry.generation == generation)
                .fold((0, 0), |(conns, handshakes), entry| {
                    (conns + 1, handshakes + usize::from(entry.handshake))
                });
            if open_conns >= MAX_CONNECTIONS || handshakes >= MAX_OPEN_HANDSHAKES {
                drop(state);
                send_conn_close(out.clone(), open.conn, CloseCode::RateLimited);
                return;
            }
            let pairing = match (&open.peer, &state.offer) {
                (ConnPeer::Invite(invite), Some(offer)) if offer.invite == *invite => {
                    Some(PairingStart {
                        offer_id: offer.id,
                        invite: invite.clone(),
                        psk: offer.psk.clone(),
                    })
                }
                _ => None,
            };
            state.conns.insert(
                open.conn,
                ConnEntry {
                    generation,
                    inbox: inbox_sender,
                    commands: command_sender,
                    device: None,
                    since_ms: unix_time_ms(),
                    handshake: true,
                },
            );
            pairing
        };
        tasks.spawn(conn::run(ConnTask {
            shared: self.clone(),
            conn: open.conn,
            peer: open.peer,
            generation,
            out: out.clone(),
            inbox,
            commands,
            pairing,
        }));
    }

    /// Removes a connection. Its task sees the closed inbox and ends.
    pub(crate) fn forget_conn(&self, conn: ConnId, generation: u64) {
        let mut state = self.lock();
        if state
            .conns
            .get(&conn)
            .is_some_and(|entry| entry.generation == generation)
        {
            state.conns.remove(&conn);
        }
    }

    /// Ends the handshake of a connection: its first valid transport message arrived.
    pub(crate) fn end_handshake(&self, conn: ConnId, generation: u64) {
        let mut state = self.lock();
        if let Some(entry) = state.conns.get_mut(&conn) {
            if entry.generation == generation {
                entry.handshake = false;
            }
        }
    }

    /// Records the device of a session, for revocation and for [`Shared::connected_devices`].
    /// It also ends the handshake.
    ///
    /// It returns false when a revoke of the device started. The caller must then end the session.
    /// The check and the mark share one lock, so a revoke either sees the session or the
    /// session sees the revoke.
    pub(crate) fn mark_session(&self, conn: ConnId, generation: u64, device: DeviceId) -> bool {
        let mut state = self.lock();
        if state.revoked.contains(&device) {
            return false;
        }
        if let Some(entry) = state.conns.get_mut(&conn) {
            if entry.generation == generation {
                entry.device = Some(device);
                entry.since_ms = unix_time_ms();
                entry.handshake = false;
            }
        }
        true
    }

    /// Returns true when a revoke of the device started.
    pub(crate) fn is_revoked(&self, device: &DeviceId) -> bool {
        self.lock().revoked.contains(device)
    }

    /// Records that a revoke of the device started. From now on, its sessions take no input.
    pub(crate) fn note_revoked(&self, device: &DeviceId) {
        self.lock().revoked.insert(device.clone());
    }

    pub(crate) fn connected_devices(&self) -> Vec<ConnectedDevice> {
        let state = self.lock();
        let mut devices: Vec<ConnectedDevice> = state
            .conns
            .iter()
            .filter_map(|(conn, entry)| {
                entry.device.clone().map(|device| ConnectedDevice {
                    device,
                    conn: *conn,
                    since_ms: entry.since_ms,
                })
            })
            .collect();
        devices.sort_by_key(|device| (device.since_ms, device.conn));
        devices
    }

    /// Sends a control message and waits for its `ok` or `err` (section 6.4).
    pub(crate) async fn control(
        &self,
        make: impl FnOnce(u32) -> HostToRelay,
    ) -> Result<(), HostError> {
        let (receiver, out, generation, rid, text) = {
            let mut state = self.lock();
            let link = state.link.as_mut().ok_or(HostError::Offline)?;
            let rid = link.next_rid;
            link.next_rid = link.next_rid.wrapping_add(1);
            let text = make(rid).to_json()?;
            let (sender, receiver) = oneshot::channel();
            link.pending.insert(rid, sender);
            (receiver, link.out.clone(), link.generation, rid, text)
        };
        if out
            .send(Outgoing::Frame(Message::text(text)))
            .await
            .is_err()
        {
            self.forget_pending(generation, rid);
            return Err(HostError::Offline);
        }
        match tokio::time::timeout(self.config.control_timeout, receiver).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err(HostError::Offline),
            Err(_) => {
                self.forget_pending(generation, rid);
                Err(HostError::Timeout)
            }
        }
    }

    fn forget_pending(&self, generation: u64, rid: u32) {
        let mut state = self.lock();
        if let Some(link) = state.link.as_mut() {
            if link.generation == generation {
                link.pending.remove(&rid);
            }
        }
    }

    /// Sends `conn_close` on the current host socket.
    pub(crate) async fn close_conn(&self, conn: ConnId, code: CloseCode) {
        let out = {
            let mut state = self.lock();
            state.conns.remove(&conn);
            state.link.as_ref().map(|link| link.out.clone())
        };
        if let Some(out) = out {
            if let Ok(text) = HostToRelay::conn_close(conn, code).to_json() {
                let _ = out.send(Outgoing::Frame(Message::text(text))).await;
            }
        }
    }

    /// Sends the active devices of the registry with `devices_sync`.
    pub(crate) async fn sync_devices(&self) -> Result<(), HostError> {
        let store = self.store.clone();
        let devices = blocking(move || store.list()).await??;
        let devices: Vec<DeviceEntry> = devices
            .into_iter()
            .filter(|device| !device.is_revoked())
            .take(MAX_PAIRED_DEVICES)
            .map(|device| DeviceEntry {
                device: device.id,
                hash: device.token_hash,
            })
            .collect();
        self.control(|rid| HostToRelay::DevicesSync { rid, devices })
            .await
    }

    /// Reads the panes of the backend and returns the shared list.
    pub(crate) async fn refresh_panes(&self) -> Arc<PaneList> {
        let backend = self.backend.clone();
        self.panes.refresh(move || backend.panes()).await
    }

    /// Returns the bracketed paste mode of a pane (section 8.8).
    ///
    /// The order: the backend, then the scanner of the live output, then a scan of the ring.
    /// Without any switch, the mode is unknown: an earlier switch can have left the ring,
    /// or the tap can have lost it.
    pub(crate) async fn paste_mode(&self, session: &str) -> PasteMode {
        let backend = self.backend.clone();
        let name = session.to_owned();
        if let Ok(Some(mode)) = blocking(move || backend.bracketed_paste(&name)).await {
            return PasteMode::from_switch(mode);
        }
        if let Some(mode) = self.paste.mode(session) {
            return PasteMode::from_switch(mode);
        }
        let backend = self.backend.clone();
        let name = session.to_owned();
        blocking(move || backend.snapshot(&name))
            .await
            .ok()
            .flatten()
            .and_then(|snapshot| scan_mode(&snapshot.data))
            .map_or(PasteMode::Unknown, PasteMode::from_switch)
    }

    /// Makes a new pairing offer and registers its invite (section 7.3, step 1).
    pub(crate) async fn start_pairing(self: &Arc<Self>) -> Result<PairingOfferInfo, HostError> {
        let _serial = self.pairing.lock().await;
        self.end_offer(None).await;
        if self.lock().link.is_none() {
            return Err(HostError::Offline);
        }
        let identity = &self.config.identity;
        let lifetime_ms = u64::try_from(self.config.offer_lifetime.as_millis())
            .unwrap_or(u64::MAX)
            .min(OFFER_LIFETIME_MS);
        let expires_at_ms = unix_time_ms().saturating_add(lifetime_ms);
        let name: String = self
            .backend
            .host_info()
            .name
            .chars()
            .take(MAX_HOST_NAME_CHARS)
            .collect();
        let offer = PairingOffer::new(
            self.config.relay_url.clone(),
            identity.host_id(),
            identity.noise_public(),
            PairingSecret::generate()?,
            expires_at_ms,
            (!name.is_empty()).then_some(name),
        )?;
        let invite = InviteId::generate()?;
        let id = self.next_offer.fetch_add(1, Ordering::Relaxed);
        let hash = offer.invite_token().hash();
        self.lock().offer = Some(ActiveOffer {
            id,
            invite: invite.clone(),
            psk: offer.psk(),
            expires_at_ms,
            approving: None,
        });
        let registered = self
            .control(|rid| HostToRelay::InvitePut {
                rid,
                invite: invite.clone(),
                hash,
                exp: expires_at_ms,
                tries: MAX_INVITE_TRIES,
            })
            .await;
        if let Err(error) = registered {
            self.take_offer(Some(id));
            return Err(error);
        }
        let shared = Arc::downgrade(self);
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(lifetime_ms)).await;
            if let Some(shared) = shared.upgrade() {
                shared.expire_offer(id).await;
            }
        });
        Ok(PairingOfferInfo {
            uri: offer.to_uri(),
            expires_at_ms,
            invite,
        })
    }

    fn take_offer(&self, id: Option<u64>) -> Option<ActiveOffer> {
        let mut state = self.lock();
        let matches = match (id, state.offer.as_ref()) {
            (_, None) => false,
            (None, Some(_)) => true,
            (Some(id), Some(offer)) => offer.id == id,
        };
        if matches {
            state.offer.take()
        } else {
            None
        }
    }

    /// Ends an offer: the PSK leaves memory and the invite leaves the relay.
    /// A connection that waits for the user gets [`ConnCommand::OfferEnded`].
    /// `id` `None` ends any offer.
    pub(crate) async fn end_offer(&self, id: Option<u64>) {
        let Some(offer) = self.take_offer(id) else {
            return;
        };
        if let Some(conn) = offer.approving {
            if let Some(entry) = self.lock().conns.get(&conn) {
                let _ = entry.commands.send(ConnCommand::OfferEnded);
            }
        }
        let invite = offer.invite.clone();
        drop(offer);
        let _ = self
            .control(|rid| HostToRelay::InviteDel { rid, invite })
            .await;
    }

    /// Ends an offer at its expiry, unless a pairing waits for the user.
    async fn expire_offer(&self, id: u64) {
        let waits = self
            .lock()
            .offer
            .as_ref()
            .is_some_and(|offer| offer.id == id && offer.approving.is_some());
        if !waits {
            self.end_offer(Some(id)).await;
        }
    }

    /// Marks a pairing connection as the one that waits for the user.
    pub(crate) fn begin_approval(&self, offer_id: u64, conn: ConnId) -> Result<(), ErrorCode> {
        let mut state = self.lock();
        match state.offer.as_mut() {
            Some(offer) if offer.id == offer_id && offer.expires_at_ms > unix_time_ms() => {
                if offer.approving.is_some() {
                    Err(ErrorCode::Busy)
                } else {
                    offer.approving = Some(conn);
                    Ok(())
                }
            }
            _ => Err(ErrorCode::Expired),
        }
    }

    /// Ends the wait of a pairing whose device left. The offer stays, unless it expired.
    pub(crate) async fn release_approval(&self, offer_id: u64, conn: ConnId) {
        let expired = {
            let mut state = self.lock();
            match state.offer.as_mut() {
                Some(offer) if offer.id == offer_id && offer.approving == Some(conn) => {
                    offer.approving = None;
                    offer.expires_at_ms <= unix_time_ms()
                }
                _ => false,
            }
        };
        if expired {
            self.end_offer(Some(offer_id)).await;
        }
    }

    /// Revokes a device (section 9).
    ///
    /// - `update_store`: set `revoked_at` first. `device.unpair` already did this.
    /// - `closing`: one more connection to close with `4401`, after `device_del`.
    pub(crate) async fn revoke_device(
        &self,
        id: &DeviceId,
        update_store: bool,
        closing: Option<ConnId>,
    ) -> Result<(), HostError> {
        // The flag comes first, so input that arrives during the store call is refused.
        self.note_revoked(id);
        if update_store {
            let store = self.store.clone();
            let target = id.clone();
            let now = unix_time_ms();
            let revoked = match blocking(move || store.revoke(&target, now)).await {
                Ok(Ok(true)) => Ok(()),
                Ok(Ok(false)) => Err(HostError::UnknownDevice),
                Ok(Err(error)) | Err(error) => Err(HostError::Store(error)),
            };
            if let Err(error) = revoked {
                // The revoke failed, so the device keeps its sessions.
                self.lock().revoked.remove(id);
                return Err(error);
            }
        }
        let sessions: Vec<(ConnId, mpsc::UnboundedSender<ConnCommand>)> = {
            let state = self.lock();
            state
                .conns
                .iter()
                .filter(|(_, entry)| entry.device.as_ref() == Some(id))
                .map(|(conn, entry)| (*conn, entry.commands.clone()))
                .collect()
        };
        // Each session queues the encrypted event before `device_del`, so the device
        // reads it before the relay closes the socket.
        for (_, commands) in &sessions {
            let (done, queued) = oneshot::channel();
            if commands.send(ConnCommand::Revoke { done }).is_ok() {
                let _ = tokio::time::timeout(REVOKE_EVENT_WAIT, queued).await;
            }
        }
        let deleted = self
            .control(|rid| HostToRelay::DeviceDel {
                rid,
                device: id.clone(),
            })
            .await;
        for conn in sessions.iter().map(|(conn, _)| *conn).chain(closing) {
            self.close_conn(conn, CloseCode::Unauthorized).await;
        }
        match deleted {
            // Without a host socket, the next `devices_sync` removes the device.
            Ok(()) | Err(HostError::Offline) => Ok(()),
            Err(error) => Err(error),
        }
    }

    /// Deletes an invite that has no offer, best effort.
    pub(crate) fn delete_stale_invite(self: &Arc<Self>, invite: InviteId) {
        let shared = Arc::downgrade(self);
        tokio::spawn(async move {
            if let Some(shared) = shared.upgrade() {
                let _ = shared
                    .control(|rid| HostToRelay::InviteDel { rid, invite })
                    .await;
            }
        });
    }
}

/// Sends `conn_close` from a context that cannot wait.
fn send_conn_close(out: mpsc::Sender<Outgoing>, conn: ConnId, code: CloseCode) {
    if let Ok(text) = HostToRelay::conn_close(conn, code).to_json() {
        tokio::spawn(async move {
            let _ = out.send(Outgoing::Frame(Message::text(text))).await;
        });
    }
}

/// The engine task: connect, read, and reconnect with backoff until the stop flag.
async fn run(shared: Arc<Shared>) {
    let mut backoff = Backoff::new(shared.config.backoff_min, shared.config.backoff_max);
    let mut stop = shared.stop.subscribe();
    let mut generation = 0u64;
    let mut replaced = 0u32;
    loop {
        if *stop.borrow() {
            break;
        }
        shared.set_status(HostStatus::Connecting);
        let connected = tokio::select! {
            result = shared.connect() => result,
            _ = wait_stop(&mut stop) => break,
        };
        let (reason, delay) = match connected {
            Ok(socket) => {
                generation += 1;
                let opened = Instant::now();
                let end = shared.run_link(socket, generation, &mut stop).await;
                if opened.elapsed() >= BACKOFF_RESET_AFTER {
                    backoff.reset();
                }
                match end {
                    LinkEnd::Stopped => break,
                    LinkEnd::Closed(Some(code)) if code == CloseCode::Replaced.code() => {
                        // Another desktop with this identity connected, or the untrusted relay
                        // sent a false 4409. Wait 5 minutes and try once more. After a second
                        // 4409, do not fight the other desktop (section 6.6).
                        replaced += 1;
                        if replaced >= MAX_REPLACED {
                            shared.set_status(HostStatus::Replaced);
                            wait_stop(&mut stop).await;
                            break;
                        }
                        (
                            OfflineReason::Closed(Some(code)),
                            shared.config.replaced_retry_delay,
                        )
                    }
                    LinkEnd::Closed(Some(code)) if code == CloseCode::RateLimited.code() => {
                        (OfflineReason::Closed(Some(code)), backoff.long_delay())
                    }
                    LinkEnd::Closed(code) => (OfflineReason::Closed(code), backoff.next_delay()),
                    LinkEnd::Failed(reason) => {
                        (OfflineReason::Network(reason), backoff.next_delay())
                    }
                    LinkEnd::Idle => (OfflineReason::IdleTimeout, backoff.next_delay()),
                }
            }
            Err(error) => {
                let delay = match &error {
                    ConnectError::Upgrade {
                        error: UpgradeError::RateLimited,
                        ..
                    } => backoff.long_delay(),
                    ConnectError::Upgrade {
                        retry_after: Some(after),
                        ..
                    } => backoff.next_delay().max(*after),
                    _ => backoff.next_delay(),
                };
                (OfflineReason::Connect(error), delay)
            }
        };
        shared.set_status(HostStatus::Offline {
            reason,
            retry_in: delay,
        });
        tokio::select! {
            _ = tokio::time::sleep(delay) => {}
            _ = wait_stop(&mut stop) => break,
        }
    }
    shared.set_status(HostStatus::Stopped);
}

/// Keeps the pane list fresh and sends a new revision to every subscribed session.
async fn watch_panes(
    shared: Weak<Shared>,
    mut changes: broadcast::Receiver<()>,
    mut stop: watch::Receiver<bool>,
) {
    let refresh = |shared: &Weak<Shared>| shared.upgrade();
    if let Some(shared) = refresh(&shared) {
        shared.refresh_panes().await;
    }
    loop {
        tokio::select! {
            change = changes.recv() => match change {
                Ok(()) | Err(broadcast::error::RecvError::Lagged(_)) => {
                    // One refresh covers every queued change.
                    while matches!(
                        changes.try_recv(),
                        Ok(()) | Err(broadcast::error::TryRecvError::Lagged(_))
                    ) {}
                    match refresh(&shared) {
                        Some(shared) => {
                            shared.refresh_panes().await;
                        }
                        None => return,
                    }
                }
                Err(broadcast::error::RecvError::Closed) => return,
            },
            _ = wait_stop(&mut stop) => return,
        }
    }
}

/// Follows the bracketed paste mode of every pane in the live output (section 8.8).
async fn track_paste_modes(
    shared: Weak<Shared>,
    mut output: broadcast::Receiver<OutputEvent>,
    mut stop: watch::Receiver<bool>,
) {
    loop {
        let event = tokio::select! {
            event = output.recv() => event,
            _ = wait_stop(&mut stop) => return,
        };
        let Some(shared) = shared.upgrade() else {
            return;
        };
        match event {
            Ok(OutputEvent::Data { session, data, .. }) => shared.paste.feed(&session, &data),
            Ok(OutputEvent::Closed { session }) => shared.paste.remove(&session),
            Ok(_) => {}
            // Lost output can hold a switch, so the mode of every pane is unknown now.
            // The next switch in the output, or a switch in the ring, makes it known again.
            Err(broadcast::error::RecvError::Lagged(_)) => shared.paste.mark_unknown(),
            Err(broadcast::error::RecvError::Closed) => return,
        }
    }
}
