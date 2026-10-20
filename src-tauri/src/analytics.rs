use crate::{app_data, auth, preferences::PreferencesManager, storage::Storage};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};
use tauri::{AppHandle, Manager};

const INSTALL_ID_FILE_NAME: &str = "analytics-install-id";
const QUEUE_FILE_NAME: &str = "analytics-queue.json";
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
    ) -> Result<bool, String> {
        validate_event(event)?;
        validate_category(event, category.as_deref())?;
        let value = AnalyticsEvent {
            event: event.to_string(),
            at,
            category,
            app_version,
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
}

#[derive(Clone, Debug)]
pub struct AnalyticsManager {
    install_id: Arc<str>,
    queue_path: Arc<PathBuf>,
    queue: Arc<Mutex<AnalyticsQueue>>,
    flush_lock: Arc<tokio::sync::Mutex<()>>,
}

impl AnalyticsManager {
    pub fn open(app_data_dir: &Path) -> io::Result<Self> {
        fs::create_dir_all(app_data_dir)?;
        let queue_path = app_data_dir.join(QUEUE_FILE_NAME);
        // An earlier release created these files with the default mode.
        app_data::restrict_file(&app_data_dir.join(INSTALL_ID_FILE_NAME));
        app_data::restrict_file(&queue_path);
        let install_id = read_or_create_install_id(app_data_dir)?;
        let queue = read_queue(&queue_path)?;
        queue
            .validate()
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;

        Ok(Self {
            install_id: install_id.into(),
            queue_path: Arc::new(queue_path),
            queue: Arc::new(Mutex::new(queue)),
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
        let mut next = queue.clone();
        let recorded = next.record(event, at, category, app_version)?;
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
        let mut next = queue.clone();
        next.pending.clear();
        write_queue_atomically(&self.queue_path, &next).map_err(|error| error.to_string())?;
        *queue = next;
        Ok(())
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
            let response = auth::api_client()
                .post(auth::api_url(server_url, path))
                .json(&AnalyticsBatch {
                    install_id: &manager.install_id,
                    app_version,
                    os: analytics_os()?,
                    events: batch,
                })
                .send()
                .await
                .map_err(|error| format!("Analytics request failed: {error}"))?;
            if !response.status().is_success() {
                return Err(format!(
                    "Analytics request returned HTTP {}",
                    response.status()
                ));
            }
            manager.acknowledge(batch)?;
        }
    }
    Ok(())
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
    if !app.state::<PreferencesManager>().analytics_enabled()? {
        return manager.clear_pending();
    }

    manager.record_with_context(
        &event,
        unix_time_seconds()?,
        category,
        Some(app.package_info().version.to_string()),
    )?;
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
}
