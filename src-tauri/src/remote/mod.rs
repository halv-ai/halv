//! The desktop remote gateway (ADR 0009, GitHub issue #20).
//!
//! The gateway connects the host engine of `crates/halv-remote` to the app:
//!
//! - `backend`: the panes, the terminal output, and the input of `TerminalManager`.
//! - `store`: the device registry in the SQLite table `remote_devices`.
//! - `identity`: the host identity in the keychain.
//! - `gateway`: the lifetime of the engine, pairing, revoke, reset, and the status.
//! - `relay_url`, `qr`, `host_name`: small helpers.
//!
//! `docs/remote-protocol.md` is the normative contract. The gateway is off by default.
//! When it is off, the app makes no connection to the relay.
//! Never log tokens, secrets, keys, or pairing URIs (section 11).

mod agent_bridge;
mod backend;
mod gateway;
mod host_name;
mod identity;
mod qr;
mod relay_url;
mod store;
#[cfg(test)]
mod tests;

use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State, Webview};

use crate::preferences::{AgentPreferences, PreferencesManager};
use crate::storage::Storage;
use crate::terminal::TerminalManager;
use agent_bridge::DesktopAgentStatus;
use backend::ConversationActivity;
use gateway::{Gateway, GatewayDeps, RemoteEvents, RemoteSettings};
use identity::Keychain;
use store::SqliteDeviceStore;

/// The label of the main webview. Every command that changes the gateway accepts only it.
const MAIN_WEBVIEW: &str = "main";
/// True in debug builds. Debug builds keep their own relay URL preference.
const DEBUG_BUILD: bool = cfg!(debug_assertions);
/// The longest wait at app exit for the host socket to close.
const EXIT_TIMEOUT: Duration = Duration::from_secs(3);

/// The connection state of the gateway.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum RemoteConnection {
    Off,
    Connecting,
    Connected,
    Error,
}

/// A paired device that is not revoked.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RemoteDevice {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) platform: String,
    pub(crate) model: Option<String>,
    pub(crate) paired_at: u64,
    pub(crate) last_seen_at: Option<u64>,
    pub(crate) connected: bool,
}

/// A pairing offer on the screen. The URI and the QR code hold the pairing secret.
#[derive(Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RemotePairing {
    pub(crate) uri: String,
    pub(crate) qr_svg_data_uri: String,
    pub(crate) expires_at: u64,
}

impl std::fmt::Debug for RemotePairing {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RemotePairing")
            .field("expires_at", &self.expires_at)
            .finish_non_exhaustive()
    }
}

/// A pairing request that waits for the user (section 7.3, step 8).
/// `code` is the 6-digit confirmation code of section 7.3.1, with leading zeros.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RemotePairingRequest {
    pub(crate) request_id: String,
    pub(crate) name: String,
    pub(crate) platform: String,
    pub(crate) model: Option<String>,
    pub(crate) code: String,
    pub(crate) expires_at: u64,
}

/// The state of the gateway for Settings > Mobile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RemoteStatus {
    pub(crate) enabled: bool,
    pub(crate) relay_url: String,
    pub(crate) relay_url_is_default: bool,
    pub(crate) connection: RemoteConnection,
    pub(crate) error: Option<String>,
    pub(crate) host_id: Option<String>,
    pub(crate) host_name: String,
    pub(crate) devices: Vec<RemoteDevice>,
    pub(crate) pairing: Option<RemotePairing>,
    pub(crate) pending_approval: Option<RemotePairingRequest>,
}

/// Returns the relay URL preference of a build profile:
/// `remoteRelayUrlDev` in debug builds, `remoteRelayUrl` in release builds.
/// Debug and release builds share `preferences.json`, so a development relay URL
/// would otherwise reach a release build.
fn relay_url_preference(preferences: &AgentPreferences, debug_build: bool) -> Option<String> {
    if debug_build {
        preferences.remote_relay_url_dev.clone()
    } else {
        preferences.remote_relay_url.clone()
    }
}

/// Stores the relay URL preference of a build profile.
fn store_relay_url_preference(
    preferences: &PreferencesManager,
    relay_url: Option<String>,
    debug_build: bool,
) -> Result<(), String> {
    if debug_build {
        preferences.set_remote_relay_url_dev(relay_url)
    } else {
        preferences.set_remote_relay_url(relay_url)
    }
    .map(|_| ())
}

/// The preferences of the gateway in `preferences.json`.
struct PreferenceSettings(AppHandle);

