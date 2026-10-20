//! A client session with automatic reconnects (sections 7.4, 8, and 10).
//!
//! One engine task owns the socket and the [`Transport`]. The handle and the subscriptions
//! talk to it through a command channel.

use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use ciborium::Value;
use serde::Serialize;
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::JoinHandle;
use tokio::time::Instant;
use tokio_tungstenite::tungstenite::http::header::AUTHORIZATION;
use tokio_tungstenite::tungstenite::{Bytes, Message as WsMessage};

use super::{ClientConfig, ClientError, ClientEvent, ClientStatus, PairedHost, TerminalUpdate};
use crate::framing::Transport;
use crate::ids::RequestId;
use crate::net::{self, random_between, Backoff, ConnectError, Incoming, Outgoing, WsReader};
use crate::noise::{HandshakePayload, SessionInitiator};
use crate::relay_auth::bearer_header_value;
use crate::relay_wire::{device_socket_url, CloseCode, UpgradeError};
use crate::rpc::{
    self, method, DeviceEvent, Empty, HelloParams, HelloResult, HostEvent, Message, Method,
    PaneList, RpcError, TerminalAck, TerminalInputParams, TerminalPasteParams,
    TerminalSubscribeParams, TerminalSubscribeResult, TerminalUnsubscribeParams,
    MAX_TERMINAL_INPUT, MAX_TERMINAL_PASTE,
};
use crate::Error;

/// The events that can wait for the app. A full queue drops new events.
const EVENT_QUEUE: usize = 64;

/// A connection that lasted this long resets the backoff (section 10).
const BACKOFF_RESET_AFTER: Duration = Duration::from_secs(60);

/// The time that a closing socket gets to send its close frame.
const CLOSE_WAIT: Duration = Duration::from_secs(2);

/// A command to the engine task.
enum Command {
    Call {
        method: &'static str,
        params: Value,
        reply: oneshot::Sender<Result<Value, ClientError>>,
    },
    SubscribePanes {
        reply: oneshot::Sender<Result<PaneList, ClientError>>,
    },
    UnsubscribePanes {
        reply: oneshot::Sender<Result<Value, ClientError>>,
    },
    SubscribeTerminal {
        session: String,
        reply: oneshot::Sender<Result<TerminalSubscription, ClientError>>,
    },
    Unsubscribe {
        local: u64,
    },
    Applied {
        local: u64,
    },
    ReconnectNow,
    RetryUnauthorized,
    Close,
}

/// A request to end the wait before the next attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Wake {
    /// From [`ClientHandle::reconnect_now`]. The pause after "Not authorized" stays.
    Soon,
    /// From [`ClientHandle::retry_unauthorized`]. It ends every wait.
    Always,
}

/// What the app applied of one subscription.
#[derive(Debug, Default)]
struct Progress {
    applied: Option<u64>,
    /// A snapshot was applied and needs its own ack.
    snapshot: bool,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poison| poison.into_inner())
}

/// The state that the engine and the handles share.
struct Shared {
    status: watch::Sender<ClientStatus>,
    hello: Mutex<Option<HelloResult>>,
    task: Mutex<Option<JoinHandle<()>>>,
}

/// Starts client sessions.
pub struct ClientSession;

impl ClientSession {
    /// Starts a session engine on the current tokio runtime.
    ///
    /// The engine connects at once, sends `hello`, and reconnects with backoff.
    /// After a reconnect it subscribes again: the pane list, and each terminal with `fromSeq`.
    /// The receiver gets the state changes and the pane list changes.
    pub fn connect(
        paired: PairedHost,
        config: ClientConfig,
    ) -> (ClientHandle, mpsc::Receiver<ClientEvent>) {
        let (events, event_receiver) = mpsc::channel(EVENT_QUEUE);
        let (command_sender, commands) = mpsc::unbounded_channel();
        let (status, _) = watch::channel(ClientStatus::Connecting);
        let shared = Arc::new(Shared {
            status,
            hello: Mutex::new(None),
            task: Mutex::new(None),
        });
        let backoff = Backoff::new(config.backoff_min, config.backoff_max);
        let engine = Engine {
            paired: Arc::new(paired),
            config: Arc::new(config),
            commands,
            command_sender: command_sender.clone(),
            events,
            shared: shared.clone(),
            subs: BTreeMap::new(),
            next_local: 1,
            panes_subscribed: false,
            backoff,
            stop: None,
            wake: None,
        };
        let task = tokio::spawn(engine.run());
        *lock(&shared.task) = Some(task);
        let handle = ClientHandle {
            owner: Arc::new(HandleOwner {
                commands: command_sender,
                shared,
            }),
        };
        (handle, event_receiver)
    }
}

/// Closes the session when the last [`ClientHandle`] drops.
struct HandleOwner {
    commands: mpsc::UnboundedSender<Command>,
    shared: Arc<Shared>,
}

impl Drop for HandleOwner {
    fn drop(&mut self) {
        let _ = self.commands.send(Command::Close);
    }
}

/// Controls a client session. Clones control the same session.
/// The session closes when the last clone drops.
#[derive(Clone)]
pub struct ClientHandle {
    owner: Arc<HandleOwner>,
}

impl fmt::Debug for ClientHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ClientHandle")
            .field("status", &self.status())
            .finish_non_exhaustive()
    }
}

impl ClientHandle {
    fn send(&self, command: Command) -> Result<(), ClientError> {
        self.owner
            .commands
            .send(command)
            .map_err(|_| ClientError::Closed)
    }

    /// Returns the current state.
    pub fn status(&self) -> ClientStatus {
        self.owner.shared.status.borrow().clone()
    }

