//! One device connection: the handshake, then a pairing or a session (sections 7.3, 7.4, and 8).
//!
//! The task owns the [`Transport`] of the connection, so it seals every outgoing message
//! and opens every incoming Noise message in order.

use std::collections::BTreeMap;
use std::sync::Arc;

use tokio::sync::broadcast::error::RecvError;
use tokio::sync::{mpsc, oneshot, watch};
use tokio::time::Instant;
use tokio_tungstenite::tungstenite::{Bytes, Message as WsMessage};
use zeroize::Zeroizing;

use super::engine::{blocking, Shared};
use super::paste::encode_paste;
use super::stream::{Push, Subscription};
use super::{NewDevice, OutputEvent, PairRequestInfo, Snapshot, StoredDevice, WriteError};
use crate::framing::Transport;
use crate::ids::{ConnId, DeviceId, InviteId, RequestId};
use crate::net::Outgoing;
use crate::noise::{
    Established, HandshakePayload, IncomingHandshake, PairingResponder, SessionResponder,
};
use crate::pairing::Psk;
use crate::relay_wire::{encode_data, CloseCode, ConnPeer, HostToRelay, MAX_PAIRED_DEVICES};
use crate::rpc::{
    self, caps, DeviceEvent, Empty, ErrorCode, HelloParams, HelloResult, HostEvent, Message,
    PairRequestParams, PairRequestResult, PaneList, PaneState, Request, RpcError, SessionRequest,
    TerminalAck, TerminalClosed, TerminalExit, TerminalInputParams, TerminalPasteParams,
    TerminalResized, TerminalSnapshot, TerminalSubscribeParams, TerminalSubscribeResult,
    TerminalUnsubscribeParams, MAX_TERMINAL_SUBSCRIPTIONS,
};
use crate::{unix_time_ms, Error};

/// The largest snapshot that the host sends. It stays far below the 8 MiB message limit.
const MAX_SNAPSHOT_BYTES: usize = 4 * 1024 * 1024;

/// The largest write to the PTY. The host splits a larger paste (section 8.8).
const MAX_WRITE_BYTES: usize = 32 * 1024;

/// A command from the engine to a connection task.
#[derive(Debug)]
pub(crate) enum ConnCommand {
    /// Send `device.revoked`, then answer `done` and stop. The engine closes the connection.
    Revoke {
        /// Gets a value when the event is queued.
        done: oneshot::Sender<()>,
    },
    /// The pairing offer ended while the connection waited for the user.
    OfferEnded,
}

/// The offer of a pairing connection.
pub(crate) struct PairingStart {
    pub(crate) offer_id: u64,
    pub(crate) invite: InviteId,
    pub(crate) psk: Psk,
}

/// The parts of one connection task.
pub(crate) struct ConnTask {
    pub(crate) shared: Arc<Shared>,
    pub(crate) conn: ConnId,
    pub(crate) peer: ConnPeer,
    pub(crate) generation: u64,
    pub(crate) out: mpsc::Sender<Outgoing>,
    pub(crate) inbox: mpsc::Receiver<Bytes>,
    pub(crate) commands: mpsc::UnboundedReceiver<ConnCommand>,
    pub(crate) pairing: Option<PairingStart>,
}

/// How a connection ends.
enum Ending {
    /// The device socket closed, or the engine removed the connection.
    DeviceLeft,
    /// The host closes the connection with this code.
    Close(CloseCode),
    /// A revoke ended the session. The revoke closes the connection.
    Revoked,
    /// `device.unpair` ended the session. The rest of the revoke runs after the task.
    Unpaired(DeviceId),
    /// The host socket is gone.
    LinkDown,
}

type Step = Result<(), Ending>;

/// Runs a connection until it ends.
pub(crate) async fn run(mut task: ConnTask) {
    let ending = task.drive().await;
    let shared = task.shared.clone();
    shared.forget_conn(task.conn, task.generation);
    match ending {
        Ending::DeviceLeft | Ending::Revoked | Ending::LinkDown => {}
        Ending::Close(code) => {
            if let Ok(text) = HostToRelay::conn_close(task.conn, code).to_json() {
                let _ = task.out.send(Outgoing::Frame(WsMessage::text(text))).await;
            }
        }
        Ending::Unpaired(device) => {
            let conn = task.conn;
            tokio::spawn(async move {
                let _ = shared.revoke_device(&device, false, Some(conn)).await;
            });
        }
    }
}

/// The state of an open session.
struct Session {
    transport: Transport,
    device: StoredDevice,
    hello_done: bool,
    panes_subscribed: bool,
    panes_rev: u64,
    subs: BTreeMap<u32, Subscription>,
    next_sub: u32,
}

impl Session {
    fn next_sub_id(&mut self) -> u32 {
        loop {
            self.next_sub = self.next_sub.wrapping_add(1).max(1);
            if !self.subs.contains_key(&self.next_sub) {
                return self.next_sub;
            }
        }
    }

    fn next_flush(&self) -> Option<Instant> {
        self.subs.values().filter_map(Subscription::deadline).min()
    }

    fn subs_of(&self, session: &str) -> Vec<u32> {
        self.subs
            .iter()
            .filter(|(_, sub)| sub.session == session)
            .map(|(id, _)| *id)
            .collect()
    }
}

