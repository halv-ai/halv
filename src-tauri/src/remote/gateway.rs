//! The gateway core: the lifetime of the host engine, pairing, the device list, and the status.
//!
//! The core has no Tauri dependency. `super::RemoteManager` connects it to the app state,
//! and the tests connect it to fakes and to the mock relay of `halv-remote`.
//!
//! Rules:
//!
//! - The engine runs only while the `remoteEnabled` preference is on.
//!   When it is off, the app makes no connection to the relay.
//! - The identity loads from the keychain when the engine starts for the first time.
//!   A keychain read error keeps the gateway off and shows the error (section 4.1).
//! - One lifecycle lock orders start, stop, pairing, the pairing answer, revoke, and reset.
//!   The view lock is short and never held across an `await`.
//! - The device registry is the registry of the loaded host identity. Before the identity
//!   loads, the status shows no devices.
//! - Events never hold the pairing offer, because every webview can read events.

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, Weak};
use std::time::Duration;

use halv_remote::host::{
    BoxFuture, DeviceStore, HostConfig, HostEngine, HostError, HostHandle, HostStatus, NewDevice,
    OfflineReason, PairRequestInfo, PairingApprover, PairingOfferInfo, StoreError, StoredDevice,
};
use halv_remote::net::ConnectError;
use halv_remote::rpc::HostInfo;
use halv_remote::{unix_time_ms, DeviceId, HostIdentity, UpgradeError};
use tokio::sync::oneshot;

use super::backend::{AgentStatusSource, ConversationActivity, GatewayBackend, TerminalSource};
use super::identity::{self, Keychain};
use super::qr::qr_svg_data_uri;
use super::relay_url;
use super::store::{HostDevices, SqliteDeviceStore};
use super::{RemoteConnection, RemoteDevice, RemotePairing, RemotePairingRequest, RemoteStatus};

/// The monitor reads the connected devices and the offer expiry this often.
const MONITOR_INTERVAL: Duration = Duration::from_secs(2);
/// The monitor asks the engine to read the pane list again after this many intervals.
/// The tap reports most changes at once. This catches a new pane that has no output yet.
const PANE_REFRESH_TICKS: u32 = 8;
/// The longest wait for the engine to close the host socket.
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);

/// The preferences of the gateway.
pub(crate) trait RemoteSettings: Send + Sync + 'static {
    /// Returns the `remoteEnabled` preference.
    fn enabled(&self) -> bool;
    /// Returns the relay URL preference of this build profile.
    fn relay_url(&self) -> Option<String>;
    /// Stores the `remoteEnabled` preference.
    fn set_enabled(&self, enabled: bool) -> Result<(), String>;
    /// Stores the relay URL preference of this build profile. `None` selects the default.
    fn set_relay_url(&self, relay_url: Option<String>) -> Result<(), String>;
}

/// The events of the gateway for the main webview.
pub(crate) trait RemoteEvents: Send + Sync + 'static {
    /// `remote-changed`. The status never holds the pairing offer.
    fn changed(&self, status: &RemoteStatus);
    /// `remote-pairing-request`.
    fn pairing_request(&self, request: &RemotePairingRequest);
}

type HostNameFn = dyn Fn() -> String + Send + Sync;
type ConfigureFn = dyn Fn(&mut HostConfig) + Send + Sync;

/// The parts of the app that the gateway uses.
pub(crate) struct GatewayDeps {
    pub(crate) settings: Arc<dyn RemoteSettings>,
    pub(crate) events: Arc<dyn RemoteEvents>,
    pub(crate) keychain: Keychain,
    pub(crate) store: Arc<SqliteDeviceStore>,
    pub(crate) terminals: Arc<dyn TerminalSource>,
    pub(crate) agent_status: Arc<dyn AgentStatusSource>,
    /// Updates the conversation time after remote input.
    pub(crate) conversations: Arc<dyn ConversationActivity>,
    /// The relay URL when the preference is not set.
    pub(crate) default_relay_url: String,
    /// Returns the host name. The gateway calls it once.
    pub(crate) host_name: Arc<HostNameFn>,
    /// Changes the engine settings. Only tests use it.
    pub(crate) configure: Option<Arc<ConfigureFn>>,
}

/// A pairing offer on the screen.
struct ActivePairing {
    offer: PairingOfferInfo,
    qr_svg_data_uri: String,
}