    /// Returns a receiver that sees each change of the state.
    pub fn subscribe_status(&self) -> watch::Receiver<ClientStatus> {
        self.owner.shared.status.subscribe()
    }

    /// Returns the `hello` result of the open session: the host, the device id, and the capabilities.
    pub fn hello(&self) -> Option<HelloResult> {
        lock(&self.owner.shared.hello).clone()
    }

    async fn call<M: Method>(&self, params: &M::Params) -> Result<M::Result, ClientError> {
        let params = Value::serialized(params)
            .map_err(|_| ClientError::Protocol("the params do not encode".to_owned()))?;
        let (reply, response) = oneshot::channel();
        self.send(Command::Call {
            method: M::NAME,
            params,
            reply,
        })?;
        let value = response.await.map_err(|_| ClientError::Closed)??;
        value
            .deserialized()
            .map_err(|_| ClientError::Protocol("the response does not decode".to_owned()))
    }

    /// Returns the pane list (`panes.list`).
    pub async fn panes_list(&self) -> Result<PaneList, ClientError> {
        self.call::<rpc::PanesList>(&Empty {}).await
    }

    /// Subscribes to the pane list (`panes.subscribe`).
    ///
    /// Changes arrive as [`ClientEvent::PanesChanged`]. After a reconnect the engine
    /// subscribes again and sends the new list as [`ClientEvent::PanesChanged`].
    pub async fn subscribe_panes(&self) -> Result<PaneList, ClientError> {
        let (reply, response) = oneshot::channel();
        self.send(Command::SubscribePanes { reply })?;
        response.await.map_err(|_| ClientError::Closed)?
    }

    /// Ends the pane list subscription (`panes.unsubscribe`).
    pub async fn unsubscribe_panes(&self) -> Result<(), ClientError> {
        let (reply, response) = oneshot::channel();
        self.send(Command::UnsubscribePanes { reply })?;
        response.await.map_err(|_| ClientError::Closed)?.map(|_| ())
    }

    /// Subscribes to the output of a pane (`terminal.subscribe`).
    ///
    /// The subscription survives reconnects: the engine subscribes again with `fromSeq`
    /// set to the last sequence number that it delivered.
    pub async fn subscribe_terminal(
        &self,
        session: &str,
    ) -> Result<TerminalSubscription, ClientError> {
        let (reply, response) = oneshot::channel();
        self.send(Command::SubscribeTerminal {
            session: session.to_owned(),
            reply,
        })?;
        response.await.map_err(|_| ClientError::Closed)?
    }

    /// Writes input to a pane (`terminal.input`, at most 4 KiB).
    pub async fn input(&self, session: &str, data: &[u8]) -> Result<(), ClientError> {
        if data.len() > MAX_TERMINAL_INPUT {
            return Err(ClientError::TooLarge);
        }
        let params = TerminalInputParams {
            session: session.to_owned(),
            data: data.to_vec(),
        };
        self.call::<rpc::TerminalInput>(&params).await.map(|_| ())
    }

    /// Pastes text into a pane (`terminal.paste`, at most 64 KiB).
    /// With `submit`, the host then writes `\r`.
    pub async fn paste(&self, session: &str, text: &str, submit: bool) -> Result<(), ClientError> {
        if text.len() > MAX_TERMINAL_PASTE {
            return Err(ClientError::TooLarge);
        }
        let params = TerminalPasteParams {
            session: session.to_owned(),
            text: text.to_owned(),
            submit,
        };
        self.call::<rpc::TerminalPaste>(&params).await.map(|_| ())
    }

    /// Removes this device from the host (`device.unpair`).
    ///
    /// After the answer, the host sends `device.revoked` and the engine stops
    /// with [`ClientStatus::Revoked`]. The app then deletes the keys.
    pub async fn unpair(&self) -> Result<(), ClientError> {
        self.call::<rpc::DeviceUnpair>(&Empty {}).await.map(|_| ())
    }

    /// Ends the wait before the next attempt, for example when the app returns to the foreground.
    ///
    /// - It does not end the 5-minute pause after "Not authorized" (section 6.6).
    ///   Use [`ClientHandle::retry_unauthorized`] for an explicit user action.
    /// - On an open session, it checks the socket: it sends a WebSocket ping, and it
    ///   connects again when no ping or pong arrives in [`ClientConfig::probe_timeout`]
    ///   (5 seconds). After a resume on iOS, a dead socket can look open.
    pub fn reconnect_now(&self) {
        let _ = self.send(Command::ReconnectNow);
    }

    /// Ends every wait before the next attempt, also the 5-minute pause after "Not authorized".
    ///
    /// Call it only after an explicit user action, for example a tap on "Try again".
    /// On an open session, it checks the socket like [`ClientHandle::reconnect_now`].
    pub fn retry_unauthorized(&self) {
        let _ = self.send(Command::RetryUnauthorized);
    }

    /// Closes the session and waits for the engine to stop.
    pub async fn close(&self) {
        let _ = self.send(Command::Close);
        let task = lock(&self.owner.shared.task).take();
        if let Some(task) = task {
            let _ = task.await;
        }
    }
}

/// The output of one pane (section 8.8).
///
/// [`TerminalSubscription::recv`] marks each snapshot and data update as applied, and the
/// engine acknowledges them: at most once every 250 ms, and once after each snapshot.
/// The host stops the data when 512 KiB are not acknowledged, so a reader that falls
/// behind gets a new snapshot later instead of an unbounded queue.
///
/// Dropping the value ends the subscription.
pub struct TerminalSubscription {
    local: u64,
    session: String,
    cols: u16,
    rows: u16,
    seq: u64,
    updates: mpsc::UnboundedReceiver<TerminalUpdate>,
    progress: Arc<Mutex<Progress>>,
    commands: mpsc::UnboundedSender<Command>,
}