impl ConnTask {
    async fn drive(&mut self) -> Ending {
        if let ConnPeer::Invite(invite) = &self.peer {
            if self.pairing.is_none() {
                // The relay admitted an invite that has no offer here. Remove it there too.
                self.shared.delete_stale_invite(invite.clone());
                return Ending::Close(CloseCode::Unauthorized);
            }
        }
        let message_1 = match self.wait_message_1().await {
            Ok(message) => message,
            Err(ending) => return ending,
        };
        match self.pairing.take() {
            Some(start) => self.pair(start, &message_1).await,
            None => self.session(&message_1).await,
        }
    }

    /// Waits for handshake message 1.
    async fn wait_message_1(&mut self) -> Result<Bytes, Ending> {
        let deadline = Instant::now() + self.shared.config.handshake_timeout;
        loop {
            tokio::select! {
                message = self.inbox.recv() => return message.ok_or(Ending::DeviceLeft),
                command = self.commands.recv() => match command {
                    Some(ConnCommand::OfferEnded) => return Err(Ending::Close(CloseCode::Unauthorized)),
                    Some(ConnCommand::Revoke { done }) => {
                        let _ = done.send(());
                    }
                    None => return Err(Ending::DeviceLeft),
                },
                _ = tokio::time::sleep_until(deadline) => {
                    return Err(Ending::Close(CloseCode::HostGone));
                }
            }
        }
    }

    /// The payload P2 of the host (section 7.5).
    fn p2(&self) -> HandshakePayload {
        HandshakePayload {
            v: self.shared.config.max_version,
            min: self.shared.config.min_version,
            app: self.shared.backend.host_info().app,
        }
    }

    /// Writes message 2. It returns `None` when the connection must end.
    async fn accept(&mut self, incoming: IncomingHandshake) -> Result<Established, Ending> {
        let (message_2, established) = incoming
            .accept(self.p2())
            .map_err(|_| Ending::Close(CloseCode::HostGone))?;
        self.send_noise(&message_2).await?;
        if !established.versions_overlap() {
            // The host still sends message 2, then closes (section 7.5).
            return Err(Ending::Close(CloseCode::Incompatible));
        }
        Ok(established)
    }

    /// Sends one Noise message in a DATA frame.
    async fn send_noise(&self, noise_message: &[u8]) -> Step {
        let frame = encode_data(self.conn, noise_message)
            .map_err(|_| Ending::Close(CloseCode::HostGone))?;
        self.out
            .send(Outgoing::Frame(WsMessage::Binary(frame.into())))
            .await
            .map_err(|_| Ending::LinkDown)
    }

    /// Seals an application message and sends its Noise messages in order.
    /// A message above 8 MiB is not sent and returns `Ok(false)`.
    async fn send_message(&self, transport: &mut Transport, bytes: &[u8]) -> Result<bool, Ending> {
        let sealed = match transport.seal(bytes) {
            Ok(sealed) => sealed,
            Err(Error::TooLarge) => return Ok(false),
            Err(_) => return Err(Ending::Close(CloseCode::BadFrame)),
        };
        for noise_message in sealed {
            self.send_noise(&noise_message).await?;
        }
        Ok(true)
    }

    async fn respond<V: serde::Serialize>(
        &self,
        transport: &mut Transport,
        id: RequestId,
        value: &V,
    ) -> Step {
        let bytes =
            rpc::encode_response(id, value).map_err(|_| Ending::Close(CloseCode::HostGone))?;
        if !self.send_message(transport, &bytes).await? {
            let error = RpcError::too_large("the response is larger than 8 MiB");
            return self.respond_error(transport, id, &error).await;
        }
        Ok(())
    }

    async fn respond_error(
        &self,
        transport: &mut Transport,
        id: RequestId,
        error: &RpcError,
    ) -> Step {
        let bytes = rpc::encode_error(id, error).map_err(|_| Ending::Close(CloseCode::HostGone))?;
        self.send_message(transport, &bytes).await.map(|_| ())
    }

    async fn send_event(&self, transport: &mut Transport, event: &HostEvent) -> Step {
        let bytes = event
            .encode()
            .map_err(|_| Ending::Close(CloseCode::HostGone))?;
        // An event above 8 MiB cannot go out. Terminal events are far below that limit.
        self.send_message(transport, &bytes).await.map(|_| ())
    }

    // Pairing (section 7.3).