impl ActivePairing {
    fn to_remote(&self) -> RemotePairing {
        RemotePairing {
            uri: self.offer.uri.as_str().to_owned(),
            qr_svg_data_uri: self.qr_svg_data_uri.clone(),
            expires_at: self.offer.expires_at_ms,
        }
    }
}

/// A pairing request that waits for the user.
struct PendingApproval {
    request: RemotePairingRequest,
    answer: oneshot::Sender<bool>,
}

/// The state that the status shows.
struct View {
    /// A copy of `Lifecycle::active` for the status.
    active: bool,
    running: bool,
    connection: RemoteConnection,
    error: Option<String>,
    host_id: Option<String>,
    pairing: Option<ActivePairing>,
    pending: Option<PendingApproval>,
    connected: BTreeSet<String>,
}

impl Default for View {
    fn default() -> View {
        View {
            active: false,
            running: false,
            connection: RemoteConnection::Off,
            error: None,
            host_id: None,
            pairing: None,
            pending: None,
            connected: BTreeSet::new(),
        }
    }
}

struct Shared {
    deps: GatewayDeps,
    host_name: OnceLock<String>,
    view: Mutex<View>,
}

struct RunningEngine {
    handle: HostHandle,
    relay_url: String,
    monitor: tokio::task::JoinHandle<()>,
    /// The registry of the identity that the engine uses.
    registry: HostDevices,
}

#[derive(Default)]
struct Lifecycle {
    identity: Option<Arc<HostIdentity>>,
    engine: Option<RunningEngine>,
    /// True while the app allows the gateway to run: the `mobileRemote` feature flag is on.
    /// The engine runs only when the gateway is active and the preference is on.
    active: bool,
}

/// The desktop remote gateway.
pub(crate) struct Gateway {
    shared: Arc<Shared>,
    lifecycle: tokio::sync::Mutex<Lifecycle>,
}

impl Shared {
    fn view(&self) -> MutexGuard<'_, View> {
        self.view
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn host_name(&self) -> String {
        self.host_name
            .get_or_init(|| (self.deps.host_name)())
            .clone()
    }

    fn relay_url(&self) -> (String, bool) {
        relay_url::effective_relay_url(
            self.deps.settings.relay_url().as_deref(),
            &self.deps.default_relay_url,
        )
    }

    /// Returns the registry of the loaded identity, or `None` before the identity loads.
    fn registry(&self) -> Option<HostDevices> {
        let host_id = self.view().host_id.clone()?;
        Some(self.deps.store.for_host(host_id))
    }

    fn status(&self) -> RemoteStatus {
        let enabled = self.deps.settings.enabled();
        let (relay_url, relay_url_is_default) = self.relay_url();
        let host_name = self.host_name();
        // The registry query runs before the view lock, so the lock stays short.
        let devices = self
            .registry()
            .map(|registry| registry.active())
            .transpose();
        let view = self.view();
        let (connection, mut error) = if !enabled || !view.active {
            (RemoteConnection::Off, view.error.clone())
        } else if view.running {
            (view.connection, view.error.clone())
        } else if view.error.is_some() {
            (RemoteConnection::Error, view.error.clone())
        } else {
            (RemoteConnection::Connecting, None)
        };
        let devices = match devices {
            Ok(devices) => devices
                .unwrap_or_default()
                .iter()
                .map(|device| remote_device(device, view.connected.contains(device.id.as_str())))
                .collect(),
            Err(store_error) => {
                error.get_or_insert(store_error.0);
                Vec::new()
            }
        };
        RemoteStatus {
            enabled,
            relay_url,
            relay_url_is_default,
            connection,
            error,
            host_id: view.host_id.clone(),
            host_name,
            devices,
            pairing: view.pairing.as_ref().map(ActivePairing::to_remote),
            pending_approval: view.pending.as_ref().map(|pending| pending.request.clone()),
        }
    }

    /// Sends `remote-changed`. Every webview can read events, so the event status never
    /// holds the pairing URI or the QR code. The main webview reads the offer with the
    /// `remote_status` command, which checks the webview label.
    fn emit_changed(&self) {
        let mut status = self.status();
        status.pairing = None;
        self.deps.events.changed(&status);
    }

