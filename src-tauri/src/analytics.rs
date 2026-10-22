use crate::{
    app_data, auth,
    preferences::{AnalyticsConsent, PreferencesManager},
    storage::Storage,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};
use tauri::{AppHandle, Emitter, Manager};

const INSTALL_ID_FILE_NAME: &str = "analytics-install-id";
const QUEUE_FILE_NAME: &str = "analytics-queue.json";
/// The download id of the site link that led to this install (ADR 0021).
const ATTRIBUTION_FILE_NAME: &str = "attribution.json";
/// The app tells the webview about a new download id with this event.
pub const ATTRIBUTION_EVENT: &str = "halv://attribution";
const FIRST_OPEN: &str = "first_open";
const DOWNLOAD_LINK_SCHEME: &str = "halv";
const DOWNLOAD_LINK_HOST: &str = "welcome";
const DOWNLOAD_ID_PARAMETER: &str = "d";
const DOWNLOAD_ID_MAX_LENGTH: usize = 64;
const ANALYTICS_EVENTS_PATH: &str = "/v1/analytics/events";
const JOURNEY_EVENTS_PATH: &str = "/v1/analytics/journey";
const JOURNEY_EVENTS: [&str; 13] = [
    "first_open",
    "account_connected",
    "project_selected",
    "first_task_started",
    "first_task_completed",
    "first_task_failed",
    "setup_failed",
    "context_saved",
    "trial_prompted",
    "checkout_opened",
    "trial_confirmed",
    "payment_confirmed",
    "active_day",
];
const ALLOWED_EVENTS: [&str; 5] = [
    "installed",
    "cli_connected",
    "first_saving",
    "trial_started",
    "paid",
];

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct AnalyticsEvent {
    event: String,
    at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    category: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    app_version: Option<String>,
    /// The download id of the site link. Only `first_open` has it (ADR 0021).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    download_id: Option<String>,
}

impl AnalyticsEvent {
    fn key(&self) -> String {
        if self.event == "active_day" {
            format!("active_day:{}", self.at / 86_400)
        } else {
            self.event.clone()
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default)]
struct AnalyticsQueue {
    pending: Vec<AnalyticsEvent>,
    recorded: BTreeSet<String>,
}

impl AnalyticsQueue {
    fn record(
        &mut self,
        event: &str,
        at: u64,
        category: Option<String>,
        app_version: Option<String>,
        download_id: Option<String>,
    ) -> Result<bool, String> {
        validate_event(event)?;
        validate_category(event, category.as_deref())?;
        validate_download_id(event, download_id.as_deref())?;
        let value = AnalyticsEvent {
            event: event.to_string(),
            at,
            category,
            app_version,
            download_id,
        };
        let key = value.key();
        if self.recorded.contains(&key) {
            return Ok(false);
        }

        if event == "active_day" {
            self.recorded.retain(|key| !key.starts_with("active_day:"));
        }
        self.recorded.insert(key);
        self.pending
            .retain(|value| value.at.saturating_add(30 * 86_400) >= at);
        self.pending.push(value);
        Ok(true)
    }

    fn acknowledge(&mut self, sent: &[AnalyticsEvent]) {
        let sent_names = sent
            .iter()
            .map(AnalyticsEvent::key)
            .collect::<BTreeSet<_>>();
        self.pending
            .retain(|event| !sent_names.contains(&event.key()));
    }

    fn validate(&self) -> Result<(), String> {
        let mut pending_names = BTreeSet::new();
        for event in &self.pending {
            validate_event(&event.event)?;
            validate_category(&event.event, event.category.as_deref())?;
            validate_download_id(&event.event, event.download_id.as_deref())?;
            if !pending_names.insert(event.key()) {
                return Err(format!("Duplicate queued analytics event: {}", event.event));
            }
            if event.event != "active_day" && !self.recorded.contains(&event.key()) {
                return Err(format!(
                    "Queued analytics event is not marked as recorded: {}",
                    event.event
                ));
            }
        }
        for event in &self.recorded {
            if let Some(day) = event.strip_prefix("active_day:") {
                day.parse::<u64>()
                    .map_err(|_| "Invalid activity day".to_string())?;
            } else {
                validate_event(event)?;
            }
        }
        Ok(())
    }

    /// True after `first_open` left the queue. A download id that arrives
    /// later cannot travel with it anymore.
    fn first_open_sent(&self) -> bool {
        self.recorded.contains(FIRST_OPEN)
            && !self.pending.iter().any(|event| event.event == FIRST_OPEN)
    }
}

/// The download id that a site link gave to this install (ADR 0021).
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct Attribution {
    download_id: String,
    received_at: u64,
}

/// What the webview may show about the download id.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AttributionSnapshot {
    pub download_id: Option<String>,
}

#[derive(Clone, Debug)]
pub struct AnalyticsManager {
    install_id: Arc<str>,
    queue_path: Arc<PathBuf>,
    queue: Arc<Mutex<AnalyticsQueue>>,
    attribution_path: Arc<PathBuf>,
    // Lock order: `queue` first, then `attribution`.
    attribution: Arc<Mutex<Option<Attribution>>>,
    flush_lock: Arc<tokio::sync::Mutex<()>>,
}