    async fn pair(&mut self, start: PairingStart, message_1: &[u8]) -> Ending {
        let identity = self.shared.config.identity.clone();
        let incoming = PairingResponder::new(&identity, &start.psk)
            .and_then(|responder| responder.read_message_1(message_1));
        // The PSK is not needed after message 1.
        drop(start.psk);
        let incoming = match incoming {
            Ok(incoming) => incoming,
            Err(_) => return Ending::Close(CloseCode::Unauthorized),
        };
        let established = match self.accept(incoming).await {
            Ok(established) => established,
            Err(ending) => return ending,
        };
        let device_key = established.remote_static;
        let code = established.confirmation_code();
        let mut transport = established.transport;

        let mut request = match self.wait_pair_request(&mut transport).await {
            Ok(request) => request,
            Err(ending) => return ending,
        };
        // The first valid transport message ends the handshake (section 7.4, step 8).
        self.shared.end_handshake(self.conn, self.generation);
        let parsed = PairRequestParams::parse(&request);
        // The params hold the relay token. Only the zeroizing copy in `params` stays.
        request.wipe_params();
        let params = match parsed {
            Ok(params) => params,
            Err(error) => {
                let _ = self.respond_error(&mut transport, request.id, &error).await;
                return Ending::Close(CloseCode::BadFrame);
            }
        };

        // A full registry cannot take the device, so the user is not asked.
        let store = self.shared.store.clone();
        let active = blocking(move || store.list()).await.map(|listed| {
            listed.map(|devices| devices.iter().filter(|device| !device.is_revoked()).count())
        });
        match active {
            Ok(Ok(count)) if count < MAX_PAIRED_DEVICES => {}
            Ok(Ok(_)) => {
                let error = RpcError::new(ErrorCode::Unavailable, "the host has 32 paired devices");
                return self.fail_pairing(&mut transport, request.id, error).await;
            }
            _ => {
                let error = RpcError::new(ErrorCode::Internal, "the device registry failed");
                return self.fail_pairing(&mut transport, request.id, error).await;
            }
        }

        if let Err(code) = self.shared.begin_approval(start.offer_id, self.conn) {
            let error = match code {
                ErrorCode::Busy => RpcError::new(code, "another device waits for approval"),
                _ => RpcError::new(code, "the pairing offer ended"),
            };
            return self.fail_pairing(&mut transport, request.id, error).await;
        }
        let info = PairRequestInfo {
            name: params.name.clone(),
            platform: params.platform.clone(),
            model: params.model.clone(),
            app: params.app.clone(),
            code,
            invite: start.invite.clone(),
        };
        let allowed = match self.wait_approval(&mut transport, info).await {
            Ok(allowed) => allowed,
            Err((ending, error)) => {
                if let Some(error) = error {
                    let _ = self.respond_error(&mut transport, request.id, &error).await;
                    self.shared.end_offer(Some(start.offer_id)).await;
                } else {
                    self.shared
                        .release_approval(start.offer_id, self.conn)
                        .await;
                }
                return ending;
            }
        };
        if !allowed {
            let error = RpcError::new(ErrorCode::Denied, "the user denied the device");
            self.shared.end_offer(Some(start.offer_id)).await;
            return self.fail_pairing(&mut transport, request.id, error).await;
        }

        // Allowed: store the device, register its token hash, delete the invite, answer.
        let id = match DeviceId::generate() {
            Ok(id) => id,
            Err(_) => {
                self.shared.end_offer(Some(start.offer_id)).await;
                let error = RpcError::new(ErrorCode::Internal, "the host could not make an id");
                return self.fail_pairing(&mut transport, request.id, error).await;
            }
        };
        let new_device = NewDevice {
            id: id.clone(),
            name: params.name,
            platform: params.platform,
            model: params.model,
            noise_public: device_key,
            token_hash: params.token.hash(),
            created_at_ms: unix_time_ms(),
        };
        // The commit runs in its own task, so the end of this task cannot stop it halfway.
        // `keep` is the guard: if this task ends before it answers, the sender drops,
        // and the commit task revokes the device again.
        let (result_sender, result) = oneshot::channel();
        let (keep, keep_receiver) = oneshot::channel();
        tokio::spawn(commit_device(
            self.shared.clone(),
            new_device,
            result_sender,
            keep_receiver,
        ));
        let committed = result.await.unwrap_or_else(|_| {
            Err(RpcError::new(
                ErrorCode::Internal,
                "the device registry failed",
            ))
        });
        self.shared.end_offer(Some(start.offer_id)).await;
        if let Err(error) = committed {
            return self.fail_pairing(&mut transport, request.id, error).await;
        }
        let result = PairRequestResult {
            device: id,
            host: self.shared.backend.host_info().name,
        };
        if let Err(ending) = self.respond(&mut transport, request.id, &result).await {
            return ending;
        }
        // The answer is on its way. The device stays.
        let _ = keep.send(());
        // The device closes the socket. After the delay, the host closes it with 4410.
        let delay = tokio::time::sleep(self.shared.config.pairing_close_delay);
        tokio::pin!(delay);
        loop {
            tokio::select! {
                message = self.inbox.recv() => if message.is_none() {
                    return Ending::DeviceLeft;
                },
                command = self.commands.recv() => if command.is_none() {
                    return Ending::DeviceLeft;
                },
                _ = &mut delay => return Ending::Close(CloseCode::PairingDone),
            }
        }
    }

    /// Answers a failed pairing and closes the connection with `4401`.
    async fn fail_pairing(
        &self,
        transport: &mut Transport,
        id: RequestId,
        error: RpcError,
    ) -> Ending {
        match self.respond_error(transport, id, &error).await {
            Ok(()) => Ending::Close(CloseCode::Unauthorized),
            Err(ending) => ending,
        }
    }