impl fmt::Debug for TerminalSubscription {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TerminalSubscription")
            .field("session", &self.session)
            .field("cols", &self.cols)
            .field("rows", &self.rows)
            .field("seq", &self.seq)
            .finish_non_exhaustive()
    }
}

impl TerminalSubscription {
    /// Returns the terminal session id of the pane.
    pub fn session(&self) -> &str {
        &self.session
    }

    /// Returns the PTY size at the subscribe.
    pub fn size(&self) -> (u16, u16) {
        (self.cols, self.rows)
    }

    /// Returns the sequence number of the last chunk at the subscribe.
    pub fn seq(&self) -> u64 {
        self.seq
    }

    /// Waits for the next update and marks it as applied.
    ///
    /// It returns `None` when the subscription ended: after [`TerminalUpdate::Closed`],
    /// or when the session stopped.
    pub async fn recv(&mut self) -> Option<TerminalUpdate> {
        let update = self.updates.recv().await?;
        self.applied(&update);
        Some(update)
    }

    /// Ends the subscription (`terminal.unsubscribe`). Dropping the value does the same.
    pub fn unsubscribe(self) {
        drop(self);
    }

    /// Returns the next update if one waits, and marks it as applied.
    pub fn try_recv(&mut self) -> Option<TerminalUpdate> {
        let update = self.updates.try_recv().ok()?;
        self.applied(&update);
        Some(update)
    }

    fn applied(&self, update: &TerminalUpdate) {
        let (seq, snapshot) = match update {
            TerminalUpdate::Snapshot { seq, .. } => (*seq, true),
            TerminalUpdate::Data { seq, .. } => (*seq, false),
            _ => return,
        };
        {
            let mut progress = lock(&self.progress);
            progress.applied = Some(seq);
            progress.snapshot |= snapshot;
        }
        let _ = self.commands.send(Command::Applied { local: self.local });
    }
}

impl Drop for TerminalSubscription {
    fn drop(&mut self) {
        let _ = self
            .commands
            .send(Command::Unsubscribe { local: self.local });
    }
}

/// One subscription in the engine.
struct LocalSub {
    session: String,
    updates: mpsc::UnboundedSender<TerminalUpdate>,
    progress: Arc<Mutex<Progress>>,
    /// The subscription id on the open connection.
    host_sub: Option<u32>,
    /// The newest sequence number in the channel. The engine resumes with it (section 10).
    received: Option<u64>,
    acked: Option<u64>,
    last_ack: Option<Instant>,
    /// The newest PTY size in the channel: `(cols, rows)`.
    size: (u16, u16),
}

/// A request that waits for its response.
enum Pending {
    Hello,
    Call(oneshot::Sender<Result<Value, ClientError>>),
    PanesSubscribe(Option<oneshot::Sender<Result<PaneList, ClientError>>>),
    Subscribe {
        session: String,
        /// A subscription to restore after a reconnect.
        local: Option<u64>,
        reply: Option<oneshot::Sender<Result<TerminalSubscription, ClientError>>>,
    },
    Ignore,
}

impl Pending {
    /// Answers a request that will get no response.
    fn fail(self, error: ClientError) {
        match self {
            Pending::Call(reply) => {
                let _ = reply.send(Err(error));
            }
            Pending::PanesSubscribe(Some(reply)) => {
                let _ = reply.send(Err(error));
            }
            Pending::Subscribe {
                reply: Some(reply), ..
            } => {
                let _ = reply.send(Err(error));
            }
            _ => {}
        }
    }
}

struct PendingEntry {
    kind: Pending,
    deadline: Instant,
}

/// An open connection.
struct Connection {
    reader: WsReader,
    out: mpsc::Sender<Outgoing>,
    writer: Option<JoinHandle<()>>,
    transport: Transport,
    pending: HashMap<u32, PendingEntry>,
    next_id: u32,
    host_subs: HashMap<u32, u64>,
    opened: Instant,
    last_frame: Instant,
    /// The time by which a ping or a pong must arrive, after [`ClientHandle::reconnect_now`].
    probe_deadline: Option<Instant>,
}

impl Connection {
    fn next_request_id(&mut self) -> RequestId {
        loop {
            self.next_id = self.next_id.wrapping_add(1);
            if let Some(id) = RequestId::new(self.next_id) {
                if !self.pending.contains_key(&id.get()) {
                    return id;
                }
            }
        }
    }

    /// Sends a close frame and stops the writer after a short wait.
    fn close(&mut self, code: u16, reason: &'static str) {
        let _ = self.out.try_send(Outgoing::Close(code, reason));
        if let Some(mut writer) = self.writer.take() {
            tokio::spawn(async move {
                if tokio::time::timeout(CLOSE_WAIT, &mut writer).await.is_err() {
                    writer.abort();
                }
            });
        }
    }
}