    fn set_error(&self, error: String) {
        self.view().error = Some(error);
    }

    /// Applies an engine state. Returns true when the view changed.
    fn apply_host_status(&self, status: &HostStatus) -> bool {
        let (connection, error) = describe_host_status(status);
        let mut view = self.view();
        if !view.running || (view.connection == connection && view.error == error) {
            return false;
        }
        view.connection = connection;
        view.error = error;
        true
    }

    /// Stores the devices with an open session.
    /// Returns the devices whose session ended, or `None` when the set did not change.
    fn set_connected(&self, connected: BTreeSet<String>) -> Option<Vec<String>> {
        let mut view = self.view();
        if view.connected == connected {
            return None;
        }
        let ended = view.connected.difference(&connected).cloned().collect();
        view.connected = connected;
        Some(ended)
    }

    /// Removes an offer after its expiry. Returns true when it removed one.
    fn expire_pairing(&self, now_ms: u64) -> bool {
        let mut view = self.view();
        if view
            .pairing
            .as_ref()
            .is_some_and(|pairing| pairing.offer.expires_at_ms <= now_ms)
        {
            view.pairing = None;
            true
        } else {
            false
        }
    }
}

fn remote_device(device: &StoredDevice, connected: bool) -> RemoteDevice {
    RemoteDevice {
        id: device.id.to_string(),
        name: device.name.clone(),
        platform: device.platform.as_str().to_string(),
        model: device.model.clone(),
        paired_at: device.created_at_ms,
        last_seen_at: device.last_seen_at_ms,
        connected,
    }
}

fn retry_text(retry_in: Duration) -> String {
    let seconds = retry_in.as_secs() + u64::from(retry_in.subsec_nanos() > 0);
    format!("Halv tries again in {} s.", seconds.max(1))
}

/// Maps an engine state to the connection and the error text of the status.
pub(crate) fn describe_host_status(status: &HostStatus) -> (RemoteConnection, Option<String>) {
    match status {
        HostStatus::Connecting => (RemoteConnection::Connecting, None),
        HostStatus::Online => (RemoteConnection::Connected, None),
        HostStatus::Offline { reason, retry_in } => {
            let reason = match reason {
                OfflineReason::Connect(ConnectError::Upgrade {
                    error: UpgradeError::ClockSkew,
                    ..
                }) => "The relay refused the connection. Check the clock of this computer."
                    .to_string(),
                OfflineReason::Connect(ConnectError::Upgrade { error, .. }) => {
                    format!("The relay refused the connection ({error:?}).")
                }
                OfflineReason::Connect(ConnectError::Http(code)) => {
                    format!("The relay answered with HTTP {code}.")
                }
                OfflineReason::Connect(ConnectError::InvalidUrl) => {
                    "The relay URL is not valid.".to_string()
                }
                OfflineReason::Connect(_) => "Halv cannot reach the relay.".to_string(),
                OfflineReason::Closed(_) => "The relay closed the connection.".to_string(),
                OfflineReason::Network(_) => "The connection to the relay failed.".to_string(),
                OfflineReason::IdleTimeout => "The relay stopped answering.".to_string(),
                _ => "The relay is not connected.".to_string(),
            };
            (
                RemoteConnection::Error,
                Some(format!("{reason} {}", retry_text(*retry_in))),
            )
        }
        HostStatus::Replaced => (
            RemoteConnection::Error,
            Some(
                "Another Halv app with this identity connected to the relay. Turn Mobile off and on to connect this one."
                    .to_string(),
            ),
        ),
        HostStatus::Stopped => (RemoteConnection::Off, None),
        _ => (RemoteConnection::Connecting, None),
    }
}

fn pairing_error(error: HostError) -> String {
    match error {
        HostError::Offline => "The relay is not connected yet. Try again in a moment.".to_string(),
        HostError::Relay { code } => format!("The relay refused the pairing offer ({code})."),
        HostError::Timeout => "The relay did not answer. Try again.".to_string(),
        HostError::Stopped => "Mobile is turned off.".to_string(),
        _ => "Halv could not start pairing.".to_string(),
    }
}

impl Gateway {
    pub(crate) fn new(deps: GatewayDeps) -> Gateway {
        Gateway {
            shared: Arc::new(Shared {
                deps,
                host_name: OnceLock::new(),
                view: Mutex::new(View::default()),
            }),
            lifecycle: tokio::sync::Mutex::new(Lifecycle::default()),
        }
    }