impl RemoteSettings for PreferenceSettings {
    fn enabled(&self) -> bool {
        self.0
            .try_state::<PreferencesManager>()
            .and_then(|preferences| preferences.get().ok())
            .is_some_and(|preferences| preferences.remote_enabled)
    }

    fn relay_url(&self) -> Option<String> {
        self.0
            .try_state::<PreferencesManager>()
            .and_then(|preferences| preferences.get().ok())
            .and_then(|preferences| relay_url_preference(&preferences, DEBUG_BUILD))
    }

    fn set_enabled(&self, enabled: bool) -> Result<(), String> {
        self.0
            .try_state::<PreferencesManager>()
            .ok_or_else(|| "Preferences are not available".to_string())?
            .set_remote_enabled(enabled)
            .map(|_| ())
    }

    fn set_relay_url(&self, relay_url: Option<String>) -> Result<(), String> {
        let preferences = self
            .0
            .try_state::<PreferencesManager>()
            .ok_or_else(|| "Preferences are not available".to_string())?;
        store_relay_url_preference(&preferences, relay_url, DEBUG_BUILD)
    }
}

/// Updates the conversation time after remote input, like the `terminal_write` command.
struct AppConversationActivity(AppHandle);

impl ConversationActivity for AppConversationActivity {
    fn touch(&self, conversation_id: &str) {
        if let Some(storage) = self.0.try_state::<Storage>() {
            if let Err(error) = storage.touch_conversation(conversation_id) {
                eprintln!("remote gateway: failed to update conversation activity: {error}");
            }
        }
        if let Err(error) = crate::terminal::emit_conversation_activity(&self.0, conversation_id) {
            eprintln!("remote gateway: failed to emit conversation activity: {error}");
        }
    }
}

/// Sends the gateway events to the main webview.
///
/// `emit_to` does not keep the events from other webviews: a plain `listen` in any webview
/// also gets them. So the gateway never puts the pairing URI or the QR code into an event.
struct WebviewEvents(AppHandle);

impl RemoteEvents for WebviewEvents {
    fn changed(&self, status: &RemoteStatus) {
        let _ = self.0.emit_to(MAIN_WEBVIEW, "remote-changed", status);
    }

    fn pairing_request(&self, request: &RemotePairingRequest) {
        let _ = self
            .0
            .emit_to(MAIN_WEBVIEW, "remote-pairing-request", request);
    }
}

/// The gateway in the Tauri state.
pub(crate) struct RemoteManager {
    gateway: Arc<Gateway>,
}

impl RemoteManager {
    /// Makes the gateway. It needs the storage, the preferences, and the terminal manager.
    pub(crate) fn new(app: &AppHandle) -> RemoteManager {
        let database_path = app.state::<Storage>().database_path().to_path_buf();
        let terminals = Arc::new(app.state::<TerminalManager>().inner().clone());
        let default_relay_url =
            relay_url::default_relay_url(std::env::var(relay_url::RELAY_URL_ENV).ok().as_deref());
        let deps = GatewayDeps {
            settings: Arc::new(PreferenceSettings(app.clone())),
            events: Arc::new(WebviewEvents(app.clone())),
            keychain: Keychain::system(),
            store: Arc::new(SqliteDeviceStore::new(database_path)),
            terminals,
            agent_status: Arc::new(DesktopAgentStatus::new(app.clone())),
            conversations: Arc::new(AppConversationActivity(app.clone())),
            default_relay_url,
            host_name: Arc::new(host_name::computer_name),
            configure: None,
        };
        RemoteManager {
            gateway: Arc::new(Gateway::new(deps)),
        }
    }

    /// Stops the engine at app exit. The agents keep running in the supervisor.
    pub(crate) fn shutdown(&self) {
        let gateway = self.gateway.clone();
        tauri::async_runtime::block_on(async move {
            let _ = tokio::time::timeout(EXIT_TIMEOUT, gateway.shutdown()).await;
        });
    }
}

fn require_main(webview: &Webview) -> Result<(), String> {
    if webview.label() == MAIN_WEBVIEW {
        Ok(())
    } else {
        Err("Only the main Halv window can change Mobile settings.".to_string())
    }
}

/// Removes the pairing offer from a status for any webview other than the main webview.
fn for_caller(mut status: RemoteStatus, webview: &Webview) -> RemoteStatus {
    if webview.label() != MAIN_WEBVIEW {
        status.pairing = None;
    }
    status
}

#[tauri::command]
pub(crate) async fn remote_status(
    webview: Webview,
    remote: State<'_, RemoteManager>,
) -> Result<RemoteStatus, String> {
    Ok(for_caller(remote.gateway.status(), &webview))
}