    /// Waits for `pair.request`, the only method of a pairing session (section 8.3).
    async fn wait_pair_request(&mut self, transport: &mut Transport) -> Result<Request, Ending> {
        let deadline = Instant::now() + self.shared.config.handshake_timeout;
        loop {
            let bytes = tokio::select! {
                message = self.inbox.recv() => message.ok_or(Ending::DeviceLeft)?,
                command = self.commands.recv() => match command {
                    Some(ConnCommand::OfferEnded) => return Err(Ending::Close(CloseCode::Unauthorized)),
                    Some(ConnCommand::Revoke { done }) => {
                        let _ = done.send(());
                        continue;
                    }
                    None => return Err(Ending::DeviceLeft),
                },
                _ = tokio::time::sleep_until(deadline) => {
                    return Err(Ending::Close(CloseCode::HostGone));
                }
            };
            match open_message(transport, &bytes)? {
                Some(Message::Request(request)) if request.method == rpc::method::PAIR_REQUEST => {
                    return Ok(request);
                }
                Some(Message::Request(request)) => {
                    self.respond_error(transport, request.id, &RpcError::unknown_method())
                        .await?;
                }
                Some(_) | None => {}
            }
        }
    }

    /// Asks the user and waits for the answer, the timeout, the device, or the end of the offer.
    ///
    /// The error holds the ending and the error response to send, if any.
    async fn wait_approval(
        &mut self,
        transport: &mut Transport,
        info: PairRequestInfo,
    ) -> Result<bool, (Ending, Option<RpcError>)> {
        let approver = self.shared.approver.clone();
        let approval = approver.approve(info);
        tokio::pin!(approval);
        let timeout = tokio::time::sleep(self.shared.config.approval_timeout);
        tokio::pin!(timeout);
        loop {
            tokio::select! {
                allowed = &mut approval => return Ok(allowed),
                _ = &mut timeout => {
                    let error = RpcError::new(ErrorCode::Timeout, "no answer in time");
                    return Err((Ending::Close(CloseCode::Unauthorized), Some(error)));
                }
                command = self.commands.recv() => match command {
                    Some(ConnCommand::OfferEnded) => {
                        let error = RpcError::new(ErrorCode::Expired, "the pairing offer ended");
                        return Err((Ending::Close(CloseCode::Unauthorized), Some(error)));
                    }
                    Some(ConnCommand::Revoke { done }) => {
                        let _ = done.send(());
                    }
                    None => return Err((Ending::DeviceLeft, None)),
                },
                message = self.inbox.recv() => {
                    let Some(bytes) = message else {
                        return Err((Ending::DeviceLeft, None));
                    };
                    match open_message(transport, &bytes) {
                        Ok(Some(Message::Request(request))) => {
                            let error = RpcError::new(ErrorCode::Busy, "the pairing waits for the user");
                            if let Err(ending) = self.respond_error(transport, request.id, &error).await {
                                return Err((ending, None));
                            }
                        }
                        Ok(_) => {}
                        Err(ending) => return Err((ending, None)),
                    }
                }
            }
        }
    }

    // Sessions (sections 7.4 and 8).

    async fn session(&mut self, message_1: &[u8]) -> Ending {
        let ConnPeer::Device(device_id) = self.peer.clone() else {
            return Ending::Close(CloseCode::Unauthorized);
        };
        let identity = self.shared.config.identity.clone();
        let incoming = match SessionResponder::new(&identity)
            .and_then(|responder| responder.read_message_1(message_1))
        {
            Ok(incoming) => incoming,
            Err(_) => return Ending::Close(CloseCode::Unauthorized),
        };
        // The key must belong to a paired device that is not revoked, and that device
        // must be the device of `conn_open` (section 7.4, step 4).
        let key = *incoming.remote_static();
        let store = self.shared.store.clone();
        let device = match blocking(move || store.find_by_noise_key(&key)).await {
            Ok(Ok(Some(device))) if !device.is_revoked() && device.id == device_id => device,
            _ => return Ending::Close(CloseCode::Unauthorized),
        };
        let established = match self.accept(incoming).await {
            Ok(established) => established,
            Err(ending) => return ending,
        };
        // An attacker can replay message 1 and get this far, so nothing is marked yet.
        // `hello`, the first valid transport message, marks the device (section 7.4, step 7).
        let mut session = Session {
            transport: established.transport,
            device,
            hello_done: false,
            panes_subscribed: false,
            panes_rev: 0,
            subs: BTreeMap::new(),
            next_sub: 0,
        };
        self.run_session(&mut session).await
    }