    /// Returns the status.
    pub(crate) fn status(&self) -> RemoteStatus {
        self.shared.status()
    }

    /// Returns the paired devices of the loaded identity that are not revoked.
    pub(crate) fn devices(&self) -> Result<Vec<RemoteDevice>, String> {
        let Some(registry) = self.shared.registry() else {
            return Ok(Vec::new());
        };
        let devices = registry.active().map_err(|error| error.0)?;
        let view = self.shared.view();
        Ok(devices
            .iter()
            .map(|device| remote_device(device, view.connected.contains(device.id.as_str())))
            .collect())
    }

    /// Activates or deactivates the gateway. The app calls it with the `mobileRemote` flag.
    ///
    /// - Activation starts the engine when the preference is on.
    /// - Deactivation stops the engine and keeps the preference.
    ///
    /// The gateway starts inactive, so a stored preference alone opens no relay connection.
    pub(crate) async fn set_active(&self, active: bool) -> RemoteStatus {
        let mut lifecycle = self.lifecycle.lock().await;
        lifecycle.active = active;
        self.shared.view().active = active;
        if !active {
            self.stop_engine(&mut lifecycle).await;
        } else if self.shared.deps.settings.enabled() {
            self.ensure_running(&mut lifecycle).await;
        }
        drop(lifecycle);
        self.shared.emit_changed();
        self.status()
    }

    /// Turns the gateway on or off.
    pub(crate) async fn set_enabled(&self, enabled: bool) -> Result<RemoteStatus, String> {
        let mut lifecycle = self.lifecycle.lock().await;
        self.shared.deps.settings.set_enabled(enabled)?;
        if enabled {
            self.shared.view().error = None;
            self.ensure_running(&mut lifecycle).await;
        } else {
            self.stop_engine(&mut lifecycle).await;
            self.shared.view().error = None;
        }
        drop(lifecycle);
        self.shared.emit_changed();
        Ok(self.status())
    }

    /// Sets the relay URL. `None` selects the default. A running engine reconnects.
    pub(crate) async fn set_relay_url(
        &self,
        relay_url: Option<String>,
    ) -> Result<RemoteStatus, String> {
        let relay_url = match relay_url.as_deref().map(str::trim) {
            Some(url) if !url.is_empty() => Some(relay_url::normalize_relay_url(url)?),
            _ => None,
        };
        let mut lifecycle = self.lifecycle.lock().await;
        self.shared.deps.settings.set_relay_url(relay_url)?;
        if self.shared.deps.settings.enabled() {
            self.shared.view().error = None;
            self.ensure_running(&mut lifecycle).await;
        }
        drop(lifecycle);
        self.shared.emit_changed();
        Ok(self.status())
    }

    /// Makes a pairing offer and returns its QR code (section 7.3, step 1).
    pub(crate) async fn pairing_start(&self) -> Result<RemotePairing, String> {
        let lifecycle = self.lifecycle.lock().await;
        let Some(engine) = lifecycle.engine.as_ref() else {
            return Err("Turn on Mobile before you pair a phone.".to_string());
        };
        let offer = engine.handle.start_pairing().await.map_err(pairing_error)?;
        let qr_svg_data_uri = match qr_svg_data_uri(&offer.uri) {
            Ok(qr) => qr,
            Err(error) => {
                engine.handle.cancel_pairing().await;
                return Err(error);
            }
        };
        let pairing = ActivePairing {
            offer,
            qr_svg_data_uri,
        };
        let remote = pairing.to_remote();
        self.shared.view().pairing = Some(pairing);
        drop(lifecycle);
        self.shared.emit_changed();
        Ok(remote)
    }

    /// Ends the pairing offer. A waiting approval ends as "deny".
    pub(crate) async fn pairing_cancel(&self) -> RemoteStatus {
        let lifecycle = self.lifecycle.lock().await;
        if let Some(engine) = lifecycle.engine.as_ref() {
            engine.handle.cancel_pairing().await;
        }
        {
            let mut view = self.shared.view();
            view.pairing = None;
            view.pending = None;
        }
        drop(lifecycle);
        self.shared.emit_changed();
        self.status()
    }