/// Why a connection ended.
#[derive(Debug)]
enum End {
    Connect(ConnectError),
    Closed(Option<u16>),
    Network(String),
    Protocol(String),
    Timeout(&'static str),
    Incompatible,
    Revoked,
    UserClosed,
    Lifetime,
    /// The socket did not answer the ping of [`ClientHandle::reconnect_now`].
    Unresponsive,
}

impl End {
    fn describe(&self) -> String {
        match self {
            End::Connect(error) => error.to_string(),
            End::Closed(Some(code)) => format!("the relay closed the connection ({code})"),
            End::Closed(None) => "the connection closed".to_owned(),
            End::Network(reason) => format!("network error: {reason}"),
            End::Protocol(reason) => format!("protocol error: {reason}"),
            End::Timeout(what) => format!("timeout: {what}"),
            End::Incompatible => "the versions do not overlap".to_owned(),
            End::Revoked => "access revoked".to_owned(),
            End::UserClosed => "closed".to_owned(),
            End::Lifetime => "the connection reached its 24 hour limit".to_owned(),
            End::Unresponsive => "the connection did not answer".to_owned(),
        }
    }
}

type Step = Result<(), End>;

/// The engine task.
struct Engine {
    paired: Arc<PairedHost>,
    config: Arc<ClientConfig>,
    commands: mpsc::UnboundedReceiver<Command>,
    command_sender: mpsc::UnboundedSender<Command>,
    events: mpsc::Sender<ClientEvent>,
    shared: Arc<Shared>,
    subs: BTreeMap<u64, LocalSub>,
    next_local: u64,
    panes_subscribed: bool,
    backoff: Backoff,
    /// The final state, once the engine must stop.
    stop: Option<ClientStatus>,
    /// A request to end the wait before the next attempt.
    wake: Option<Wake>,
}

impl Engine {
    async fn run(mut self) {
        while self.stop.is_none() {
            self.set_status(ClientStatus::Connecting);
            let attempt = connect_session(self.paired.clone(), self.config.clone());
            tokio::pin!(attempt);
            let result = loop {
                tokio::select! {
                    result = &mut attempt => break Some(result),
                    command = self.commands.recv() => {
                        self.on_command_offline(command);
                        if self.stop.is_some() {
                            break None;
                        }
                    }
                }
            };
            let Some(result) = result else {
                break;
            };
            let end = match result {
                Ok(mut conn) => {
                    let end = self.serve(&mut conn).await;
                    self.end_connection(&mut conn, &end);
                    if conn.opened.elapsed() >= BACKOFF_RESET_AFTER {
                        self.backoff.reset();
                    }
                    end
                }
                Err(end) => end,
            };
            let Some((status, delay)) = self.after(end) else {
                break;
            };
            // Only an explicit user action ends the pause after "Not authorized" (section 6.6).
            let unauthorized = matches!(status, ClientStatus::NotAuthorized { .. });
            self.set_status(status);
            self.wake = None;
            let sleep = tokio::time::sleep(delay);
            tokio::pin!(sleep);
            loop {
                tokio::select! {
                    _ = &mut sleep => break,
                    command = self.commands.recv() => {
                        self.on_command_offline(command);
                        if self.stop.is_some() {
                            break;
                        }
                        match self.wake.take() {
                            Some(Wake::Always) => break,
                            Some(Wake::Soon) if !unauthorized => break,
                            _ => {}
                        }
                    }
                }
            }
        }
        let status = self.stop.take().unwrap_or(ClientStatus::Closed);
        // Dropping the senders ends every subscription.
        self.subs.clear();
        self.set_status(status);
    }

    fn set_status(&self, status: ClientStatus) {
        let changed = self.shared.status.send_if_modified(|current| {
            if *current == status {
                false
            } else {
                *current = status.clone();
                true
            }
        });
        if changed {
            let _ = self.events.try_send(ClientEvent::Status(status));
        }
    }

    /// Decides what happens after a connection ends (sections 6.6 and 10).
    /// It returns the state and the wait, or `None` when the engine stops.
    fn after(&mut self, end: End) -> Option<(ClientStatus, Duration)> {
        let not_authorized = |config: &ClientConfig| {
            let pause = config.not_authorized_pause;
            Some((ClientStatus::NotAuthorized { retry_in: pause }, pause))
        };
        match end {
            End::UserClosed => {
                self.stop = Some(ClientStatus::Closed);
                None
            }
            End::Revoked => {
                self.stop = Some(ClientStatus::Revoked);
                None
            }
            // Only the own handshake decides "update needed". A 4426 close code comes from the
            // untrusted relay, so it falls through to the 4400 rule below (section 6.6).
            End::Incompatible => {
                self.stop = Some(ClientStatus::Incompatible);
                None
            }
            End::Unresponsive => {
                // The app asked for a check, so the engine connects again at once.
                let reason = End::Unresponsive.describe();
                Some((
                    ClientStatus::Disconnected {
                        reason,
                        retry_in: Duration::ZERO,
                    },
                    Duration::ZERO,
                ))
            }
            End::Closed(Some(code)) if code == CloseCode::Unauthorized.code() => {
                not_authorized(&self.config)
            }
            End::Connect(ConnectError::Upgrade { error, .. })
                if error.status() == UpgradeError::Unauthorized.status() =>
            {
                not_authorized(&self.config)
            }
            End::Connect(ConnectError::Upgrade {
                error: UpgradeError::HostOffline,
                ..
            }) => {
                let delay = random_between(
                    self.config.host_offline_retry_min,
                    self.config.host_offline_retry_max,
                );
                Some((ClientStatus::HostOffline { retry_in: delay }, delay))
            }
            End::Connect(ConnectError::Upgrade {
                error: UpgradeError::RateLimited,
                ..
            }) => {
                let delay = self.backoff.long_delay();
                let reason = "rate limited".to_owned();
                Some((
                    ClientStatus::Disconnected {
                        reason,
                        retry_in: delay,
                    },
                    delay,
                ))
            }
            End::Closed(Some(code)) if code == CloseCode::RateLimited.code() => {
                let delay = self.backoff.long_delay();
                let reason = "rate limited".to_owned();
                Some((
                    ClientStatus::Disconnected {
                        reason,
                        retry_in: delay,
                    },
                    delay,
                ))
            }
            other => {
                // 4413 is a bug in a peer. The device reconnects like for 4400, 4408, and 4500.
                // A 4426 without a failed overlap check of the own handshake gets this rule too.
                let delay = self.backoff.next_delay();
                let reason = other.describe();
                Some((
                    ClientStatus::Disconnected {
                        reason,
                        retry_in: delay,
                    },
                    delay,
                ))
            }
        }
    }