    async fn run_session(&mut self, s: &mut Session) -> Ending {
        let started = Instant::now();
        let lifetime_end = started + self.shared.config.connection_lifetime;
        let hello_end = started + self.shared.config.hello_timeout;
        let mut output = self.shared.backend.output();
        let mut output_open = true;
        let mut panes = self.shared.panes.subscribe();
        let mut panes_open = true;
        loop {
            let flush_at = s.next_flush();
            // `biased`: the branches are polled in order. A revoke wins over input that
            // already waits in the inbox, and the timers cannot starve behind the inbox.
            let step = tokio::select! {
                biased;
                command = self.commands.recv() => match command {
                    Some(ConnCommand::Revoke { done }) => {
                        let sent = self.send_event(&mut s.transport, &HostEvent::DeviceRevoked).await;
                        let _ = done.send(());
                        Err(sent.err().unwrap_or(Ending::Revoked))
                    }
                    Some(ConnCommand::OfferEnded) => Ok(()),
                    None => Err(Ending::DeviceLeft),
                },
                _ = tokio::time::sleep_until(hello_end), if !s.hello_done => {
                    // The device did not prove itself. A replayed message 1 ends here.
                    Err(Ending::Close(CloseCode::BadFrame))
                }
                _ = tokio::time::sleep_until(lifetime_end) => {
                    // Version 1 has no rekey, so a connection ends after 24 hours (section 7.7).
                    Err(Ending::Close(CloseCode::HostGone))
                }
                message = self.inbox.recv() => match message {
                    Some(bytes) => self.on_noise(s, &bytes).await,
                    None => Err(Ending::DeviceLeft),
                },
                event = output.recv(), if output_open => match event {
                    Err(RecvError::Closed) => {
                        output_open = false;
                        Ok(())
                    }
                    event => self.on_output(s, event).await,
                },
                changed = panes.changed(), if panes_open && s.panes_subscribed => match changed {
                    Ok(()) => self.send_panes_changed(s, &mut panes).await,
                    Err(_) => {
                        panes_open = false;
                        Ok(())
                    }
                },
                _ = tokio::time::sleep_until(flush_at.unwrap_or(lifetime_end)), if flush_at.is_some() => {
                    self.flush_due(s).await
                }
            };
            if let Err(ending) = step {
                return ending;
            }
        }
    }

    async fn on_noise(&mut self, s: &mut Session, bytes: &[u8]) -> Step {
        match open_message(&mut s.transport, bytes)? {
            None => Ok(()),
            Some(Message::Request(request)) => self.on_request(s, request).await,
            Some(Message::Event(event)) => match DeviceEvent::parse(&event) {
                Ok(Some(DeviceEvent::TerminalAck(ack))) => self.on_ack(s, ack).await,
                Ok(None) => Ok(()),
                Err(_) => Err(Ending::Close(CloseCode::BadFrame)),
            },
            // In version 1 the host sends no requests, so a response has no use.
            Some(Message::Response(_)) => Ok(()),
        }
    }

    async fn on_request(&mut self, s: &mut Session, request: Request) -> Step {
        let id = request.id;
        let parsed = match SessionRequest::parse(&request) {
            Ok(parsed) => parsed,
            Err(error) => return self.respond_error(&mut s.transport, id, &error).await,
        };
        if !s.hello_done && !matches!(parsed, SessionRequest::Hello(_)) {
            let error = RpcError::bad_request("send hello first");
            return self.respond_error(&mut s.transport, id, &error).await;
        }
        match parsed {
            SessionRequest::Hello(params) => self.hello(s, id, params).await,
            SessionRequest::PanesList => {
                let list = self.shared.refresh_panes().await;
                self.respond(&mut s.transport, id, &*list).await
            }
            SessionRequest::PanesSubscribe => {
                let list = self.shared.refresh_panes().await;
                s.panes_subscribed = true;
                s.panes_rev = list.rev;
                self.respond(&mut s.transport, id, &*list).await
            }
            SessionRequest::PanesUnsubscribe => {
                s.panes_subscribed = false;
                self.respond(&mut s.transport, id, &Empty {}).await
            }
            SessionRequest::TerminalSubscribe(params) => self.subscribe(s, id, params).await,
            SessionRequest::TerminalUnsubscribe(TerminalUnsubscribeParams { sub }) => {
                if s.subs.remove(&sub).is_some() {
                    self.respond(&mut s.transport, id, &Empty {}).await
                } else {
                    let error = RpcError::new(ErrorCode::NotFound, "no such subscription");
                    self.respond_error(&mut s.transport, id, &error).await
                }
            }
            SessionRequest::TerminalInput(TerminalInputParams { session, data }) => {
                self.write(s, id, session, data).await
            }
            SessionRequest::TerminalPaste(params) => self.paste(s, id, params).await,
            SessionRequest::DeviceUnpair => self.unpair(s, id).await,
        }
    }

    async fn hello(&mut self, s: &mut Session, id: RequestId, _params: HelloParams) -> Step {
        if !s.hello_done {
            self.start_session(s).await?;
        }
        let mut host_caps: Vec<String> =
            caps::HOST_V1.iter().map(|cap| (*cap).to_owned()).collect();
        if self.shared.backend.agent_status() {
            host_caps.push(caps::AGENT_STATUS_V1.to_owned());
        }
        let result = HelloResult {
            host: self.shared.backend.host_info(),
            device: s.device.id.clone(),
            caps: host_caps,
            time: unix_time_ms(),
        };
        s.hello_done = true;
        self.respond(&mut s.transport, id, &result).await
    }

    /// Marks the device as connected and updates its "last seen" time (section 7.4, step 7).
    ///
    /// The registry check of message 1 can run before a revoke, and this mark after it.
    /// So the mark checks again. A revoked device gets `device.revoked` and the close code `4401`.
    async fn start_session(&mut self, s: &mut Session) -> Step {
        let marked = self
            .shared
            .mark_session(self.conn, self.generation, s.device.id.clone());
        if !marked || !self.still_paired(&s.device).await {
            let sent = self
                .send_event(&mut s.transport, &HostEvent::DeviceRevoked)
                .await;
            return Err(sent.err().unwrap_or(Ending::Close(CloseCode::Unauthorized)));
        }
        let store = self.shared.store.clone();
        let seen = s.device.id.clone();
        let now = unix_time_ms();
        tokio::task::spawn_blocking(move || {
            let _ = store.touch_last_seen(&seen, now);
        });
        Ok(())
    }