impl AnalyticsManager {
    pub fn open(app_data_dir: &Path) -> io::Result<Self> {
        fs::create_dir_all(app_data_dir)?;
        let queue_path = app_data_dir.join(QUEUE_FILE_NAME);
        let attribution_path = app_data_dir.join(ATTRIBUTION_FILE_NAME);
        // An earlier release created these files with the default mode.
        app_data::restrict_file(&app_data_dir.join(INSTALL_ID_FILE_NAME));
        app_data::restrict_file(&queue_path);
        let install_id = read_or_create_install_id(app_data_dir)?;
        let queue = read_queue(&queue_path)?;
        queue
            .validate()
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        let attribution = read_attribution(&attribution_path);

        Ok(Self {
            install_id: install_id.into(),
            queue_path: Arc::new(queue_path),
            queue: Arc::new(Mutex::new(queue)),
            attribution_path: Arc::new(attribution_path),
            attribution: Arc::new(Mutex::new(attribution)),
            flush_lock: Arc::new(tokio::sync::Mutex::new(())),
        })
    }

    fn install_id(&self) -> String {
        self.install_id.to_string()
    }

    #[cfg(test)]
    fn record(&self, event: &str, at: u64) -> Result<bool, String> {
        self.record_with_context(event, at, None, None)
    }

    fn record_with_context(
        &self,
        event: &str,
        at: u64,
        category: Option<String>,
        app_version: Option<String>,
    ) -> Result<bool, String> {
        let mut queue = self
            .queue
            .lock()
            .map_err(|_| "Analytics queue lock is poisoned".to_string())?;
        let download_id = if event == FIRST_OPEN {
            self.download_id()?
        } else {
            None
        };
        let mut next = queue.clone();
        let recorded = next.record(event, at, category, app_version, download_id)?;
        if recorded {
            write_queue_atomically(&self.queue_path, &next).map_err(|error| error.to_string())?;
            *queue = next;
        }
        Ok(recorded)
    }

    fn pending(&self) -> Result<Vec<AnalyticsEvent>, String> {
        self.queue
            .lock()
            .map(|queue| queue.pending.clone())
            .map_err(|_| "Analytics queue lock is poisoned".to_string())
    }

    fn expire_pending(&self, now: u64) -> Result<(), String> {
        let mut queue = self
            .queue
            .lock()
            .map_err(|_| "Analytics queue lock is poisoned".to_string())?;
        let mut next = queue.clone();
        next.pending
            .retain(|event| event.at.saturating_add(30 * 86_400) >= now);
        if next.pending.len() != queue.pending.len() {
            write_queue_atomically(&self.queue_path, &next).map_err(|error| error.to_string())?;
            *queue = next;
        }
        Ok(())
    }

    fn acknowledge(&self, sent: &[AnalyticsEvent]) -> Result<(), String> {
        let mut queue = self
            .queue
            .lock()
            .map_err(|_| "Analytics queue lock is poisoned".to_string())?;
        let mut next = queue.clone();
        next.acknowledge(sent);
        write_queue_atomically(&self.queue_path, &next).map_err(|error| error.to_string())?;
        *queue = next;
        // The server has `first_open` now. The download id has done its job.
        if sent.iter().any(|event| event.event == FIRST_OPEN) {
            self.forget_download_id()?;
        }
        Ok(())
    }

    /// The stored download id, when a site link gave one.
    pub(crate) fn download_id(&self) -> Result<Option<String>, String> {
        self.attribution
            .lock()
            .map(|attribution| {
                attribution
                    .as_ref()
                    .map(|attribution| attribution.download_id.clone())
            })
            .map_err(|_| "Analytics attribution lock is poisoned".to_string())
    }

    /// Stores the download id of a site link, also without consent (ADR
    /// 0021). The first id wins. An id that arrives after `first_open` left
    /// the queue is dropped. Returns true when Halv stored the id.
    pub(crate) fn accept_download_id(&self, download_id: &str, now: u64) -> Result<bool, String> {
        if !is_valid_download_id(download_id) {
            return Err("Invalid download id".to_string());
        }
        let mut queue = self
            .queue
            .lock()
            .map_err(|_| "Analytics queue lock is poisoned".to_string())?;
        let mut attribution = self
            .attribution
            .lock()
            .map_err(|_| "Analytics attribution lock is poisoned".to_string())?;
        if attribution.is_some() || queue.first_open_sent() {
            return Ok(false);
        }
        let stored = Attribution {
            download_id: download_id.to_string(),
            received_at: now,
        };
        write_attribution_atomically(&self.attribution_path, &stored)
            .map_err(|error| error.to_string())?;
        *attribution = Some(stored);

        // A `first_open` that still waits in the queue takes the id with it.
        let mut next = queue.clone();
        let mut changed = false;
        for event in next
            .pending
            .iter_mut()
            .filter(|event| event.event == FIRST_OPEN && event.download_id.is_none())
        {
            event.download_id = Some(download_id.to_string());
            changed = true;
        }
        if changed {
            write_queue_atomically(&self.queue_path, &next).map_err(|error| error.to_string())?;
            *queue = next;
        }
        Ok(true)
    }