    /// Answers the waiting pairing request.
    ///
    /// The lifecycle lock orders the answer with a reset: a reset never runs between
    /// the check of the request and the answer.
    pub(crate) async fn pairing_respond(
        &self,
        request_id: &str,
        allow: bool,
    ) -> Result<RemoteStatus, String> {
        let lifecycle = self.lifecycle.lock().await;
        let pending = {
            let mut view = self.shared.view();
            let matches = view
                .pending
                .as_ref()
                .is_some_and(|pending| pending.request.request_id == request_id);
            if !matches {
                return Err("This pairing request is no longer waiting.".to_string());
            }
            // The engine ends the offer after each answer.
            view.pairing = None;
            view.pending.take()
        };
        if let Some(pending) = pending {
            let _ = pending.answer.send(allow);
        }
        drop(lifecycle);
        self.shared.emit_changed();
        Ok(self.status())
    }

    /// Revokes a paired device (section 9).
    pub(crate) async fn device_revoke(&self, device_id: &str) -> Result<RemoteStatus, String> {
        const UNKNOWN: &str = "This device is not paired.";
        let id = DeviceId::parse(device_id).map_err(|_| UNKNOWN.to_string())?;
        let lifecycle = self.lifecycle.lock().await;
        match lifecycle.engine.as_ref() {
            Some(engine) => match engine.handle.revoke_device(&id).await {
                Ok(()) => {}
                Err(HostError::UnknownDevice) => return Err(UNKNOWN.to_string()),
                Err(HostError::Store(error)) => return Err(error.0),
                // The registry already revoked the device. The next `devices_sync` updates the relay.
                Err(error) => {
                    eprintln!("remote gateway: the relay did not confirm a revoke: {error}")
                }
            },
            None => {
                // Without a loaded identity, no device is known.
                let Some(identity) = lifecycle.identity.as_ref() else {
                    return Err(UNKNOWN.to_string());
                };
                let registry = self
                    .shared
                    .deps
                    .store
                    .for_host(identity.host_id().to_string());
                let revoked =
                    tokio::task::spawn_blocking(move || registry.revoke(&id, unix_time_ms()))
                        .await
                        .map_err(|_| "Halv could not revoke the device.".to_string())?
                        .map_err(|error| error.0)?;
                if !revoked {
                    return Err(UNKNOWN.to_string());
                }
            }
        }
        drop(lifecycle);
        self.shared.emit_changed();
        Ok(self.status())
    }

    /// Revokes every device of the old identity and makes a new host identity (section 9).
    pub(crate) async fn identity_reset(&self) -> Result<RemoteStatus, String> {
        let mut lifecycle = self.lifecycle.lock().await;
        let identity = Arc::new(
            HostIdentity::generate()
                .map_err(|_| "Halv could not make a remote identity.".to_string())?,
        );

        // 1. End the pairing offer, so no phone pairs with the old identity during the reset.
        //    Dropping the answer channel ends a waiting approval as "deny".
        if let Some(engine) = lifecycle.engine.as_ref() {
            engine.handle.cancel_pairing().await;
        }
        {
            let mut view = self.shared.view();
            view.pairing = None;
            view.pending = None;
        }

        // 2. Revoke every device of the old identity. Devices of other identities stay.
        //    A running engine sends `device.revoked` to open sessions.
        if let Some(registry) = self.current_registry(&lifecycle).await {
            if let Some(engine) = lifecycle.engine.as_ref() {
                for device in active_devices(&registry).await? {
                    if let Err(error) = engine.handle.revoke_device(&device.id).await {
                        eprintln!(
                            "remote gateway: a revoke during the identity reset failed: {error}"
                        );
                    }
                }
            }
            tokio::task::spawn_blocking(move || registry.revoke_all(unix_time_ms()))
                .await
                .map_err(|_| "Halv could not revoke the devices.".to_string())?
                .map_err(|error| error.0)?;
        }

        // 3. Store the new identity. A failure keeps the old identity, with every device revoked.
        let keychain = self.shared.deps.keychain.clone();
        let stored_identity = identity.clone();
        let stored =
            tokio::task::spawn_blocking(move || identity::store(&keychain, &stored_identity))
                .await
                .map_err(|_| "Halv could not write to the keychain.".to_string())
                .and_then(|result| result);
        if let Err(error) = stored {
            drop(lifecycle);
            self.shared.emit_changed();
            return Err(error);
        }

        // 4. Connect with the new identity.
        self.stop_engine(&mut lifecycle).await;
        self.shared.view().host_id = Some(identity.host_id().to_string());
        lifecycle.identity = Some(identity);
        if self.shared.deps.settings.enabled() {
            self.shared.view().error = None;
            self.ensure_running(&mut lifecycle).await;
        }
        drop(lifecycle);
        self.shared.emit_changed();
        Ok(self.status())
    }