    /// Returns true when the registry still has the device and the device is not revoked.
    async fn still_paired(&self, device: &StoredDevice) -> bool {
        if self.shared.is_revoked(&device.id) {
            return false;
        }
        let store = self.shared.store.clone();
        let key = device.noise_public;
        let found = blocking(move || store.find_by_noise_key(&key)).await;
        matches!(found, Ok(Ok(Some(stored))) if !stored.is_revoked() && stored.id == device.id)
    }

    async fn send_panes_changed(
        &mut self,
        s: &mut Session,
        panes: &mut watch::Receiver<Arc<PaneList>>,
    ) -> Step {
        let list = panes.borrow_and_update().clone();
        if list.rev <= s.panes_rev {
            return Ok(());
        }
        s.panes_rev = list.rev;
        let event = HostEvent::PanesChanged((*list).clone());
        self.send_event(&mut s.transport, &event).await
    }

    /// `terminal.subscribe`: a replay with `fromSeq` when the ring holds every chunk after it,
    /// otherwise one snapshot (section 8.8).
    async fn subscribe(
        &mut self,
        s: &mut Session,
        id: RequestId,
        params: TerminalSubscribeParams,
    ) -> Step {
        if s.subs.len() >= MAX_TERMINAL_SUBSCRIPTIONS {
            let error = RpcError::new(
                ErrorCode::Busy,
                "a session has at most 8 terminal subscriptions",
            );
            return self.respond_error(&mut s.transport, id, &error).await;
        }
        let session = params.session;
        let resume = params.from_seq.is_some();
        let replay = match params.from_seq {
            Some(after) => {
                let backend = self.shared.backend.clone();
                let name = session.clone();
                blocking(move || backend.replay_since(&name, after))
                    .await
                    .ok()
                    .flatten()
                    .map(|replay| (after, replay))
            }
            None => None,
        };
        let sub_id = s.next_sub_id();
        if let Some((after, replay)) = replay {
            let seq = replay
                .chunks
                .iter()
                .map(|chunk| chunk.seq)
                .filter(|seq| *seq > after)
                .max()
                .unwrap_or(after);
            let result = TerminalSubscribeResult {
                sub: sub_id,
                cols: replay.cols,
                rows: replay.rows,
                seq,
            };
            self.respond(&mut s.transport, id, &result).await?;
            let mut sub = Subscription::new(sub_id, session, after);
            let now = Instant::now();
            for chunk in &replay.chunks {
                if sub.push(chunk.seq, &chunk.data, now) == Push::FlushNow {
                    self.flush(&mut s.transport, &mut sub).await?;
                }
            }
            self.flush(&mut s.transport, &mut sub).await?;
            s.subs.insert(sub_id, sub);
            return self.send_missed_exit(s, sub_id).await;
        }
        let Some(snapshot) = self.snapshot(&session).await else {
            let error = RpcError::new(ErrorCode::NotFound, "the pane does not exist");
            return self.respond_error(&mut s.transport, id, &error).await;
        };
        let result = TerminalSubscribeResult {
            sub: sub_id,
            cols: snapshot.cols,
            rows: snapshot.rows,
            seq: snapshot.seq,
        };
        self.respond(&mut s.transport, id, &result).await?;
        let mut sub = Subscription::new(sub_id, session, 0);
        self.send_snapshot(&mut s.transport, &mut sub, snapshot)
            .await?;
        s.subs.insert(sub_id, sub);
        if resume {
            return self.send_missed_exit(s, sub_id).await;
        }
        Ok(())
    }

    /// Sends `terminal.exit` after a resume when the process of the pane exited
    /// while the device was away (section 8.8).
    ///
    /// The host does not know if the device saw the exit before it went away,
    /// so a device can get the same exit twice. A second exit changes nothing on the device.
    async fn send_missed_exit(&mut self, s: &mut Session, sub: u32) -> Step {
        let Some(session) = s.subs.get(&sub).map(|sub| sub.session.clone()) else {
            return Ok(());
        };
        let list = self.shared.refresh_panes().await;
        let Some(pane) = list.panes.iter().find(|pane| pane.session == session) else {
            return Ok(());
        };
        if pane.state != PaneState::Exited {
            return Ok(());
        }
        let event = HostEvent::TerminalExit(TerminalExit {
            sub,
            code: pane.exit_code,
            signal: None,
        });
        self.send_event(&mut s.transport, &event).await
    }

    async fn snapshot(&self, session: &str) -> Option<Snapshot> {
        let backend = self.shared.backend.clone();
        let name = session.to_owned();
        blocking(move || backend.snapshot(&name))
            .await
            .ok()
            .flatten()
    }