    /// Handles a command while no session is open.
    fn on_command_offline(&mut self, command: Option<Command>) {
        match command {
            None | Some(Command::Close) => self.stop = Some(ClientStatus::Closed),
            Some(Command::Call { reply, .. }) | Some(Command::UnsubscribePanes { reply }) => {
                let _ = reply.send(Err(ClientError::NotConnected));
            }
            Some(Command::SubscribePanes { reply }) => {
                let _ = reply.send(Err(ClientError::NotConnected));
            }
            Some(Command::SubscribeTerminal { reply, .. }) => {
                let _ = reply.send(Err(ClientError::NotConnected));
            }
            Some(Command::Unsubscribe { local }) => {
                self.subs.remove(&local);
            }
            Some(Command::Applied { .. }) => {}
            Some(Command::ReconnectNow) => {
                if self.wake != Some(Wake::Always) {
                    self.wake = Some(Wake::Soon);
                }
            }
            Some(Command::RetryUnauthorized) => self.wake = Some(Wake::Always),
        }
    }

    /// Runs an open connection until it ends.
    async fn serve(&mut self, conn: &mut Connection) -> End {
        let hello = HelloParams {
            app: self.config.app.clone(),
            platform: self.config.platform.clone(),
            caps: self.config.caps.clone(),
        };
        if let Err(end) = self
            .request(conn, method::HELLO, &hello, Pending::Hello)
            .await
        {
            return end;
        }
        let lifetime_end = conn.opened + self.config.connection_lifetime;
        loop {
            let wake = self.next_wake(conn, lifetime_end);
            let step = tokio::select! {
                frame = net::read_frame(&mut conn.reader) => {
                    conn.last_frame = Instant::now();
                    match frame {
                        Incoming::Binary(bytes) => self.on_noise(conn, &bytes).await,
                        Incoming::Alive => {
                            // A ping or a pong proves that the socket works now. Data frames do not:
                            // they can wait in a buffer from before a suspend.
                            conn.probe_deadline = None;
                            Ok(())
                        }
                        Incoming::Text(_) => Ok(()),
                        Incoming::Closed(code) => Err(End::Closed(code)),
                        Incoming::Failed(reason) => Err(End::Network(reason)),
                    }
                }
                command = self.commands.recv() => self.on_command(conn, command).await,
                _ = tokio::time::sleep_until(wake) => self.on_timers(conn, lifetime_end).await,
            };
            if let Err(end) = step {
                return end;
            }
        }
    }

    /// Fails the waiting requests and forgets the subscription ids of the connection.
    fn end_connection(&mut self, conn: &mut Connection, end: &End) {
        for (_, entry) in conn.pending.drain() {
            entry.kind.fail(ClientError::Disconnected);
        }
        for sub in self.subs.values_mut() {
            sub.host_sub = None;
            sub.acked = None;
            sub.last_ack = None;
        }
        *lock(&self.shared.hello) = None;
        match end {
            // The device closes its socket after a protocol error (section 7.7).
            End::Protocol(_) => conn.close(CloseCode::BadFrame.code(), "protocol error"),
            _ => conn.close(1000, "closed"),
        }
    }

    /// Returns the next time that a timer needs the engine.
    fn next_wake(&self, conn: &Connection, lifetime_end: Instant) -> Instant {
        let mut wake = lifetime_end.min(conn.last_frame + self.config.idle_timeout);
        if let Some(probe) = conn.probe_deadline {
            wake = wake.min(probe);
        }
        for entry in conn.pending.values() {
            wake = wake.min(entry.deadline);
        }
        let now = Instant::now();
        for sub in self.subs.values() {
            if sub.host_sub.is_none() {
                continue;
            }
            let progress = lock(&sub.progress);
            let Some(applied) = progress.applied else {
                continue;
            };
            if progress.snapshot {
                return now;
            }
            if sub.acked.is_none_or(|acked| acked < applied) {
                let due = sub.last_ack.map_or(now, |at| at + self.config.ack_interval);
                wake = wake.min(due);
            }
        }
        wake
    }

    async fn on_timers(&mut self, conn: &mut Connection, lifetime_end: Instant) -> Step {
        let now = Instant::now();
        if now >= lifetime_end {
            // Version 1 has no rekey, so a connection ends after 24 hours (section 7.7).
            return Err(End::Lifetime);
        }
        if now >= conn.last_frame + self.config.idle_timeout {
            return Err(End::Timeout("no frame from the relay"));
        }
        if conn.probe_deadline.is_some_and(|probe| now >= probe) {
            return Err(End::Unresponsive);
        }
        let expired: Vec<u32> = conn
            .pending
            .iter()
            .filter(|(_, entry)| entry.deadline <= now)
            .map(|(id, _)| *id)
            .collect();
        for id in expired {
            let Some(entry) = conn.pending.remove(&id) else {
                continue;
            };
            match entry.kind {
                Pending::Hello => return Err(End::Timeout("hello")),
                Pending::Subscribe { local: Some(_), .. } => {
                    return Err(End::Timeout("terminal.subscribe after a reconnect"));
                }
                kind => kind.fail(ClientError::Timeout),
            }
        }
        self.send_acks(conn, None).await
    }