    fn forget_download_id(&self) -> Result<(), String> {
        let mut attribution = self
            .attribution
            .lock()
            .map_err(|_| "Analytics attribution lock is poisoned".to_string())?;
        if attribution.is_none() {
            return Ok(());
        }
        match fs::remove_file(self.attribution_path.as_path()) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.to_string()),
        }
        *attribution = None;
        Ok(())
    }

    pub(crate) fn clear_pending(&self) -> Result<(), String> {
        let mut queue = self
            .queue
            .lock()
            .map_err(|_| "Analytics queue lock is poisoned".to_string())?;
        if queue.pending.is_empty() {
            return Ok(());
        }
        // The in-memory queue drops its events first, so a failed write
        // never leaves an event to send in this session.
        queue.pending.clear();
        write_queue_atomically(&self.queue_path, &queue).map_err(|error| error.to_string())
    }
}

#[derive(Serialize)]
struct AnalyticsBatch<'a> {
    install_id: &'a str,
    app_version: &'a str,
    os: &'a str,
    events: &'a [AnalyticsEvent],
}

fn validate_event(event: &str) -> Result<(), String> {
    if ALLOWED_EVENTS.contains(&event) || JOURNEY_EVENTS.contains(&event) {
        Ok(())
    } else {
        Err(format!("Unknown analytics event: {event}"))
    }
}

fn validate_download_id(event: &str, download_id: Option<&str>) -> Result<(), String> {
    match download_id {
        None => Ok(()),
        Some(value) if event == FIRST_OPEN && is_valid_download_id(value) => Ok(()),
        Some(_) => Err("Invalid analytics download id".to_string()),
    }
}

/// A download id is opaque: 1 to 64 characters of `[A-Za-z0-9_-]`.
fn is_valid_download_id(value: &str) -> bool {
    (1..=DOWNLOAD_ID_MAX_LENGTH).contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

/// Returns the download id of a `halv://welcome?d=<download_id>` link. Any
/// other URL, or a link with a bad or repeated id, gives `None`.
pub(crate) fn download_id_from_url(url: &url::Url) -> Option<String> {
    let host_matches = url
        .host_str()
        .is_some_and(|host| host.eq_ignore_ascii_case(DOWNLOAD_LINK_HOST));
    if url.scheme() != DOWNLOAD_LINK_SCHEME
        || !host_matches
        || !matches!(url.path(), "" | "/")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
    {
        return None;
    }
    let mut values = url
        .query_pairs()
        .filter(|(key, _)| key == DOWNLOAD_ID_PARAMETER)
        .map(|(_, value)| value.into_owned());
    let download_id = values.next()?;
    if values.next().is_some() {
        return None;
    }
    is_valid_download_id(&download_id).then_some(download_id)
}

fn validate_category(event: &str, category: Option<&str>) -> Result<(), String> {
    if category.is_some_and(|value| {
        !matches!(event, "first_task_failed" | "setup_failed")
            || !matches!(value, "connection" | "engine" | "task" | "cancelled")
    }) {
        return Err("Invalid analytics failure category".to_string());
    }
    Ok(())
}

fn unix_time_seconds() -> Result<u64, String> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .map_err(|error| format!("System time is before the Unix epoch: {error}"))
}

fn analytics_os() -> Result<&'static str, String> {
    match std::env::consts::OS {
        "macos" => Ok("macos"),
        "windows" => Ok("windows"),
        "linux" => Ok("linux"),
        os => Err(format!("Unsupported analytics operating system: {os}")),
    }
}