    async fn send_snapshot(
        &self,
        transport: &mut Transport,
        sub: &mut Subscription,
        snapshot: Snapshot,
    ) -> Step {
        let mut data = snapshot.data;
        // A message holds at most 8 MiB. A larger ring keeps its newest bytes.
        if data.len() > MAX_SNAPSHOT_BYTES {
            data.drain(..data.len() - MAX_SNAPSHOT_BYTES);
        }
        let seq = snapshot.seq;
        let event = HostEvent::TerminalSnapshot(TerminalSnapshot {
            sub: sub.id,
            seq,
            cols: snapshot.cols,
            rows: snapshot.rows,
            data,
        });
        self.send_event(transport, &event).await?;
        sub.snapshot_sent(seq);
        Ok(())
    }

    /// Sends the batch of a subscription, if the window allows it.
    async fn flush(&self, transport: &mut Transport, sub: &mut Subscription) -> Step {
        match sub.take_batch() {
            Some(data) => {
                self.send_event(transport, &HostEvent::TerminalData(data))
                    .await
            }
            None => Ok(()),
        }
    }

    async fn flush_due(&mut self, s: &mut Session) -> Step {
        let now = Instant::now();
        let due: Vec<u32> = s
            .subs
            .iter()
            .filter(|(_, sub)| sub.deadline().is_some_and(|deadline| deadline <= now))
            .map(|(id, _)| *id)
            .collect();
        for id in due {
            if let Some(sub) = s.subs.get_mut(&id) {
                self.flush(&mut s.transport, sub).await?;
            }
        }
        Ok(())
    }

    async fn on_output(&mut self, s: &mut Session, event: Result<OutputEvent, RecvError>) -> Step {
        let event = match event {
            Ok(event) => event,
            Err(RecvError::Lagged(_)) => return self.resync(s).await,
            Err(RecvError::Closed) => return Ok(()),
        };
        let ids = s.subs_of(event.session());
        if ids.is_empty() {
            return Ok(());
        }
        let now = Instant::now();
        for id in ids {
            let Some(sub) = s.subs.get_mut(&id) else {
                continue;
            };
            match &event {
                OutputEvent::Data { seq, data, .. } => {
                    if sub.push(*seq, data, now) == Push::FlushNow {
                        self.flush(&mut s.transport, sub).await?;
                    }
                }
                OutputEvent::Resized { cols, rows, .. } => {
                    self.flush(&mut s.transport, sub).await?;
                    // A stopped subscription gets the size with its next snapshot.
                    if !sub.is_paused() {
                        let event = HostEvent::TerminalResized(TerminalResized {
                            sub: id,
                            cols: *cols,
                            rows: *rows,
                        });
                        self.send_event(&mut s.transport, &event).await?;
                    }
                }
                OutputEvent::Exit { code, signal, .. } => {
                    self.flush(&mut s.transport, sub).await?;
                    let event = HostEvent::TerminalExit(TerminalExit {
                        sub: id,
                        code: *code,
                        signal: signal.clone(),
                    });
                    self.send_event(&mut s.transport, &event).await?;
                }
                OutputEvent::Closed { .. } => {
                    self.flush(&mut s.transport, sub).await?;
                    s.subs.remove(&id);
                    let event = HostEvent::TerminalClosed(TerminalClosed {
                        sub: id,
                        reason: "closed".to_owned(),
                    });
                    self.send_event(&mut s.transport, &event).await?;
                }
            }
        }
        Ok(())
    }

    /// The output tap lost events, so each subscription needs a new snapshot.
    ///
    /// The window decides when it goes out: now, when the count is below 128 KiB,
    /// or after the acks that bring it there (section 8.8).
    async fn resync(&mut self, s: &mut Session) -> Step {
        let ids: Vec<u32> = s.subs.keys().copied().collect();
        for id in ids {
            let Some(sub) = s.subs.get_mut(&id) else {
                continue;
            };
            if !sub.lost_output() {
                continue;
            }
            let session = sub.session.clone();
            let snapshot = self.snapshot(&session).await;
            self.send_resume_snapshot(s, id, snapshot).await?;
        }
        Ok(())
    }

    /// Sends a new snapshot to a subscription, or `terminal.closed` when the pane is gone.
    async fn send_resume_snapshot(
        &self,
        s: &mut Session,
        id: u32,
        snapshot: Option<Snapshot>,
    ) -> Step {
        match snapshot {
            Some(snapshot) => match s.subs.get_mut(&id) {
                Some(sub) => self.send_snapshot(&mut s.transport, sub, snapshot).await,
                None => Ok(()),
            },
            None => {
                s.subs.remove(&id);
                let event = HostEvent::TerminalClosed(TerminalClosed {
                    sub: id,
                    reason: "closed".to_owned(),
                });
                self.send_event(&mut s.transport, &event).await
            }
        }
    }

    async fn on_ack(&mut self, s: &mut Session, ack: TerminalAck) -> Step {
        let Some(sub) = s.subs.get_mut(&ack.sub) else {
            return Ok(());
        };
        if !sub.ack(ack.seq) {
            return Ok(());
        }
        // The device caught up: a new snapshot replaces the output that it missed.
        let session = sub.session.clone();
        let snapshot = self.snapshot(&session).await;
        self.send_resume_snapshot(s, ack.sub, snapshot).await
    }