    /// Stops the engine and closes the host socket. The app calls it at exit.
    pub(crate) async fn shutdown(&self) {
        let mut lifecycle = self.lifecycle.lock().await;
        self.stop_engine(&mut lifecycle).await;
    }

    /// Returns the registry of the current identity. It reads the keychain when the identity
    /// did not load yet. Without a readable identity, no device belongs to a known host.
    async fn current_registry(&self, lifecycle: &Lifecycle) -> Option<HostDevices> {
        let host_id = match lifecycle.identity.as_ref() {
            Some(identity) => identity.host_id().to_string(),
            None => {
                let keychain = self.shared.deps.keychain.clone();
                let loaded = tokio::task::spawn_blocking(move || identity::load(&keychain))
                    .await
                    .ok()?;
                loaded.ok().flatten()?.host_id().to_string()
            }
        };
        Some(self.shared.deps.store.for_host(host_id))
    }

    /// Starts the engine with the current relay URL, unless it already runs with it.
    /// An inactive gateway starts no engine.
    async fn ensure_running(&self, lifecycle: &mut Lifecycle) {
        if !lifecycle.active {
            return;
        }
        let (relay_url, _) = self.shared.relay_url();
        if let Some(engine) = lifecycle.engine.as_ref() {
            if engine.relay_url == relay_url {
                return;
            }
            self.stop_engine(lifecycle).await;
        }

        let identity = match lifecycle.identity.clone() {
            Some(identity) => identity,
            None => {
                let keychain = self.shared.deps.keychain.clone();
                let loaded =
                    tokio::task::spawn_blocking(move || identity::load_or_create(&keychain))
                        .await
                        .map_err(|_| "Halv could not read the remote identity.".to_string())
                        .and_then(|result| result);
                match loaded {
                    Ok(identity) => {
                        let identity = Arc::new(identity);
                        self.shared.view().host_id = Some(identity.host_id().to_string());
                        lifecycle.identity = Some(identity.clone());
                        identity
                    }
                    Err(error) => {
                        self.shared.set_error(error);
                        return;
                    }
                }
            }
        };

        let shared = self.shared.clone();
        let host_name = tokio::task::spawn_blocking(move || shared.host_name())
            .await
            .unwrap_or_else(|_| super::host_name::clean_host_name(None));
        let registry = self
            .shared
            .deps
            .store
            .for_host(identity.host_id().to_string());
        let mut config = HostConfig::new(relay_url.clone(), identity);
        if let Some(configure) = self.shared.deps.configure.as_ref() {
            configure(&mut config);
        }
        let approver = Arc::new(Approver {
            shared: Arc::downgrade(&self.shared),
            approval_timeout: config.approval_timeout,
        });
        let backend = GatewayBackend::new(
            self.shared.deps.terminals.clone(),
            self.shared.deps.agent_status.clone(),
            self.shared.deps.conversations.clone(),
            HostInfo {
                name: host_name,
                app: format!("halv-desktop/{}", env!("CARGO_PKG_VERSION")),
                os: std::env::consts::OS.to_string(),
            },
        );
        let store = Arc::new(NotifyingStore {
            inner: registry.clone(),
            shared: Arc::downgrade(&self.shared),
        });
        let handle = match HostEngine::start(config, backend.clone(), store, approver) {
            Ok(handle) => handle,
            Err(_) => {
                self.shared
                    .set_error("The relay URL is not valid.".to_string());
                return;
            }
        };
        self.shared.deps.terminals.install_tap(Some(backend.tap()));
        {
            let mut view = self.shared.view();
            view.running = true;
            view.connection = RemoteConnection::Connecting;
            view.error = None;
        }
        let monitor = tokio::spawn(monitor(
            Arc::downgrade(&self.shared),
            handle.clone(),
            backend,
            registry.clone(),
        ));
        lifecycle.engine = Some(RunningEngine {
            handle,
            relay_url,
            monitor,
            registry,
        });
    }