    /// Sends the due `terminal.ack` events (section 8.8).
    async fn send_acks(&mut self, conn: &mut Connection, only: Option<u64>) -> Step {
        let now = Instant::now();
        let interval = self.config.ack_interval;
        let mut acks = Vec::new();
        for (local, sub) in self.subs.iter_mut() {
            if only.is_some_and(|only| only != *local) {
                continue;
            }
            let Some(host_sub) = sub.host_sub else {
                continue;
            };
            let mut progress = lock(&sub.progress);
            let Some(applied) = progress.applied else {
                continue;
            };
            let snapshot = progress.snapshot;
            let behind = sub.acked.is_none_or(|acked| acked < applied);
            let interval_passed = sub.last_ack.is_none_or(|at| now >= at + interval);
            if !(snapshot || (behind && interval_passed)) {
                continue;
            }
            progress.snapshot = false;
            sub.acked = Some(applied);
            sub.last_ack = Some(now);
            acks.push(TerminalAck {
                sub: host_sub,
                seq: applied,
            });
        }
        for ack in acks {
            let bytes = DeviceEvent::TerminalAck(ack)
                .encode()
                .map_err(|error| End::Protocol(error.to_string()))?;
            self.send_bytes(conn, &bytes).await?;
        }
        Ok(())
    }

    /// Seals and sends one application message. It returns false for a message above 8 MiB.
    async fn send_bytes(&self, conn: &mut Connection, bytes: &[u8]) -> Result<bool, End> {
        let sealed = match conn.transport.seal(bytes) {
            Ok(sealed) => sealed,
            Err(Error::TooLarge) => return Ok(false),
            Err(error) => return Err(End::Protocol(error.to_string())),
        };
        for noise_message in sealed {
            conn.out
                .send(Outgoing::Frame(WsMessage::Binary(noise_message.into())))
                .await
                .map_err(|_| End::Network("the socket closed".to_owned()))?;
        }
        Ok(true)
    }

    /// Sends a request. A request that is too large fails at once with [`ClientError::TooLarge`].
    async fn request<P: Serialize>(
        &self,
        conn: &mut Connection,
        method: &'static str,
        params: &P,
        pending: Pending,
    ) -> Step {
        let id = conn.next_request_id();
        let Ok(bytes) = rpc::encode_request(id, method, params) else {
            pending.fail(ClientError::Protocol("the params do not encode".to_owned()));
            return Ok(());
        };
        if !self.send_bytes(conn, &bytes).await? {
            pending.fail(ClientError::TooLarge);
            return Ok(());
        }
        let deadline = Instant::now() + self.config.request_timeout;
        conn.pending.insert(
            id.get(),
            PendingEntry {
                kind: pending,
                deadline,
            },
        );
        Ok(())
    }

    async fn on_command(&mut self, conn: &mut Connection, command: Option<Command>) -> Step {
        match command {
            None | Some(Command::Close) => Err(End::UserClosed),
            Some(Command::Call {
                method,
                params,
                reply,
            }) => {
                self.request(conn, method, &params, Pending::Call(reply))
                    .await
            }
            Some(Command::SubscribePanes { reply }) => {
                self.panes_subscribed = true;
                self.request(
                    conn,
                    method::PANES_SUBSCRIBE,
                    &Empty {},
                    Pending::PanesSubscribe(Some(reply)),
                )
                .await
            }
            Some(Command::UnsubscribePanes { reply }) => {
                self.panes_subscribed = false;
                self.request(
                    conn,
                    method::PANES_UNSUBSCRIBE,
                    &Empty {},
                    Pending::Call(reply),
                )
                .await
            }
            Some(Command::SubscribeTerminal { session, reply }) => {
                let params = TerminalSubscribeParams {
                    session: session.clone(),
                    from_seq: None,
                };
                let pending = Pending::Subscribe {
                    session,
                    local: None,
                    reply: Some(reply),
                };
                self.request(conn, method::TERMINAL_SUBSCRIBE, &params, pending)
                    .await
            }
            Some(Command::Unsubscribe { local }) => {
                let Some(sub) = self.subs.remove(&local) else {
                    return Ok(());
                };
                match sub.host_sub {
                    Some(host_sub) => {
                        conn.host_subs.remove(&host_sub);
                        self.send_unsubscribe(conn, host_sub).await
                    }
                    None => Ok(()),
                }
            }
            Some(Command::Applied { local }) => self.send_acks(conn, Some(local)).await,
            Some(Command::ReconnectNow | Command::RetryUnauthorized) => self.probe(conn).await,
        }
    }

    /// Checks a connection that looks open: it sends a WebSocket ping.
    /// When no ping or pong arrives in time, the connection ends with [`End::Unresponsive`].
    async fn probe(&self, conn: &mut Connection) -> Step {
        if conn.probe_deadline.is_some() {
            return Ok(());
        }
        conn.out
            .send(Outgoing::Frame(WsMessage::Ping(Bytes::new())))
            .await
            .map_err(|_| End::Network("the socket closed".to_owned()))?;
        conn.probe_deadline = Some(Instant::now() + self.config.probe_timeout);
        Ok(())
    }

    async fn send_unsubscribe(&self, conn: &mut Connection, host_sub: u32) -> Step {
        let params = TerminalUnsubscribeParams { sub: host_sub };
        self.request(conn, method::TERMINAL_UNSUBSCRIBE, &params, Pending::Ignore)
            .await
    }