    /// Writes input to the PTY in parts of at most 32 KiB (section 8.8).
    async fn write(
        &mut self,
        s: &mut Session,
        id: RequestId,
        session: String,
        data: Vec<u8>,
    ) -> Step {
        let data = Zeroizing::new(data);
        // The input of a device whose revoke started never reaches the PTY (section 9).
        if self.shared.is_revoked(&s.device.id) {
            let error = RpcError::new(ErrorCode::Denied, "the device was revoked");
            return self.respond_error(&mut s.transport, id, &error).await;
        }
        let backend = self.shared.backend.clone();
        let written = blocking(move || {
            let mut rest = &data[..];
            loop {
                let (part, tail) = rest.split_at(rest.len().min(MAX_WRITE_BYTES));
                backend.write(&session, part)?;
                if tail.is_empty() {
                    return Ok(());
                }
                rest = tail;
            }
        })
        .await;
        match written {
            Ok(Ok(())) => self.respond(&mut s.transport, id, &Empty {}).await,
            Ok(Err(WriteError::NotFound)) => {
                let error = RpcError::new(ErrorCode::NotFound, "the pane does not exist");
                self.respond_error(&mut s.transport, id, &error).await
            }
            Ok(Err(_)) | Err(_) => {
                let error = RpcError::new(ErrorCode::Internal, "the write failed");
                self.respond_error(&mut s.transport, id, &error).await
            }
        }
    }

    async fn paste(&mut self, s: &mut Session, id: RequestId, params: TerminalPasteParams) -> Step {
        let mode = self.shared.paste_mode(&params.session).await;
        let bytes = encode_paste(&params.text, mode.brackets(&params.text), params.submit);
        self.write(s, id, params.session, bytes).await
    }

    /// `device.unpair`: revoke the device, answer `{}`, send `device.revoked`, and end.
    /// The rest of the revoke (other sessions, `device_del`, and `4401`) runs after the task.
    async fn unpair(&mut self, s: &mut Session, id: RequestId) -> Step {
        self.shared.note_revoked(&s.device.id);
        let store = self.shared.store.clone();
        let device = s.device.id.clone();
        let now = unix_time_ms();
        if !matches!(
            blocking(move || store.revoke(&device, now)).await,
            Ok(Ok(_))
        ) {
            let error = RpcError::new(ErrorCode::Internal, "the host could not revoke the device");
            return self.respond_error(&mut s.transport, id, &error).await;
        }
        self.respond(&mut s.transport, id, &Empty {}).await?;
        self.send_event(&mut s.transport, &HostEvent::DeviceRevoked)
            .await?;
        Err(Ending::Unpaired(s.device.id.clone()))
    }
}

/// Stores an allowed device and registers its token hash with the relay (section 7.3, step 9).
///
/// It sends the outcome on `result`. The device stays only when the pairing connection
/// answers `pair.request` and then sends on `keep`. When the connection ends first, for example
/// because its task was aborted, `keep` closes and this task revokes the device again.
/// So an incomplete pairing never leaves an active device in the registry.
async fn commit_device(
    shared: Arc<Shared>,
    device: NewDevice,
    result: oneshot::Sender<Result<(), RpcError>>,
    keep: oneshot::Receiver<()>,
) {
    let id = device.id.clone();
    let hash = device.token_hash.clone();
    let store = shared.store.clone();
    if !matches!(blocking(move || store.insert(device)).await, Ok(Ok(_))) {
        let error = RpcError::new(ErrorCode::Internal, "the device registry failed");
        let _ = result.send(Err(error));
        return;
    }
    let registered = shared
        .control(|rid| HostToRelay::DevicePut {
            rid,
            device: id.clone(),
            hash,
        })
        .await;
    if registered.is_err() {
        // A device that the relay does not admit must not stay active in the registry.
        revoke_unfinished(&shared, &id).await;
        let error = RpcError::new(
            ErrorCode::Unavailable,
            "the relay did not accept the device",
        );
        let _ = result.send(Err(error));
        return;
    }
    if result.send(Ok(())).is_ok() && keep.await.is_ok() {
        return;
    }
    // The pairing connection ended before it answered. The phone has no device id.
    revoke_unfinished(&shared, &id).await;
    let _ = shared
        .control(|rid| HostToRelay::DeviceDel { rid, device: id })
        .await;
}

/// Sets `revoked_at` for a device of a pairing that did not finish.
async fn revoke_unfinished(shared: &Shared, id: &DeviceId) {
    let store = shared.store.clone();
    let target = id.clone();
    let now = unix_time_ms();
    let _ = blocking(move || store.revoke(&target, now)).await;
}

/// Opens one Noise message. A decryption, framing, or decoding failure ends the connection
/// with `4400` (section 7.7).
///
/// The plaintext can hold a relay token or keystrokes, so it is zeroized after the decode.
fn open_message(transport: &mut Transport, bytes: &[u8]) -> Result<Option<Message>, Ending> {
    match transport.open(bytes) {
        Ok(Some(message)) => Message::decode(&Zeroizing::new(message))
            .map(Some)
            .map_err(|_| Ending::Close(CloseCode::BadFrame)),
        Ok(None) => Ok(None),
        Err(_) => Err(Ending::Close(CloseCode::BadFrame)),
    }
}