/// Activates the gateway while the `mobileRemote` feature flag of the main window is on.
/// The app does not start the gateway at launch: the flag decides.
#[tauri::command]
pub(crate) async fn remote_set_active(
    webview: Webview,
    remote: State<'_, RemoteManager>,
    active: bool,
) -> Result<RemoteStatus, String> {
    require_main(&webview)?;
    Ok(for_caller(
        remote.gateway.set_active(active).await,
        &webview,
    ))
}

#[tauri::command]
pub(crate) async fn remote_set_enabled(
    webview: Webview,
    remote: State<'_, RemoteManager>,
    enabled: bool,
) -> Result<RemoteStatus, String> {
    require_main(&webview)?;
    remote.gateway.set_enabled(enabled).await
}

#[tauri::command]
pub(crate) async fn remote_set_relay_url(
    webview: Webview,
    remote: State<'_, RemoteManager>,
    relay_url: Option<String>,
) -> Result<RemoteStatus, String> {
    require_main(&webview)?;
    remote.gateway.set_relay_url(relay_url).await
}

#[tauri::command]
pub(crate) async fn remote_pairing_start(
    webview: Webview,
    remote: State<'_, RemoteManager>,
) -> Result<RemotePairing, String> {
    require_main(&webview)?;
    remote.gateway.pairing_start().await
}

#[tauri::command]
pub(crate) async fn remote_pairing_cancel(
    webview: Webview,
    remote: State<'_, RemoteManager>,
) -> Result<RemoteStatus, String> {
    require_main(&webview)?;
    Ok(remote.gateway.pairing_cancel().await)
}

#[tauri::command]
pub(crate) async fn remote_pairing_respond(
    webview: Webview,
    remote: State<'_, RemoteManager>,
    request_id: String,
    allow: bool,
) -> Result<RemoteStatus, String> {
    require_main(&webview)?;
    remote.gateway.pairing_respond(&request_id, allow).await
}

#[tauri::command]
pub(crate) async fn remote_devices_list(
    remote: State<'_, RemoteManager>,
) -> Result<Vec<RemoteDevice>, String> {
    remote.gateway.devices()
}

#[tauri::command]
pub(crate) async fn remote_device_revoke(
    webview: Webview,
    remote: State<'_, RemoteManager>,
    device_id: String,
) -> Result<RemoteStatus, String> {
    require_main(&webview)?;
    remote.gateway.device_revoke(&device_id).await
}

#[tauri::command]
pub(crate) async fn remote_identity_reset(
    webview: Webview,
    remote: State<'_, RemoteManager>,
) -> Result<RemoteStatus, String> {
    require_main(&webview)?;
    remote.gateway.identity_reset().await
}

#[cfg(test)]
mod preference_tests {
    use super::*;

    #[test]
    fn each_build_profile_keeps_its_own_relay_url() {
        let directory =
            std::env::temp_dir().join(format!("halv-remote-relay-{}", rand::random::<u64>()));
        let preferences = PreferencesManager::open(&directory).expect("open the preferences");

        // A debug build stores a local relay. A release build never reads it.
        store_relay_url_preference(&preferences, Some("ws://127.0.0.1:8791".to_string()), true)
            .unwrap();
        let stored = preferences.get().unwrap();
        assert_eq!(
            relay_url_preference(&stored, true).as_deref(),
            Some("ws://127.0.0.1:8791")
        );
        assert_eq!(relay_url_preference(&stored, false), None);

        store_relay_url_preference(
            &preferences,
            Some("wss://own.example.test".to_string()),
            false,
        )
        .unwrap();
        let stored = preferences.get().unwrap();
        assert_eq!(
            relay_url_preference(&stored, false).as_deref(),
            Some("wss://own.example.test")
        );
        assert_eq!(
            relay_url_preference(&stored, true).as_deref(),
            Some("ws://127.0.0.1:8791")
        );

        // The file keeps one key for each build profile.
        let file = std::fs::read_to_string(directory.join("preferences.json")).unwrap();
        let json: serde_json::Value = serde_json::from_str(&file).unwrap();
        assert_eq!(json["remoteRelayUrlDev"], "ws://127.0.0.1:8791");
        assert_eq!(json["remoteRelayUrl"], "wss://own.example.test");

        store_relay_url_preference(&preferences, None, true).unwrap();
        assert_eq!(
            relay_url_preference(&preferences.get().unwrap(), true),
            None
        );
        let _ = std::fs::remove_dir_all(&directory);
    }
}