    async fn stop_engine(&self, lifecycle: &mut Lifecycle) {
        let Some(engine) = lifecycle.engine.take() else {
            return;
        };
        engine.monitor.abort();
        self.shared.deps.terminals.install_tap(None);
        if tokio::time::timeout(SHUTDOWN_TIMEOUT, engine.handle.shutdown())
            .await
            .is_err()
        {
            eprintln!("remote gateway: the engine did not stop in time");
        }
        let ended = {
            let mut view = self.shared.view();
            view.running = false;
            view.connection = RemoteConnection::Off;
            view.pairing = None;
            // Dropping the answer channel ends a waiting approval as "deny".
            view.pending = None;
            std::mem::take(&mut view.connected)
                .into_iter()
                .collect::<Vec<_>>()
        };
        // The stop ends every open session.
        touch_last_seen(&engine.registry, ended).await;
    }
}

async fn active_devices(registry: &HostDevices) -> Result<Vec<StoredDevice>, String> {
    let registry = registry.clone();
    tokio::task::spawn_blocking(move || registry.active())
        .await
        .map_err(|_| "Halv could not read the paired devices.".to_string())?
        .map_err(|error| error.0)
}

/// Sets "last seen" of the devices whose session ended.
/// The engine sets it at the start of a session, so it then shows the end of the last session.
async fn touch_last_seen(registry: &HostDevices, devices: Vec<String>) {
    if devices.is_empty() {
        return;
    }
    let registry = registry.clone();
    let now = unix_time_ms();
    let _ = tokio::task::spawn_blocking(move || {
        for device in devices {
            let Ok(id) = DeviceId::parse(&device) else {
                continue;
            };
            if let Err(error) = registry.touch_last_seen(&id, now) {
                eprintln!("remote gateway: Halv could not save the last seen time: {error}");
            }
        }
    })
    .await;
}

/// Follows the engine state, the connected devices, and the offer expiry.
async fn monitor(
    shared: Weak<Shared>,
    handle: HostHandle,
    backend: Arc<GatewayBackend>,
    registry: HostDevices,
) {
    let mut status = handle.subscribe_status();
    let mut tick = tokio::time::interval(MONITOR_INTERVAL);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut ticks = 0_u32;
    loop {
        let (ended, emit) = {
            let Some(gateway) = shared.upgrade() else {
                return;
            };
            let current = status.borrow_and_update().clone();
            let connected = handle
                .connected_devices()
                .into_iter()
                .map(|device| device.device.to_string())
                .collect();
            let changed = gateway.apply_host_status(&current);
            let ended = gateway.set_connected(connected);
            let expired = gateway.expire_pairing(unix_time_ms());
            let emit = changed || ended.is_some() || expired;
            (ended.unwrap_or_default(), emit)
        };
        // "Last seen" changes before the event, so the event shows the new time.
        touch_last_seen(&registry, ended).await;
        if emit {
            let Some(gateway) = shared.upgrade() else {
                return;
            };
            gateway.emit_changed();
        }
        tokio::select! {
            result = status.changed() => {
                if result.is_err() {
                    return;
                }
            }
            _ = tick.tick() => {
                ticks = ticks.wrapping_add(1);
                if ticks % PANE_REFRESH_TICKS == 0 {
                    backend.notify_panes();
                }
            }
        }
    }
}

/// The device registry for the engine. Each change refreshes the status in the webview.
struct NotifyingStore {
    inner: HostDevices,
    shared: Weak<Shared>,
}

impl NotifyingStore {
    fn notify<T>(&self, result: Result<T, StoreError>) -> Result<T, StoreError> {
        if result.is_ok() {
            if let Some(shared) = self.shared.upgrade() {
                shared.emit_changed();
            }
        }
        result
    }
}

impl DeviceStore for NotifyingStore {
    fn list(&self) -> Result<Vec<StoredDevice>, StoreError> {
        self.inner.list()
    }

    fn find_by_noise_key(&self, key: &[u8; 32]) -> Result<Option<StoredDevice>, StoreError> {
        self.inner.find_by_noise_key(key)
    }