fn read_or_create_install_id(app_data_dir: &Path) -> io::Result<String> {
    let path = app_data_dir.join(INSTALL_ID_FILE_NAME);
    match fs::read_to_string(&path) {
        Ok(contents) => {
            let install_id = contents.trim();
            if is_uuid_v4(install_id) {
                return Ok(install_id.to_string());
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }

    let install_id = new_uuid_v4();
    write_text_atomically(&path, &format!("{install_id}\n"))?;
    Ok(install_id)
}

fn new_uuid_v4() -> String {
    let mut bytes = rand::random::<[u8; 16]>();
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[0],
        bytes[1],
        bytes[2],
        bytes[3],
        bytes[4],
        bytes[5],
        bytes[6],
        bytes[7],
        bytes[8],
        bytes[9],
        bytes[10],
        bytes[11],
        bytes[12],
        bytes[13],
        bytes[14],
        bytes[15],
    )
}

fn is_uuid_v4(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() == 36
        && bytes[8] == b'-'
        && bytes[13] == b'-'
        && bytes[14] == b'4'
        && bytes[18] == b'-'
        && matches!(bytes[19], b'8' | b'9' | b'a' | b'b' | b'A' | b'B')
        && bytes[23] == b'-'
        && bytes
            .iter()
            .enumerate()
            .all(|(index, byte)| matches!(index, 8 | 13 | 18 | 23) || byte.is_ascii_hexdigit())
}

fn read_queue(path: &Path) -> io::Result<AnalyticsQueue> {
    match fs::read(path) {
        Ok(contents) => serde_json::from_slice(&contents)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(AnalyticsQueue::default()),
        Err(error) => Err(error),
    }
}

/// Reads the stored download id. A damaged file counts as no id, so it never
/// stops the app from starting.
fn read_attribution(path: &Path) -> Option<Attribution> {
    app_data::restrict_file(path);
    let contents = fs::read(path).ok()?;
    serde_json::from_slice::<Attribution>(&contents)
        .ok()
        .filter(|attribution| is_valid_download_id(&attribution.download_id))
}

fn write_attribution_atomically(path: &Path, attribution: &Attribution) -> io::Result<()> {
    let mut serialized = serde_json::to_vec_pretty(attribution)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    serialized.push(b'\n');
    write_bytes_atomically(path, &serialized)
}

fn write_queue_atomically(path: &Path, queue: &AnalyticsQueue) -> io::Result<()> {
    let mut serialized = serde_json::to_vec_pretty(queue)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    serialized.push(b'\n');
    write_bytes_atomically(path, &serialized)
}

fn write_text_atomically(path: &Path, contents: &str) -> io::Result<()> {
    write_bytes_atomically(path, contents.as_bytes())
}

fn write_bytes_atomically(path: &Path, contents: &[u8]) -> io::Result<()> {
    let temporary_path = path.with_extension("tmp");
    let result = (|| {
        let mut temporary = app_data::private_file_options()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&temporary_path)?;
        temporary.write_all(contents)?;
        temporary.sync_all()?;
        drop(temporary);
        replace_file(&temporary_path, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary_path);
    }
    result
}

#[cfg(not(windows))]
fn replace_file(temporary_path: &Path, path: &Path) -> io::Result<()> {
    fs::rename(temporary_path, path)
}

#[cfg(windows)]
fn replace_file(temporary_path: &Path, path: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
    };

    let temporary = temporary_path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let destination = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let replaced = unsafe {
        MoveFileExW(
            temporary.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if replaced == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(test)]
async fn flush_pending(
    manager: &AnalyticsManager,
    server_url: &str,
    app_version: &str,
) -> Result<(), String> {
    flush_pending_guarded(manager, server_url, app_version, &|| Ok(true)).await
}

async fn flush_pending_guarded(
    manager: &AnalyticsManager,
    server_url: &str,
    app_version: &str,
    can_send: &(dyn Fn() -> Result<bool, String> + Sync),
) -> Result<(), String> {
    let _flush_guard = manager.flush_lock.lock().await;
    if !can_send()? {
        return manager.clear_pending();
    }
    manager.expire_pending(unix_time_seconds()?)?;
    let events = manager.pending()?;
    if events.is_empty() {
        return Ok(());
    }

    // Separate the corrected journey from legacy definitions. An older server
    // returns 404 here: retain the queue until the coordinated server rollout.
    for (path, selected) in [
        (
            ANALYTICS_EVENTS_PATH,
            events
                .iter()
                .filter(|event| ALLOWED_EVENTS.contains(&event.event.as_str()))
                .cloned()
                .collect::<Vec<_>>(),
        ),
        (
            JOURNEY_EVENTS_PATH,
            events
                .iter()
                .filter(|event| JOURNEY_EVENTS.contains(&event.event.as_str()))
                .cloned()
                .collect::<Vec<_>>(),
        ),
    ] {
        for batch in selected.chunks(10) {
            // A preference change also stops later batches of a running flush.
            if !can_send()? {
                return manager.clear_pending();
            }
            let mut status =
                post_batch(server_url, path, &manager.install_id, app_version, batch).await?;
            // A server that does not know `download_id` rejects the batch
            // with HTTP 400. Send the batch again without the id, so the
            // journey does not stop (ADR 0021).
            if status == reqwest::StatusCode::BAD_REQUEST
                && batch.iter().any(|event| event.download_id.is_some())
            {
                if !can_send()? {
                    return manager.clear_pending();
                }
                let without_download_id = batch
                    .iter()
                    .cloned()
                    .map(|event| AnalyticsEvent {
                        download_id: None,
                        ..event
                    })
                    .collect::<Vec<_>>();
                status = post_batch(
                    server_url,
                    path,
                    &manager.install_id,
                    app_version,
                    &without_download_id,
                )
                .await?;
            }
            if !status.is_success() {
                return Err(format!("Analytics request returned HTTP {status}"));
            }
            manager.acknowledge(batch)?;
        }
    }
    Ok(())
}

async fn post_batch(
    server_url: &str,
    path: &str,
    install_id: &str,
    app_version: &str,
    events: &[AnalyticsEvent],
) -> Result<reqwest::StatusCode, String> {
    auth::api_client()
        .post(auth::api_url(server_url, path))
        .json(&AnalyticsBatch {
            install_id,
            app_version,
            os: analytics_os()?,
            events,
        })
        .send()
        .await
        .map(|response| response.status())
        .map_err(|error| format!("Analytics request failed: {error}"))
}

/// Records one event only when the user agreed to usage events (ADR 0021).
/// Any other answer drops the event and the queue, and does not mark the
/// event as recorded. Returns true when sharing is on.
fn record_if_consented(
    manager: &AnalyticsManager,
    consent: AnalyticsConsent,
    event: &str,
    at: u64,
    category: Option<String>,
    app_version: Option<String>,
) -> Result<bool, String> {
    validate_event(event)?;
    if consent != AnalyticsConsent::On {
        manager.clear_pending()?;
        return Ok(false);
    }
    manager.record_with_context(event, at, category, app_version)?;
    Ok(true)
}

/// Reads the site links that open Halv (ADR 0021): the link that started the
/// app and each later link. The deep-link plugin must be registered.
pub fn listen_for_download_links(app: &AppHandle) {
    use tauri_plugin_deep_link::DeepLinkExt;

    let deep_link = app.deep_link();
    if needs_runtime_scheme_registration(
        std::env::consts::OS,
        runs_as_appimage(app),
        cfg!(debug_assertions),
    ) {
        if let Err(error) = deep_link.register_all() {
            log_failure(&format!(
                "Halv could not register the halv:// link: {error}"
            ));
        }
    }
    if let Ok(Some(urls)) = deep_link.get_current() {
        accept_download_links(app, &urls);
    }
    let handle = app.clone();
    deep_link.on_open_url(move |event| accept_download_links(&handle, &event.urls()));
}

/// Tells if Halv must register the halv:// scheme when it starts. The MSI,
/// NSIS, deb and rpm installers register it from `tauri.conf.json`. An
/// AppImage and a dev build have no installer. macOS registers the scheme
/// from the app bundle and cannot register it at runtime.
fn needs_runtime_scheme_registration(os: &str, appimage: bool, debug_build: bool) -> bool {
    match os {
        "linux" => appimage || debug_build,
        "windows" => debug_build,
        _ => false,
    }
}

#[cfg(target_os = "linux")]
fn runs_as_appimage(app: &AppHandle) -> bool {
    app.env().appimage.is_some()
}

#[cfg(not(target_os = "linux"))]
fn runs_as_appimage(_app: &AppHandle) -> bool {
    false
}

fn accept_download_links(app: &AppHandle, urls: &[url::Url]) {
    let Some(download_id) = urls.iter().find_map(download_id_from_url) else {
        return;
    };
    let Some(manager) = app.try_state::<AnalyticsManager>() else {
        return;
    };
    let accepted =
        unix_time_seconds().and_then(|now| manager.accept_download_id(&download_id, now));
    match accepted {
        Ok(true) => {
            let _ = app.emit(
                ATTRIBUTION_EVENT,
                AttributionSnapshot {
                    download_id: Some(download_id),
                },
            );
        }
        Ok(false) => {}
        Err(error) => log_failure(&error),
    }
}

async fn flush_for_app(app: &AppHandle) -> Result<(), String> {
    let manager = app.state::<AnalyticsManager>().inner().clone();
    let preferences = app.state::<PreferencesManager>();
    if !preferences.analytics_enabled()? {
        return manager.clear_pending();
    }
    let server_url =
        auth::auth_server_url(&app.state::<Storage>()).map_err(|error| error.to_string())?;
    let app_version = app.package_info().version.to_string();
    flush_pending_guarded(&manager, &server_url, &app_version, &|| {
        preferences.analytics_enabled()
    })
    .await
}

fn log_failure(error: &str) {
    #[cfg(debug_assertions)]
    eprintln!("{error}");
    #[cfg(not(debug_assertions))]
    let _ = error;
}

#[tauri::command]
pub async fn analytics_record(
    app: AppHandle,
    event: String,
    category: Option<String>,
) -> Result<(), String> {
    validate_event(&event)?;
    let manager = app.state::<AnalyticsManager>().inner().clone();
    let consent = app.state::<PreferencesManager>().analytics_consent()?;
    let sharing = record_if_consented(
        &manager,
        consent,
        &event,
        unix_time_seconds()?,
        category,
        Some(app.package_info().version.to_string()),
    )?;
    if !sharing {
        return Ok(());
    }
    tauri::async_runtime::spawn(async move {
        if let Err(error) = flush_for_app(&app).await {
            log_failure(&error);
        }
    });
    Ok(())
}

#[tauri::command]
pub async fn analytics_flush(app: AppHandle) -> Result<(), String> {
    let result = flush_for_app(&app).await;
    if let Err(error) = &result {
        log_failure(error);
    }
    result
}

#[tauri::command]
pub fn analytics_install_id(manager: tauri::State<'_, AnalyticsManager>) -> String {
    manager.install_id()
}

/// The stored download id. It stays `None` after `first_open` reached the
/// server, because Halv then deletes it.
#[tauri::command]
pub fn analytics_attribution_get(
    manager: tauri::State<'_, AnalyticsManager>,
) -> Result<AttributionSnapshot, String> {
    Ok(AttributionSnapshot {
        download_id: manager.download_id()?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir()
                .join(format!("halv-analytics-{name}-{}", rand::random::<u64>()));
            fs::create_dir_all(&path).expect("create analytics test directory");
            Self(path)
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn allowlist_accepts_only_funnel_events() {
        for event in ALLOWED_EVENTS {
            assert!(validate_event(event).is_ok(), "{event} should be allowed");
        }
        assert!(validate_event("signed_in").is_err());
        assert!(validate_event("").is_err());
    }

    #[test]
    fn queue_dedupes_and_persists_recorded_events() {
        let directory = TestDirectory::new("queue-dedupe");
        let manager = AnalyticsManager::open(&directory.0).expect("open analytics manager");
        assert!(manager.record("installed", 10).expect("record event"));
        assert!(!manager.record("installed", 20).expect("dedupe event"));
        assert_eq!(
            manager.pending().expect("read pending events"),
            vec![AnalyticsEvent {
                event: "installed".to_string(),
                at: 10,
                category: None,
                app_version: None,
                download_id: None,
            }]
        );

        let reopened = AnalyticsManager::open(&directory.0).expect("reopen analytics manager");
        assert!(!reopened.record("installed", 30).expect("dedupe event"));
        assert_eq!(reopened.pending().expect("read pending events").len(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn analytics_files_are_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let directory = TestDirectory::new("modes");
        let files = [
            directory.0.join(INSTALL_ID_FILE_NAME),
            directory.0.join(QUEUE_FILE_NAME),
        ];
        let mode = |path: &Path| {
            fs::metadata(path)
                .expect("analytics file should exist")
                .permissions()
                .mode()
                & 0o777
        };
        let manager = AnalyticsManager::open(&directory.0).expect("open analytics manager");
        manager.record("installed", 10).expect("record event");
        for file in &files {
            assert_eq!(mode(file), 0o600, "{}", file.display());
        }

        // An earlier release left the files readable by others.
        for file in &files {
            fs::set_permissions(file, fs::Permissions::from_mode(0o644)).expect("change mode");
        }
        AnalyticsManager::open(&directory.0).expect("reopen analytics manager");
        for file in &files {
            assert_eq!(mode(file), 0o600, "{}", file.display());
        }
    }

    #[test]
    fn acknowledging_events_clears_pending_but_keeps_dedupe_state() {
        let directory = TestDirectory::new("queue-acknowledge");
        let manager = AnalyticsManager::open(&directory.0).expect("open analytics manager");
        manager.record("installed", 10).expect("record installed");
        manager
            .record("cli_connected", 20)
            .expect("record CLI connection");
        let sent = vec![manager.pending().expect("read pending events")[0].clone()];
        manager.acknowledge(&sent).expect("acknowledge event");

        let reopened = AnalyticsManager::open(&directory.0).expect("reopen analytics manager");
        assert_eq!(
            reopened.pending().expect("read pending events"),
            vec![AnalyticsEvent {
                event: "cli_connected".to_string(),
                at: 20,
                category: None,
                app_version: None,
                download_id: None,
            }]
        );
        assert!(!reopened.record("installed", 30).expect("dedupe event"));
    }

    #[test]
    fn install_id_is_v4_and_persists() {
        let directory = TestDirectory::new("install-id");
        let first = AnalyticsManager::open(&directory.0)
            .expect("open analytics manager")
            .install_id();
        assert!(is_uuid_v4(&first));
        let second = AnalyticsManager::open(&directory.0)
            .expect("reopen analytics manager")
            .install_id();
        assert_eq!(first, second);
    }
    #[test]
    fn daily_activity_dedupes_per_day_and_retains_unsent_days() {
        let directory = TestDirectory::new("activity-days");
        let manager = AnalyticsManager::open(&directory.0).unwrap();
        assert!(manager.record("active_day", 100 * 86_400).unwrap());
        assert!(!manager.record("active_day", 100 * 86_400 + 42).unwrap());
        assert!(manager.record("active_day", 101 * 86_400).unwrap());
        let reopened = AnalyticsManager::open(&directory.0).unwrap();
        let pending = reopened.pending().unwrap();
        assert_eq!(pending.len(), 2);
        reopened.acknowledge(&pending[..1]).unwrap();
        assert_eq!(reopened.pending().unwrap()[0].at, 101 * 86_400);
    }

    #[test]
    fn queued_journey_keeps_recorded_version_and_rejects_error_text() {
        let directory = TestDirectory::new("journey-context");
        let manager = AnalyticsManager::open(&directory.0).unwrap();
        manager
            .record_with_context("first_task_completed", 100, None, Some("0.6.0".into()))
            .unwrap();
        assert_eq!(
            manager.pending().unwrap()[0].app_version.as_deref(),
            Some("0.6.0")
        );
        assert!(manager
            .record_with_context("first_task_failed", 101, Some("/private/path".into()), None)
            .is_err());
        manager
            .record_with_context("first_task_failed", 101, Some("engine".into()), None)
            .unwrap();
        assert_eq!(manager.pending().unwrap().len(), 2);
        manager.clear_pending().unwrap();
        assert!(manager.pending().unwrap().is_empty());
    }

    #[test]
    fn expired_queue_is_removed_before_retry_and_stays_removed_after_restart() {
        let directory = TestDirectory::new("expired-queue");
        let manager = AnalyticsManager::open(&directory.0).unwrap();
        manager.record("first_open", 100).unwrap();
        manager.record("first_task_started", 200).unwrap();
        manager.expire_pending(100 + 30 * 86_400 + 1).unwrap();
        let reopened = AnalyticsManager::open(&directory.0).unwrap();
        assert_eq!(reopened.pending().unwrap().len(), 1);
        assert_eq!(reopened.pending().unwrap()[0].event, "first_task_started");
        assert!(!reopened
            .record("first_open", 100 + 30 * 86_400 + 2)
            .unwrap());
    }

    #[tokio::test]
    async fn disabled_sharing_drops_queue_without_any_network_request() {
        let directory = TestDirectory::new("privacy-disabled");
        let manager = AnalyticsManager::open(&directory.0).unwrap();
        manager
            .record("first_open", unix_time_seconds().unwrap())
            .unwrap();
        flush_pending_guarded(&manager, "http://127.0.0.1:1", "0.6.0", &|| Ok(false))
            .await
            .unwrap();
        assert!(AnalyticsManager::open(&directory.0)
            .unwrap()
            .pending()
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn old_server_keeps_new_journey_queued_for_rollout_retry() {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                .unwrap();
            let mut buffer = [0; 8192];
            let size = stream.read(&mut buffer).unwrap();
            assert!(
                String::from_utf8_lossy(&buffer[..size]).starts_with("POST /v1/analytics/journey ")
            );
            stream
                .write_all(
                    b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                )
                .unwrap();
        });
        let directory = TestDirectory::new("old-server");
        let manager = AnalyticsManager::open(&directory.0).unwrap();
        manager
            .record("first_open", unix_time_seconds().unwrap())
            .unwrap();
        assert!(flush_pending(&manager, &url, "0.6.0").await.is_err());
        server.join().unwrap();
        assert_eq!(manager.pending().unwrap().len(), 1);
    }

    fn link(value: &str) -> url::Url {
        url::Url::parse(value).unwrap()
    }

    #[test]
    fn download_link_parser_accepts_only_a_valid_welcome_link() {
        let max = "a".repeat(DOWNLOAD_ID_MAX_LENGTH);
        let accepted = [
            ("halv://welcome?d=dl_8Hc-2", "dl_8Hc-2"),
            ("halv://welcome/?d=abc", "abc"),
            ("halv://WELCOME?d=abc", "abc"),
            ("halv://welcome?utm_source=site&d=abc", "abc"),
            ("halv://welcome?d=abc#top", "abc"),
        ];
        for (value, expected) in accepted {
            assert_eq!(
                download_id_from_url(&link(value)).as_deref(),
                Some(expected),
                "{value}"
            );
        }
        assert_eq!(
            download_id_from_url(&link(&format!("halv://welcome?d={max}"))),
            Some(max.clone())
        );

        let rejected = [
            // Bad characters, also after percent decoding.
            "halv://welcome?d=abc.def",
            "halv://welcome?d=abc%20def",
            "halv://welcome?d=abc%2Fdef",
            "halv://welcome?d=%C3%A9t%C3%A9",
            "halv://welcome?d=",
            // Too long.
            &format!("halv://welcome?d={max}a"),
            // Wrong host, path, scheme or authority.
            "halv://settings?d=abc",
            "halv://welcome/extra?d=abc",
            "https://welcome?d=abc",
            "https://halv.ai/welcome?d=abc",
            "halv://user@welcome?d=abc",
            "halv://welcome:8080?d=abc",
            "halv:welcome?d=abc",
            // No query, no id, or a repeated id.
            "halv://welcome",
            "halv://welcome?download=abc",
            "halv://welcome?d=abc&d=def",
        ];
        for value in rejected {
            assert_eq!(download_id_from_url(&link(value)), None, "{value}");
        }
    }

    #[test]
    fn only_installs_without_an_installer_register_the_link_scheme() {
        assert!(needs_runtime_scheme_registration("linux", true, false));
        assert!(needs_runtime_scheme_registration("linux", false, true));
        assert!(needs_runtime_scheme_registration("windows", false, true));
        assert!(!needs_runtime_scheme_registration("linux", false, false));
        assert!(!needs_runtime_scheme_registration("windows", false, false));
        assert!(!needs_runtime_scheme_registration("macos", false, true));
        assert!(!needs_runtime_scheme_registration("macos", false, false));
    }

    #[test]
    fn unset_or_declined_consent_drops_events_without_marking_them() {
        let directory = TestDirectory::new("consent-gate");
        let manager = AnalyticsManager::open(&directory.0).unwrap();
        for consent in [AnalyticsConsent::Unset, AnalyticsConsent::Off] {
            assert!(!record_if_consented(&manager, consent, FIRST_OPEN, 100, None, None).unwrap());
            assert!(manager.pending().unwrap().is_empty());
        }
        assert!(record_if_consented(
            &manager,
            AnalyticsConsent::Unset,
            "not_an_event",
            100,
            None,
            None
        )
        .is_err());

        // A later yes still records the event that the unset answer dropped.
        assert!(
            record_if_consented(&manager, AnalyticsConsent::On, FIRST_OPEN, 200, None, None)
                .unwrap()
        );
        assert_eq!(manager.pending().unwrap().len(), 1);
        assert_eq!(manager.pending().unwrap()[0].at, 200);

        // Unset again drops the queue.
        record_if_consented(
            &manager,
            AnalyticsConsent::Unset,
            "active_day",
            300,
            None,
            None,
        )
        .unwrap();
        assert!(manager.pending().unwrap().is_empty());
    }

    #[test]
    fn download_id_is_stored_without_consent_and_sent_with_first_open() {
        let directory = TestDirectory::new("attribution-before-consent");
        let manager = AnalyticsManager::open(&directory.0).unwrap();
        assert_eq!(manager.download_id().unwrap(), None);
        assert!(manager.accept_download_id("bad id", 10).is_err());

        // The link arrives while the answer is unset. Halv stores the id.
        assert!(
            !record_if_consented(&manager, AnalyticsConsent::Unset, FIRST_OPEN, 5, None, None)
                .unwrap()
        );
        assert!(manager.accept_download_id("site-download_1", 10).unwrap());
        // The first id wins.
        assert!(!manager.accept_download_id("site-download_2", 11).unwrap());
        let reopened = AnalyticsManager::open(&directory.0).unwrap();
        assert_eq!(
            reopened.download_id().unwrap().as_deref(),
            Some("site-download_1")
        );

        // The user agrees later. `first_open` carries the id.
        record_if_consented(&reopened, AnalyticsConsent::On, FIRST_OPEN, 20, None, None).unwrap();
        record_if_consented(
            &reopened,
            AnalyticsConsent::On,
            "active_day",
            20,
            None,
            None,
        )
        .unwrap();
        let pending = reopened.pending().unwrap();
        assert_eq!(pending[0].download_id.as_deref(), Some("site-download_1"));
        assert_eq!(pending[1].download_id, None);
        let serialized = serde_json::to_value(&pending[0]).unwrap();
        assert_eq!(serialized["download_id"], "site-download_1");
        assert!(serde_json::to_value(&pending[1])
            .unwrap()
            .get("download_id")
            .is_none());

        // After the server has `first_open`, Halv deletes the id and drops
        // any later link.
        reopened.acknowledge(&pending).unwrap();
        assert_eq!(reopened.download_id().unwrap(), None);
        assert!(!directory.0.join(ATTRIBUTION_FILE_NAME).exists());
        assert!(!reopened.accept_download_id("site-download_3", 30).unwrap());
        assert_eq!(reopened.download_id().unwrap(), None);
    }

    #[test]
    fn download_id_that_arrives_shortly_after_first_open_joins_the_queued_event() {
        let directory = TestDirectory::new("attribution-after-first-open");
        let manager = AnalyticsManager::open(&directory.0).unwrap();
        manager.record(FIRST_OPEN, 100).unwrap();
        assert_eq!(manager.pending().unwrap()[0].download_id, None);
        assert!(manager.accept_download_id("late_link", 101).unwrap());
        let reopened = AnalyticsManager::open(&directory.0).unwrap();
        assert_eq!(
            reopened.pending().unwrap()[0].download_id.as_deref(),
            Some("late_link")
        );
        // Only `first_open` may carry the id.
        let mut queue = AnalyticsQueue::default();
        assert!(queue
            .record("active_day", 1, None, None, Some("late_link".into()))
            .is_err());
    }

    #[test]
    fn damaged_attribution_file_counts_as_no_download_id() {
        let directory = TestDirectory::new("attribution-damaged");
        fs::write(directory.0.join(ATTRIBUTION_FILE_NAME), "{not json").unwrap();
        let manager = AnalyticsManager::open(&directory.0).unwrap();
        assert_eq!(manager.download_id().unwrap(), None);
        fs::write(
            directory.0.join(ATTRIBUTION_FILE_NAME),
            r#"{"download_id":"bad id","received_at":1}"#,
        )
        .unwrap();
        let manager = AnalyticsManager::open(&directory.0).unwrap();
        assert_eq!(manager.download_id().unwrap(), None);
        assert!(manager.accept_download_id("good_id", 2).unwrap());
    }

    #[tokio::test]
    async fn server_without_download_id_support_gets_the_batch_again_without_it() {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let mut bodies = Vec::new();
            for _ in 0..2 {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                    .unwrap();
                let mut data = Vec::new();
                let mut buffer = [0; 8192];
                loop {
                    let size = stream.read(&mut buffer).unwrap_or(0);
                    if size == 0 {
                        break;
                    }
                    data.extend_from_slice(&buffer[..size]);
                    let text = String::from_utf8_lossy(&data).to_string();
                    if let Some(end) = text.find("\r\n\r\n") {
                        let length = text[..end]
                            .to_ascii_lowercase()
                            .lines()
                            .find_map(|line| {
                                line.strip_prefix("content-length:")
                                    .and_then(|value| value.trim().parse::<usize>().ok())
                            })
                            .unwrap_or(0);
                        if data.len() >= end + 4 + length {
                            break;
                        }
                    }
                }
                let text = String::from_utf8_lossy(&data).to_string();
                let body = text
                    .split("\r\n\r\n")
                    .nth(1)
                    .unwrap_or_default()
                    .to_string();
                let status = if body.contains("download_id") {
                    "400 Bad Request"
                } else {
                    "200 OK"
                };
                stream
                    .write_all(
                        format!(
                            "HTTP/1.1 {status}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                        )
                        .as_bytes(),
                    )
                    .unwrap();
                bodies.push(body);
            }
            bodies
        });
        let directory = TestDirectory::new("attribution-old-server");
        let manager = AnalyticsManager::open(&directory.0).unwrap();
        manager.accept_download_id("site_link", 1).unwrap();
        manager
            .record(FIRST_OPEN, unix_time_seconds().unwrap())
            .unwrap();
        flush_pending(&manager, &url, "1.0.0").await.unwrap();
        let bodies = server.join().unwrap();
        assert!(
            bodies[0].contains(r#""download_id":"site_link""#),
            "{}",
            bodies[0]
        );
        assert!(!bodies[1].contains("download_id"), "{}", bodies[1]);
        assert!(manager.pending().unwrap().is_empty());
        assert_eq!(manager.download_id().unwrap(), None);
    }
}