    async fn on_noise(&mut self, conn: &mut Connection, bytes: &[u8]) -> Step {
        let message = match conn.transport.open(bytes) {
            Ok(Some(message)) => message,
            Ok(None) => return Ok(()),
            Err(error) => return Err(End::Protocol(error.to_string())),
        };
        match Message::decode(&message) {
            Ok(Message::Response(response)) => {
                let Some(entry) = conn.pending.remove(&response.id.get()) else {
                    return Ok(());
                };
                self.on_response(conn, entry.kind, response).await
            }
            Ok(Message::Event(event)) => self.on_event(conn, event),
            Ok(Message::Request(request)) => {
                // In version 1 only the device sends requests.
                let bytes = rpc::encode_error(request.id, &RpcError::unknown_method())
                    .map_err(|error| End::Protocol(error.to_string()))?;
                self.send_bytes(conn, &bytes).await.map(|_| ())
            }
            Err(error) => Err(End::Protocol(error.to_string())),
        }
    }

    async fn on_response(
        &mut self,
        conn: &mut Connection,
        pending: Pending,
        response: rpc::Response,
    ) -> Step {
        let protocol = |error: Error| End::Protocol(error.to_string());
        match pending {
            Pending::Hello => match response.decode_result::<rpc::Hello>().map_err(protocol)? {
                Ok(result) => {
                    *lock(&self.shared.hello) = Some(result.clone());
                    self.set_status(ClientStatus::Connected(result));
                    self.restore(conn).await
                }
                Err(error) => Err(End::Protocol(format!("hello failed ({})", error.code))),
            },
            Pending::Call(reply) => {
                let _ = reply.send(response.outcome.map_err(ClientError::Rpc));
                Ok(())
            }
            Pending::PanesSubscribe(reply) => {
                let result = response
                    .decode_result::<rpc::PanesSubscribe>()
                    .map_err(protocol)?;
                match (result, reply) {
                    (Ok(list), Some(reply)) => {
                        let _ = reply.send(Ok(list));
                    }
                    (Ok(list), None) => {
                        let _ = self.events.try_send(ClientEvent::PanesChanged(list));
                    }
                    (Err(error), Some(reply)) => {
                        self.panes_subscribed = false;
                        let _ = reply.send(Err(ClientError::Rpc(error)));
                    }
                    (Err(_), None) => self.panes_subscribed = false,
                }
                Ok(())
            }
            Pending::Subscribe {
                session,
                local,
                reply,
            } => {
                let result = response
                    .decode_result::<rpc::TerminalSubscribe>()
                    .map_err(protocol)?;
                match (result, local) {
                    (Ok(result), Some(local)) => match self.subs.get_mut(&local) {
                        Some(sub) => {
                            sub.host_sub = Some(result.sub);
                            conn.host_subs.insert(result.sub, local);
                            // The PTY can have changed size while the device was away.
                            // The new size goes out before the replayed data (section 8.8).
                            let size = (result.cols, result.rows);
                            if sub.size != size {
                                sub.size = size;
                                let _ = sub.updates.send(TerminalUpdate::Resized {
                                    cols: result.cols,
                                    rows: result.rows,
                                });
                            }
                            Ok(())
                        }
                        // The app dropped the subscription while it was restored.
                        None => self.send_unsubscribe(conn, result.sub).await,
                    },
                    (Ok(result), None) => match reply {
                        Some(reply) if !reply.is_closed() => {
                            let subscription = self.add_subscription(conn, session, &result);
                            // If the caller is gone, the dropped value ends the subscription.
                            let _ = reply.send(Ok(subscription));
                            Ok(())
                        }
                        _ => self.send_unsubscribe(conn, result.sub).await,
                    },
                    (Err(error), Some(local)) => {
                        // The pane is gone, or the host refused it after a reconnect.
                        if let Some(sub) = self.subs.remove(&local) {
                            let reason = error.code.to_string();
                            let _ = sub.updates.send(TerminalUpdate::Closed { reason });
                        }
                        Ok(())
                    }
                    (Err(error), None) => {
                        if let Some(reply) = reply {
                            let _ = reply.send(Err(ClientError::Rpc(error)));
                        }
                        Ok(())
                    }
                }
            }
            Pending::Ignore => Ok(()),
        }
    }

    fn add_subscription(
        &mut self,
        conn: &mut Connection,
        session: String,
        result: &TerminalSubscribeResult,
    ) -> TerminalSubscription {
        let local = self.next_local;
        self.next_local += 1;
        let (updates, receiver) = mpsc::unbounded_channel();
        let progress = Arc::new(Mutex::new(Progress::default()));
        self.subs.insert(
            local,
            LocalSub {
                session: session.clone(),
                updates,
                progress: progress.clone(),
                host_sub: Some(result.sub),
                received: None,
                acked: None,
                last_ack: None,
                size: (result.cols, result.rows),
            },
        );
        conn.host_subs.insert(result.sub, local);
        TerminalSubscription {
            local,
            session,
            cols: result.cols,
            rows: result.rows,
            seq: result.seq,
            updates: receiver,
            progress,
            commands: self.command_sender.clone(),
        }
    }

    /// Subscribes again after a reconnect: the pane list, then each terminal with `fromSeq`.
    async fn restore(&mut self, conn: &mut Connection) -> Step {
        if self.panes_subscribed {
            self.request(
                conn,
                method::PANES_SUBSCRIBE,
                &Empty {},
                Pending::PanesSubscribe(None),
            )
            .await?;
        }
        let restore: Vec<(u64, String, Option<u64>)> = self
            .subs
            .iter()
            .map(|(local, sub)| (*local, sub.session.clone(), sub.received))
            .collect();
        for (local, session, received) in restore {
            let params = TerminalSubscribeParams {
                session: session.clone(),
                from_seq: received,
            };
            let pending = Pending::Subscribe {
                session,
                local: Some(local),
                reply: None,
            };
            self.request(conn, method::TERMINAL_SUBSCRIBE, &params, pending)
                .await?;
        }
        Ok(())
    }