    fn insert(&self, device: NewDevice) -> Result<StoredDevice, StoreError> {
        self.notify(self.inner.insert(device))
    }

    fn revoke(&self, id: &DeviceId, at_ms: u64) -> Result<bool, StoreError> {
        self.notify(self.inner.revoke(id, at_ms))
    }

    fn touch_last_seen(&self, id: &DeviceId, at_ms: u64) -> Result<(), StoreError> {
        self.notify(self.inner.touch_last_seen(id, at_ms))
    }
}

/// Asks the user in the main webview (section 7.3, step 8).
struct Approver {
    shared: Weak<Shared>,
    approval_timeout: Duration,
}

/// Removes a waiting request when the engine stops waiting for it.
struct PendingGuard {
    shared: Weak<Shared>,
    request_id: String,
    expires_at_ms: u64,
}

impl Drop for PendingGuard {
    fn drop(&mut self) {
        let Some(shared) = self.shared.upgrade() else {
            return;
        };
        let removed = {
            let mut view = shared.view();
            let waiting = view
                .pending
                .as_ref()
                .is_some_and(|pending| pending.request.request_id == self.request_id);
            if waiting {
                view.pending = None;
                // At the approval timeout the engine ends the offer too.
                if unix_time_ms().saturating_add(1_000) >= self.expires_at_ms {
                    view.pairing = None;
                }
            }
            waiting
        };
        if removed {
            shared.emit_changed();
        }
    }
}

fn new_request_id() -> String {
    halv_remote::b64u::encode(&rand::random::<[u8; 16]>())
}

impl PairingApprover for Approver {
    fn approve(&self, request: PairRequestInfo) -> BoxFuture<'_, bool> {
        let shared = self.shared.clone();
        let timeout_ms = u64::try_from(self.approval_timeout.as_millis()).unwrap_or(u64::MAX);
        Box::pin(async move {
            let Some(gateway) = shared.upgrade() else {
                return false;
            };
            let request = RemotePairingRequest {
                request_id: new_request_id(),
                name: request.name,
                platform: request.platform.as_str().to_string(),
                model: request.model,
                code: request.code.to_string(),
                expires_at: unix_time_ms().saturating_add(timeout_ms),
            };
            let (answer, receiver) = oneshot::channel();
            {
                let mut view = gateway.view();
                if view.pending.is_some() {
                    return false;
                }
                view.pending = Some(PendingApproval {
                    request: request.clone(),
                    answer,
                });
            }
            let _guard = PendingGuard {
                shared: shared.clone(),
                request_id: request.request_id.clone(),
                expires_at_ms: request.expires_at,
            };
            gateway.deps.events.pairing_request(&request);
            gateway.emit_changed();
            drop(gateway);
            let allowed = receiver.await.unwrap_or(false);
            if let Some(gateway) = shared.upgrade() {
                // The engine ends the offer after each answer.
                gateway.view().pairing = None;
            }
            allowed
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn engine_states_map_to_the_status_connection() {
        assert_eq!(
            describe_host_status(&HostStatus::Connecting),
            (RemoteConnection::Connecting, None)
        );
        assert_eq!(
            describe_host_status(&HostStatus::Online),
            (RemoteConnection::Connected, None)
        );
        assert_eq!(
            describe_host_status(&HostStatus::Stopped),
            (RemoteConnection::Off, None)
        );
        let (connection, error) = describe_host_status(&HostStatus::Offline {
            reason: OfflineReason::Connect(ConnectError::Timeout),
            retry_in: Duration::from_millis(1_500),
        });
        assert_eq!(connection, RemoteConnection::Error);
        assert_eq!(
            error.as_deref(),
            Some("Halv cannot reach the relay. Halv tries again in 2 s.")
        );
        let (_, error) = describe_host_status(&HostStatus::Offline {
            reason: OfflineReason::Connect(ConnectError::Upgrade {
                error: UpgradeError::ClockSkew,
                retry_after: None,
            }),
            retry_in: Duration::ZERO,
        });
        assert!(error.unwrap().contains("clock"));
        let (connection, error) = describe_host_status(&HostStatus::Replaced);
        assert_eq!(connection, RemoteConnection::Error);
        assert!(error.unwrap().contains("Turn Mobile off and on"));
    }
}