    fn on_event(&mut self, conn: &mut Connection, event: rpc::Event) -> Step {
        let event = match HostEvent::parse(&event) {
            Ok(Some(event)) => event,
            // A receiver ignores an unknown event (section 8.2).
            Ok(None) => return Ok(()),
            Err(error) => return Err(End::Protocol(error.to_string())),
        };
        match event {
            HostEvent::PanesChanged(list) => {
                if self.panes_subscribed {
                    let _ = self.events.try_send(ClientEvent::PanesChanged(list));
                }
            }
            HostEvent::TerminalSnapshot(snapshot) => {
                let size = Some((snapshot.cols, snapshot.rows));
                let update = TerminalUpdate::Snapshot {
                    seq: snapshot.seq,
                    cols: snapshot.cols,
                    rows: snapshot.rows,
                    data: snapshot.data,
                };
                self.deliver(conn, snapshot.sub, Some(snapshot.seq), size, update);
            }
            HostEvent::TerminalData(data) => {
                let update = TerminalUpdate::Data {
                    seq: data.seq,
                    data: data.data,
                };
                self.deliver(conn, data.sub, Some(data.seq), None, update);
            }
            HostEvent::TerminalResized(resized) => {
                let size = Some((resized.cols, resized.rows));
                let update = TerminalUpdate::Resized {
                    cols: resized.cols,
                    rows: resized.rows,
                };
                self.deliver(conn, resized.sub, None, size, update);
            }
            HostEvent::TerminalExit(exit) => {
                let update = TerminalUpdate::Exit {
                    code: exit.code,
                    signal: exit.signal,
                };
                self.deliver(conn, exit.sub, None, None, update);
            }
            HostEvent::TerminalClosed(closed) => {
                let update = TerminalUpdate::Closed {
                    reason: closed.reason,
                };
                self.deliver(conn, closed.sub, None, None, update);
                if let Some(local) = conn.host_subs.remove(&closed.sub) {
                    self.subs.remove(&local);
                }
            }
            // Only this encrypted event makes the app show "Access revoked" (section 10).
            HostEvent::DeviceRevoked => return Err(End::Revoked),
        }
        Ok(())
    }

    /// Puts an update into the channel of a subscription.
    /// `seq` and `size` are the sequence number and the PTY size that the update carries.
    fn deliver(
        &mut self,
        conn: &Connection,
        host_sub: u32,
        seq: Option<u64>,
        size: Option<(u16, u16)>,
        update: TerminalUpdate,
    ) {
        let Some(local) = conn.host_subs.get(&host_sub) else {
            return;
        };
        let Some(sub) = self.subs.get_mut(local) else {
            return;
        };
        if let Some(seq) = seq {
            sub.received = Some(seq);
        }
        if let Some(size) = size {
            sub.size = size;
        }
        // A closed channel means that the app dropped the subscription.
        // Its drop already queued the unsubscribe command.
        let _ = sub.updates.send(update);
    }
}

/// Opens the device socket and runs the session handshake (section 7.4).
async fn connect_session(
    paired: Arc<PairedHost>,
    config: Arc<ClientConfig>,
) -> Result<Connection, End> {
    let bearer = bearer_header_value(paired.keys().relay_token().as_bytes());
    let header = net::secret_header(&bearer)
        .ok_or_else(|| End::Protocol("invalid bearer header".to_owned()))?;
    let url = device_socket_url(paired.relay_url(), paired.host_id());
    let socket = net::connect(&url, vec![(AUTHORIZATION, header)], config.connect_timeout)
        .await
        .map_err(End::Connect)?;
    let (mut reader, out, writer) = net::spawn_writer(socket, config.ping_interval);
    let payload = HandshakePayload {
        v: config.max_version,
        min: config.min_version,
        app: config.app.clone(),
    };
    let (initiator, message_1) = SessionInitiator::start(
        paired.host_id(),
        paired.host_noise_public(),
        paired.keys(),
        payload,
    )
    .map_err(|error| End::Protocol(error.to_string()))?;
    out.send(Outgoing::Frame(WsMessage::Binary(message_1.into())))
        .await
        .map_err(|_| End::Network("the socket closed".to_owned()))?;
    let deadline = Instant::now() + config.handshake_timeout;
    let message_2 = loop {
        let frame = tokio::time::timeout_at(deadline, net::read_frame(&mut reader))
            .await
            .map_err(|_| End::Timeout("handshake message 2"))?;
        match frame {
            Incoming::Binary(bytes) => break bytes,
            Incoming::Text(_) | Incoming::Alive => continue,
            Incoming::Closed(code) => return Err(End::Closed(code)),
            Incoming::Failed(reason) => return Err(End::Network(reason)),
        }
    };
    let established = initiator
        .read_message_2(&message_2)
        .map_err(|error| End::Protocol(error.to_string()))?;
    if !established.versions_overlap() {
        // The host closes the connection with 4426 after message 2 (section 7.5).
        let _ = out.try_send(Outgoing::Close(1000, "incompatible"));
        return Err(End::Incompatible);
    }
    let now = Instant::now();
    Ok(Connection {
        reader,
        out,
        writer: Some(writer),
        transport: established.transport,
        pending: HashMap::new(),
        next_id: 0,
        host_subs: HashMap::new(),
        opened: now,
        last_frame: now,
        probe_deadline: None,
    })
}
