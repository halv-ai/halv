//! App updates. The manager checks the release feed, downloads a release in
//! the background, and installs it when the user restarts or quits Halv.
//! The webview only shows the snapshot that the manager sends. See ADR 0012.

mod relaunch;
mod source;
mod staged;

use self::{
    source::{ChunkCallback, PluginUpdateSource, ReleaseInfo, UpdateSource},
    staged::{StagedReadError, StagedUpdate},
};
use serde::Serialize;
use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard},
    thread,
    time::{Duration, Instant},
};
use tauri::{utils::config::BundleType, AppHandle, Emitter, Manager};

pub(crate) const SNAPSHOT_EVENT: &str = "app_update://snapshot";
pub(crate) const BEFORE_RESTART_EVENT: &str = "app_update://before-restart";

const RELEASE_URL_PREFIX: &str = "https://github.com/pedr0v/halv-releases/releases/tag/v";
const UNSUPPORTED_MESSAGE: &str = "This build of Halv cannot install updates.";
const NOT_READY_MESSAGE: &str = "No downloaded Halv update is ready to install.";
const BUSY_MESSAGE: &str = "Halv is already downloading or installing an update.";
/// The plugin applies the check timeout to the request. This margin also
/// ends a check that hangs elsewhere.
const CHECK_TIMEOUT_MARGIN: Duration = Duration::from_secs(5);
/// Automatic downloads of one release stop after this many failures. A
/// permanent failure must not repeat a 177 MB download for days.
const MAX_AUTOMATIC_DOWNLOAD_ATTEMPTS: u32 = 3;

/// The update timers. The tests use short values.
#[derive(Debug, Clone, Copy)]
struct Timings {
    /// The first automatic check waits, so it does not slow the adoption of
    /// supervised sessions and the proxy start.
    first_check_delay: Duration,
    check_interval: Duration,
    /// An automatic check runs only when the last successful check is older.
    recheck_after: Duration,
    check_timeout: Duration,
    /// A download fails when no data arrives for this time.
    stall_timeout: Duration,
    /// An automatic check downloads a failed release again after this time.
    download_retry_after: Duration,
    progress_interval: Duration,
    /// The frontend has this time to save its state before an install.
    flush_timeout: Duration,
    blocker_poll_interval: Duration,
}

impl Default for Timings {
    fn default() -> Self {
        Self {
            first_check_delay: Duration::from_secs(10),
            check_interval: Duration::from_secs(6 * 60 * 60),
            recheck_after: Duration::from_secs(5 * 60),
            check_timeout: Duration::from_secs(15),
            stall_timeout: Duration::from_secs(60),
            download_retry_after: Duration::from_secs(6 * 60 * 60),
            progress_interval: Duration::from_millis(250),
            flush_timeout: Duration::from_secs(1),
            blocker_poll_interval: Duration::from_secs(2),
        }
    }
}

/// How this build of Halv installs an update.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum InstallMode {
    /// The plugin replaces the macOS app bundle or the AppImage in place.
    Replace,
    /// The Windows installer replaces Halv, and then it starts Halv again.
    Installer,
    /// The install asks for administrator rights: a Linux package, or a
    /// macOS app in a folder that only an administrator can change.
    Elevated,
    /// Halv runs from a location that it cannot change, such as a disk
    /// image. It only checks, and the card asks the user to move Halv.
    ReadOnly,
    /// A development build or an unknown package. It checks only when the
    /// user asks.
    Unsupported,
}

/// The access to the app bundle or the AppImage that an in-place install
/// replaces. A larger value is worse.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum TargetAccess {
    Writable,
    /// Only an administrator can change it.
    AdminOnly,
    /// A disk image, App Translocation, or a read-only volume.
    ReadOnly,
}

impl InstallMode {
    fn detect() -> Self {
        if cfg!(debug_assertions) {
            return Self::Unsupported;
        }
        Self::for_bundle(
            tauri::utils::platform::bundle_type(),
            replace_target_access(),
        )
    }

    /// `access` is `None` when Halv runs from no app bundle and no AppImage.
    fn for_bundle(bundle: Option<BundleType>, access: Option<TargetAccess>) -> Self {
        match (bundle, access) {
            (Some(BundleType::App | BundleType::AppImage), Some(TargetAccess::Writable)) => {
                Self::Replace
            }
            // Only the macOS install asks for an administrator password.
            (Some(BundleType::App), Some(TargetAccess::AdminOnly)) => Self::Elevated,
            (Some(BundleType::App | BundleType::AppImage), Some(_)) => Self::ReadOnly,
            (Some(BundleType::Deb | BundleType::Rpm), _) => Self::Elevated,
            (Some(BundleType::Msi | BundleType::Nsis), _) => Self::Installer,
            _ => Self::Unsupported,
        }
    }

    fn can_install(self) -> bool {
        !matches!(self, Self::ReadOnly | Self::Unsupported)
    }

    /// A read-only location still checks, so the card can ask the user to
    /// move Halv.
    fn checks_automatically(self) -> bool {
        self != Self::Unsupported
    }
}

#[cfg(target_os = "macos")]
fn replace_target_access() -> Option<TargetAccess> {
    let bundle = std::env::current_exe()
        .ok()
        .and_then(|path| relaunch::macos_bundle_path(&path).ok())?;
    // macOS runs a quarantined app from a read-only copy.
    if is_translocated(&bundle) {
        return Some(TargetAccess::ReadOnly);
    }
    // The plugin moves the bundle out of its folder, so it needs both.
    let folder = bundle.parent()?;
    Some(path_access(folder).max(path_access(&bundle)))
}

#[cfg(target_os = "linux")]
fn replace_target_access() -> Option<TargetAccess> {
    // The plugin replaces the file that $APPIMAGE names. It renames the
    // file, so it needs write access to the folder.
    let appimage = PathBuf::from(std::env::var_os("APPIMAGE")?);
    appimage.parent().map(path_access)
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn replace_target_access() -> Option<TargetAccess> {
    None
}

#[cfg(any(test, target_os = "macos"))]
fn is_translocated(bundle: &Path) -> bool {
    bundle
        .components()
        .any(|component| component.as_os_str() == "AppTranslocation")
}

#[cfg(unix)]
fn path_access(path: &Path) -> TargetAccess {
    use std::os::unix::ffi::OsStrExt;
    let Ok(c_path) = std::ffi::CString::new(path.as_os_str().as_bytes()) else {
        return TargetAccess::ReadOnly;
    };
    // SAFETY: `c_path` is a valid C string that lives for the call.
    if unsafe { libc::access(c_path.as_ptr(), libc::W_OK) } == 0 {
        return TargetAccess::Writable;
    }
    match std::io::Error::last_os_error().raw_os_error() {
        Some(libc::EACCES | libc::EPERM) => TargetAccess::AdminOnly,
        _ => TargetAccess::ReadOnly,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum UpdateOperation {
    Check,
    Download,
    Install,
    Relaunch,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum InstallTrigger {
    Restart,
    Quit,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(
    tag = "state",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub(crate) enum AppUpdateStatus {
    #[default]
    Idle,
    Checking {
        user_initiated: bool,
    },
    UpToDate {
        user_initiated: bool,
    },
    Available {
        version: String,
        notes: Option<String>,
        release_url: String,
    },
    Downloading {
        version: String,
        downloaded_bytes: u64,
        total_bytes: Option<u64>,
    },
    Ready {
        version: String,
        notes: Option<String>,
        release_url: String,
    },
    Installing {
        version: String,
        trigger: InstallTrigger,
    },
    Restarting {
        version: String,
    },
    Error {
        operation: UpdateOperation,
        message: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        version: Option<String>,
        retryable: bool,
        user_initiated: bool,
    },
}

impl AppUpdateStatus {
    /// A check does not change these states.
    fn is_busy(&self) -> bool {
        matches!(
            self,
            Self::Downloading { .. } | Self::Installing { .. } | Self::Restarting { .. }
        )
    }

    /// A user check can show "checking" over these states.
    fn shows_check(&self) -> bool {
        matches!(
            self,
            Self::Idle | Self::Checking { .. } | Self::UpToDate { .. } | Self::Error { .. }
        )
    }
}

/// Work that a restart stops. Supervised terminals keep running, so they are
/// not in this list.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RestartBlockers {
    /// Chat replies in progress. They end on their own.
    pub chat_replies: usize,
    /// GUI agent sessions with a turn in progress. They end on their own.
    pub gui_agent_sessions: usize,
    /// Running terminals inside the app process. A restart closes them.
    pub in_process_terminals: usize,
}

impl RestartBlockers {
    fn any(&self) -> bool {
        self.chat_replies + self.gui_agent_sessions + self.in_process_terminals > 0
    }

    /// Work that ends on its own. "Restart when done" waits for it.
    fn work_in_progress(&self) -> bool {
        self.chat_replies + self.gui_agent_sessions > 0
    }
}

/// The note after the first start of a new version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PostUpdate {
    from_version: String,
    to_version: String,
    /// Supervised agent sessions that kept running through the restart.
    reconnected_sessions: usize,
    release_url: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AppUpdateSnapshot {
    status: AppUpdateStatus,
    current_version: String,
    auto_download: bool,
    install_mode: InstallMode,
    /// The user chose "Later" for this version in this run.
    deferred_version: Option<String>,
    /// Halv restarts when the work in progress ends.
    restart_when_idle: bool,
    /// The work that stopped the last restart request.
    blockers: Option<RestartBlockers>,
    post_update: Option<PostUpdate>,
    last_checked_at: Option<String>,
    /// The supervisor keeps the terminals and agents through a restart.
    /// It is false on Windows and when the terminals run inside Halv.
    sessions_survive_restart: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct BeforeRestartPayload {
    request_id: u64,
}

/// What an exit request does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ExitDecision {
    Allow,
    /// An install runs. Halv exits when it ends.
    Prevent,
    /// Install the downloaded update first, then exit.
    InstallThenExit,
}

/// Decides an exit request. A restart request cannot be prevented.
/// `staged` is false when the install of the staged update failed in this
/// run, so a failed install does not repeat at each quit.
fn exit_decision(
    code: Option<i32>,
    exit_allowed: bool,
    installing: bool,
    staged: bool,
    install_mode: InstallMode,
) -> ExitDecision {
    if code == Some(tauri::RESTART_EXIT_CODE) || exit_allowed {
        ExitDecision::Allow
    } else if installing {
        ExitDecision::Prevent
    } else if staged && install_mode == InstallMode::Replace {
        // An elevated install or an installer asks the user for more, so
        // only an in-place replacement runs at quit.
        ExitDecision::InstallThenExit
    } else {
        ExitDecision::Allow
    }
}

/// The app side of the manager. The tests use a fake.
pub(crate) trait UpdateHost: Send + Sync {
    fn publish(&self, snapshot: &AppUpdateSnapshot);
    /// Asks the frontend to save its state before an install.
    fn request_flush(&self, request_id: u64);
    fn restart_blockers(&self) -> RestartBlockers;
    /// The supervisor keeps the sessions through a restart.
    fn sessions_survive_restart(&self) -> bool;
    fn auto_download(&self) -> bool;
    fn set_auto_download(&self, enabled: bool) -> Result<(), String>;
    fn hide_windows(&self);
    /// Ends Halv and starts the installed version.
    fn relaunch(&self) -> Result<(), String>;
    /// Ends Halv after an install at quit.
    fn exit(&self, code: i32);
}

#[derive(Default)]
struct State {
    status: AppUpdateStatus,
    /// The newest release from the last successful check.
    release: Option<ReleaseInfo>,
    staged: Option<StagedUpdate>,
    deferred_version: Option<String>,
    post_update: Option<PostUpdate>,
    restart_when_idle: bool,
    restart_wait_generation: u64,
    blockers: Option<RestartBlockers>,
    /// A check runs. The flag is true when a user asked for it.
    check_user_initiated: Option<bool>,
    last_check_success: Option<Instant>,
    last_checked_at: Option<String>,
    download_generation: u64,
    /// Ends the running download. Only an automatic download keeps it.
    download_cancel: Option<tokio::sync::oneshot::Sender<()>>,
    /// The automatic downloads of a release that failed in this run.
    failed_download: Option<FailedDownload>,
    /// The install of this version failed in this run, so a quit does not
    /// try it again.
    install_failed_version: Option<String>,
    /// The exit code of a quit that arrived during a restart install.
    pending_quit: Option<i32>,
    last_progress_publish: Option<Instant>,
    /// The install is complete, so the next exit request must pass.
    exit_allowed: bool,
    flush: Option<(u64, tokio::sync::oneshot::Sender<()>)>,
    next_flush_id: u64,
}

#[derive(Debug, Clone)]
struct FailedDownload {
    version: String,
    automatic_attempts: u32,
    failed_at: Instant,
}

struct Inner {
    source: Arc<dyn UpdateSource>,
    host: Arc<dyn UpdateHost>,
    timings: Timings,
    install_mode: InstallMode,
    current_version: String,
    updates_dir: PathBuf,
    started_at: Instant,
    state: Mutex<State>,
}

#[derive(Clone)]
pub(crate) struct AppUpdateManager {
    inner: Arc<Inner>,
}

enum InstallFailure {
    /// The staged file is missing or changed. A new download fixes it.
    Damaged(String),
    Install(String),
}

impl AppUpdateManager {
    fn new(
        source: Arc<dyn UpdateSource>,
        host: Arc<dyn UpdateHost>,
        timings: Timings,
        install_mode: InstallMode,
        current_version: String,
        updates_dir: PathBuf,
    ) -> Self {
        Self {
            inner: Arc::new(Inner {
                source,
                host,
                timings,
                install_mode,
                current_version,
                updates_dir,
                started_at: Instant::now(),
                state: Mutex::new(State::default()),
            }),
        }
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.inner.state.lock().expect("app update lock poisoned")
    }

    pub(crate) fn snapshot(&self) -> AppUpdateSnapshot {
        let state = self.lock();
        self.snapshot_from(&state)
    }

    fn snapshot_from(&self, state: &State) -> AppUpdateSnapshot {
        AppUpdateSnapshot {
            status: state.status.clone(),
            current_version: self.inner.current_version.clone(),
            auto_download: self.inner.host.auto_download(),
            install_mode: self.inner.install_mode,
            deferred_version: state.deferred_version.clone(),
            restart_when_idle: state.restart_when_idle,
            blockers: state.blockers,
            post_update: state.post_update.clone(),
            last_checked_at: state.last_checked_at.clone(),
            sessions_survive_restart: self.inner.host.sessions_survive_restart(),
        }
    }

    // The caller holds the lock, so the events keep the order of the changes.
    fn publish_locked(&self, state: &State) -> AppUpdateSnapshot {
        let snapshot = self.snapshot_from(state);
        self.inner.host.publish(&snapshot);
        snapshot
    }

    fn auto_download_enabled(&self) -> bool {
        self.inner.install_mode.can_install() && self.inner.host.auto_download()
    }

    fn start_scheduler(&self) {
        // A development build checks only when the user asks.
        if !self.inner.install_mode.checks_automatically() {
            return;
        }
        let manager = self.clone();
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(manager.inner.timings.first_check_delay).await;
            loop {
                manager.check_in_background();
                tokio::time::sleep(manager.inner.timings.check_interval).await;
            }
        });
    }

    /// Checks when the main window gets the focus.
    pub(crate) fn on_window_focused(&self) {
        if self.inner.install_mode.checks_automatically()
            && self.inner.started_at.elapsed() >= self.inner.timings.first_check_delay
        {
            self.check_in_background();
        }
    }

    /// Starts an automatic check unless a check succeeded a short time ago.
    fn check_in_background(&self) {
        {
            let state = self.lock();
            let recent = state
                .last_check_success
                .is_some_and(|at| at.elapsed() < self.inner.timings.recheck_after);
            if recent || state.check_user_initiated.is_some() {
                return;
            }
        }
        self.check(false);
    }

    /// Starts a check and returns at once. The result arrives as a snapshot
    /// event. An automatic check does not change the status until it ends.
    pub(crate) fn check(&self, user_initiated: bool) -> AppUpdateSnapshot {
        let mut guard = self.lock();
        let state = &mut *guard;
        if user_initiated {
            // An explicit check shows the card again.
            state.deferred_version = None;
        }
        if state.status.is_busy() {
            return if user_initiated {
                self.publish_locked(state)
            } else {
                self.snapshot_from(state)
            };
        }
        if let Some(running_user_initiated) = state.check_user_initiated.as_mut() {
            if user_initiated && !*running_user_initiated {
                *running_user_initiated = true;
                if state.status.shows_check() {
                    state.status = AppUpdateStatus::Checking {
                        user_initiated: true,
                    };
                }
            }
            return self.publish_locked(state);
        }
        state.check_user_initiated = Some(user_initiated);
        if user_initiated && state.status.shows_check() {
            state.status = AppUpdateStatus::Checking {
                user_initiated: true,
            };
        }
        let snapshot = self.publish_locked(state);
        drop(guard);
        let manager = self.clone();
        tauri::async_runtime::spawn(async move { manager.run_check().await });
        snapshot
    }

    async fn run_check(self) {
        let timeout = self.inner.timings.check_timeout;
        let result = tokio::time::timeout(
            timeout + CHECK_TIMEOUT_MARGIN,
            self.inner.source.check(timeout),
        )
        .await
        .unwrap_or_else(|_| {
            Err(format!(
                "the update check timed out after {} seconds",
                timeout.as_secs()
            ))
        });
        let mut guard = self.lock();
        let state = &mut *guard;
        let user_initiated = state.check_user_initiated.take().unwrap_or(false);
        match result {
            Ok(release) => {
                state.last_check_success = Some(Instant::now());
                state.last_checked_at = Some(chrono::Utc::now().to_rfc3339());
                self.apply_release(state, release, user_initiated);
            }
            Err(message) => {
                eprintln!("app update: the check failed: {message}");
                if user_initiated && matches!(state.status, AppUpdateStatus::Checking { .. }) {
                    state.status = AppUpdateStatus::Error {
                        operation: UpdateOperation::Check,
                        message,
                        version: None,
                        retryable: true,
                        user_initiated: true,
                    };
                }
            }
        }
        self.publish_locked(state);
    }

    fn apply_release(&self, state: &mut State, release: Option<ReleaseInfo>, user_initiated: bool) {
        if state.status.is_busy() {
            // The next check after the download or the install applies it.
            return;
        }
        let Some(release) = release else {
            // The feed has no newer release now, so the release was pulled.
            discard_staged(state);
            state.release = None;
            state.status = AppUpdateStatus::UpToDate { user_initiated };
            return;
        };
        if state
            .staged
            .as_ref()
            .is_some_and(|staged| staged.version == release.version)
        {
            state.status = ready_status(&release);
            state.release = Some(release);
            return;
        }
        // A newer release replaces the staged one.
        discard_staged(state);
        let known_release = state
            .release
            .as_ref()
            .is_some_and(|known| known.version == release.version);
        state.release = Some(release.clone());
        let download_failed = matches!(
            state.status,
            AppUpdateStatus::Error {
                operation: UpdateOperation::Download,
                ..
            }
        );
        let automatic_download_due = self.automatic_download_due(state, &release.version);
        if known_release && download_failed && !user_initiated && !automatic_download_due {
            // The status bar keeps the error until the next retry.
            return;
        }
        if self.auto_download_enabled() && (user_initiated || automatic_download_due) {
            if user_initiated {
                state.failed_download = None;
            }
            self.start_download(state, release, user_initiated);
        } else {
            state.status = available_status(&release);
        }
    }

    /// An automatic download of `version` can start. After a failure it
    /// waits for the retry interval, and it stops after a few failures.
    fn automatic_download_due(&self, state: &State, version: &str) -> bool {
        match state
            .failed_download
            .as_ref()
            .filter(|failed| failed.version == version)
        {
            None => true,
            Some(failed) => {
                failed.automatic_attempts < MAX_AUTOMATIC_DOWNLOAD_ATTEMPTS
                    && failed.failed_at.elapsed() >= self.inner.timings.download_retry_after
            }
        }
    }

    /// Starts the download of the last checked release.
    pub(crate) fn download(&self) -> Result<AppUpdateSnapshot, String> {
        if !self.inner.install_mode.can_install() {
            return Err(UNSUPPORTED_MESSAGE.to_string());
        }
        let mut guard = self.lock();
        let state = &mut *guard;
        if state.status.is_busy() {
            return Ok(self.snapshot_from(state));
        }
        let release = state
            .release
            .clone()
            .ok_or_else(|| "No Halv update is available.".to_string())?;
        if state
            .staged
            .as_ref()
            .is_some_and(|staged| staged.version == release.version)
        {
            state.status = ready_status(&release);
        } else {
            state.failed_download = None;
            self.start_download(state, release, true);
        }
        Ok(self.publish_locked(state))
    }

    // The caller publishes the new status.
    fn start_download(&self, state: &mut State, release: ReleaseInfo, user_initiated: bool) {
        state.download_generation += 1;
        state.last_progress_publish = None;
        state.status = AppUpdateStatus::Downloading {
            version: release.version.clone(),
            downloaded_bytes: 0,
            total_bytes: None,
        };
        // Turning off the automatic download ends an automatic download. A
        // dropped sender does not cancel.
        let (cancel, cancelled) = tokio::sync::oneshot::channel();
        state.download_cancel = (!user_initiated).then_some(cancel);
        let generation = state.download_generation;
        let manager = self.clone();
        tauri::async_runtime::spawn(async move {
            manager
                .run_download(release, generation, user_initiated, cancelled)
                .await
        });
    }

    async fn run_download(
        self,
        release: ReleaseInfo,
        generation: u64,
        user_initiated: bool,
        cancelled: tokio::sync::oneshot::Receiver<()>,
    ) {
        let progress = Arc::new(DownloadProgress::new());
        let on_chunk: ChunkCallback = {
            let manager = self.clone();
            let progress = progress.clone();
            Box::new(move |length, total| {
                progress.record(length, total);
                manager.publish_progress(generation, &progress);
            })
        };
        let download = self.inner.source.download(&release.version, on_chunk);
        let stall_timeout = self.inner.timings.stall_timeout;
        // Dropping the download future cancels the request.
        let downloaded = tokio::select! {
            result = download => result,
            () = wait_for_stall(progress.clone(), stall_timeout) => Err(format!(
                "the download timed out: no data arrived for {} seconds",
                stall_timeout.as_secs()
            )),
            Ok(()) = cancelled => Err("the download was cancelled".to_string()),
        };
        let staged = match downloaded {
            Ok(bytes) => {
                let dir = self.inner.updates_dir.clone();
                let version = release.version.clone();
                // The bytes leave memory after the write.
                tauri::async_runtime::spawn_blocking(move || staged::write(&dir, &version, &bytes))
                    .await
                    .map_err(|error| error.to_string())
                    .and_then(|written| written.map_err(|error| error.to_string()))
                    .map_err(|error| format!("failed to save the update: {error}"))
            }
            Err(message) => Err(message),
        };

        let mut guard = self.lock();
        let state = &mut *guard;
        if state.download_generation != generation {
            if let Ok(staged) = &staged {
                staged::remove(staged);
            }
            return;
        }
        state.download_cancel = None;
        match staged {
            Ok(staged) => {
                state.staged = Some(staged);
                state.failed_download = None;
                state.status = ready_status(&release);
            }
            Err(message) => {
                eprintln!(
                    "app update: the download of Halv {} failed: {message}",
                    release.version
                );
                let earlier_attempts = state
                    .failed_download
                    .as_ref()
                    .filter(|failed| failed.version == release.version)
                    .map_or(0, |failed| failed.automatic_attempts);
                state.failed_download = Some(FailedDownload {
                    version: release.version.clone(),
                    automatic_attempts: earlier_attempts + u32::from(!user_initiated),
                    failed_at: Instant::now(),
                });
                state.status = AppUpdateStatus::Error {
                    operation: UpdateOperation::Download,
                    message,
                    version: Some(release.version.clone()),
                    retryable: true,
                    user_initiated,
                };
            }
        }
        self.publish_locked(state);
    }

    fn publish_progress(&self, generation: u64, progress: &DownloadProgress) {
        let mut guard = self.lock();
        let state = &mut *guard;
        if state.download_generation != generation {
            return;
        }
        let AppUpdateStatus::Downloading {
            downloaded_bytes,
            total_bytes,
            ..
        } = &mut state.status
        else {
            return;
        };
        let current = progress.current();
        *downloaded_bytes = current.downloaded;
        *total_bytes = current.total;
        let due = match state.last_progress_publish {
            Some(last) => last.elapsed() >= self.inner.timings.progress_interval,
            None => true,
        };
        if due {
            state.last_progress_publish = Some(Instant::now());
            self.publish_locked(state);
        }
    }

    /// Installs the downloaded update and starts Halv again. It returns the
    /// blockers and changes nothing when work would stop and `force` is off.
    pub(crate) fn restart(&self, force: bool) -> Result<Option<RestartBlockers>, String> {
        if !self.inner.install_mode.can_install() {
            return Err(UNSUPPORTED_MESSAGE.to_string());
        }
        let blockers = if force {
            RestartBlockers::default()
        } else {
            self.inner.host.restart_blockers()
        };
        let mut guard = self.lock();
        let state = &mut *guard;
        if state.status.is_busy() {
            return Err(BUSY_MESSAGE.to_string());
        }
        let staged = state
            .staged
            .clone()
            .ok_or_else(|| NOT_READY_MESSAGE.to_string())?;
        if blockers.any() {
            state.blockers = Some(blockers);
            self.publish_locked(state);
            return Ok(Some(blockers));
        }
        self.begin_restart(state, staged);
        Ok(None)
    }

    /// Restarts when the chat replies and the agent turns end.
    pub(crate) fn restart_when_idle(&self) -> Result<AppUpdateSnapshot, String> {
        if !self.inner.install_mode.can_install() {
            return Err(UNSUPPORTED_MESSAGE.to_string());
        }
        let blockers = self.inner.host.restart_blockers();
        let mut guard = self.lock();
        let state = &mut *guard;
        if state.status.is_busy() {
            return Err(BUSY_MESSAGE.to_string());
        }
        let staged = state
            .staged
            .clone()
            .ok_or_else(|| NOT_READY_MESSAGE.to_string())?;
        if !blockers.work_in_progress() {
            self.begin_restart(state, staged);
            return Ok(self.snapshot_from(state));
        }
        state.blockers = Some(blockers);
        if !state.restart_when_idle {
            state.restart_when_idle = true;
            state.restart_wait_generation += 1;
            let generation = state.restart_wait_generation;
            let manager = self.clone();
            tauri::async_runtime::spawn(async move { manager.wait_for_idle(generation).await });
        }
        Ok(self.publish_locked(state))
    }

    async fn wait_for_idle(self, generation: u64) {
        loop {
            tokio::time::sleep(self.inner.timings.blocker_poll_interval).await;
            let blockers = self.inner.host.restart_blockers();
            let mut guard = self.lock();
            let state = &mut *guard;
            if !state.restart_when_idle || state.restart_wait_generation != generation {
                return;
            }
            let staged = state.staged.clone().filter(|_| !state.status.is_busy());
            let Some(staged) = staged else {
                // A newer or a pulled release removed the staged update.
                state.restart_when_idle = false;
                state.blockers = None;
                self.publish_locked(state);
                return;
            };
            if !blockers.work_in_progress() {
                self.begin_restart(state, staged);
                return;
            }
            if state.blockers != Some(blockers) {
                state.blockers = Some(blockers);
                self.publish_locked(state);
            }
        }
    }

    pub(crate) fn cancel_restart(&self) -> AppUpdateSnapshot {
        let mut state = self.lock();
        state.restart_when_idle = false;
        state.restart_wait_generation += 1;
        state.blockers = None;
        self.publish_locked(&state)
    }

    fn begin_restart(&self, state: &mut State, staged: StagedUpdate) {
        state.restart_when_idle = false;
        state.restart_wait_generation += 1;
        state.blockers = None;
        state.status = AppUpdateStatus::Installing {
            version: staged.version.clone(),
            trigger: InstallTrigger::Restart,
        };
        self.publish_locked(state);
        let manager = self.clone();
        tauri::async_runtime::spawn(async move { manager.run_restart(staged).await });
    }

    async fn run_restart(self, staged: StagedUpdate) {
        self.flush_frontend().await;
        let source = self.inner.source.clone();
        let install_staged = staged.clone();
        // The macOS admin path waits for the main thread, so the install
        // never runs there.
        let installed = tauri::async_runtime::spawn_blocking(move || {
            install_staged_update(source.as_ref(), &install_staged)
        })
        .await
        .unwrap_or_else(|error| Err(InstallFailure::Install(error.to_string())));
        let pending_quit = {
            let mut guard = self.lock();
            let state = &mut *guard;
            let pending_quit = state.pending_quit.take();
            match installed {
                Ok(()) => {
                    state.exit_allowed = true;
                    state.status = AppUpdateStatus::Restarting {
                        version: staged.version.clone(),
                    };
                    self.publish_locked(state);
                }
                Err(failure) => {
                    record_install_failure(state, &staged, failure);
                    state.exit_allowed |= pending_quit.is_some();
                    self.publish_locked(state);
                    drop(guard);
                    // The user asked to quit during the install.
                    if let Some(code) = pending_quit {
                        self.inner.host.exit(code);
                    }
                    return;
                }
            }
            pending_quit
        };
        if let Some(code) = pending_quit {
            // The user asked to quit during the install, so Halv does not
            // start again.
            staged::remove(&staged);
            self.inner.host.exit(code);
            return;
        }
        match self.inner.host.relaunch() {
            Ok(()) => staged::remove(&staged),
            Err(message) => {
                eprintln!("app update: failed to start Halv again: {message}");
                let mut state = self.lock();
                state.status = AppUpdateStatus::Error {
                    operation: UpdateOperation::Relaunch,
                    message,
                    version: Some(staged.version.clone()),
                    retryable: true,
                    user_initiated: true,
                };
                self.publish_locked(&state);
            }
        }
    }

    async fn flush_frontend(&self) {
        let (sender, receiver) = tokio::sync::oneshot::channel();
        let request_id = {
            let mut state = self.lock();
            state.next_flush_id += 1;
            let request_id = state.next_flush_id;
            state.flush = Some((request_id, sender));
            request_id
        };
        self.inner.host.request_flush(request_id);
        let _ = tokio::time::timeout(self.inner.timings.flush_timeout, receiver).await;
        let mut state = self.lock();
        if state
            .flush
            .as_ref()
            .is_some_and(|(pending, _)| *pending == request_id)
        {
            state.flush = None;
        }
    }

    /// The frontend saved its state, so the install can start.
    pub(crate) fn acknowledge_restart(&self, request_id: u64) {
        let mut state = self.lock();
        if state
            .flush
            .as_ref()
            .is_some_and(|(pending, _)| *pending == request_id)
        {
            if let Some((_, sender)) = state.flush.take() {
                let _ = sender.send(());
            }
        }
    }

    /// "Later" hides the card for this version until the next start.
    pub(crate) fn defer(&self, version: String) -> AppUpdateSnapshot {
        let mut state = self.lock();
        state.deferred_version = Some(version);
        self.publish_locked(&state)
    }

    pub(crate) fn set_auto_download(&self, enabled: bool) -> Result<AppUpdateSnapshot, String> {
        self.inner.host.set_auto_download(enabled)?;
        let mut guard = self.lock();
        let state = &mut *guard;
        if enabled {
            if self.auto_download_enabled()
                && matches!(state.status, AppUpdateStatus::Available { .. })
            {
                if let Some(release) = state.release.clone() {
                    self.start_download(state, release, false);
                }
            }
        } else if matches!(state.status, AppUpdateStatus::Downloading { .. }) {
            // Only an automatic download has a cancel sender. A download that
            // the user started continues.
            if let Some(cancel) = state.download_cancel.take() {
                let _ = cancel.send(());
                state.download_generation += 1;
                if let Some(release) = state.release.as_ref() {
                    state.status = available_status(release);
                }
            }
        }
        Ok(self.publish_locked(state))
    }

    pub(crate) fn dismiss_post_update(&self) -> AppUpdateSnapshot {
        let mut state = self.lock();
        state.post_update = None;
        self.publish_locked(&state)
    }

    /// Decides an exit request on the main thread, so it never blocks. A
    /// worker thread installs a downloaded update and then ends Halv.
    pub(crate) fn exit_requested(&self, code: Option<i32>) -> ExitDecision {
        let staged = {
            let mut guard = self.lock();
            let state = &mut *guard;
            let installable = state.staged.clone().filter(|staged| {
                state.install_failed_version.as_deref() != Some(staged.version.as_str())
            });
            let decision = exit_decision(
                code,
                state.exit_allowed,
                matches!(state.status, AppUpdateStatus::Installing { .. }),
                installable.is_some(),
                self.inner.install_mode,
            );
            if decision == ExitDecision::Prevent {
                // Halv quits when the running install ends.
                state.pending_quit = Some(code.unwrap_or(0));
                return decision;
            }
            let Some(staged) = installable.filter(|_| decision == ExitDecision::InstallThenExit)
            else {
                return decision;
            };
            state.restart_when_idle = false;
            state.restart_wait_generation += 1;
            state.blockers = None;
            state.status = AppUpdateStatus::Installing {
                version: staged.version.clone(),
                trigger: InstallTrigger::Quit,
            };
            self.publish_locked(state);
            staged
        };
        // Hide the windows before the worker can end Halv.
        self.inner.host.hide_windows();
        let manager = self.clone();
        let exit_code = code.unwrap_or(0);
        let spawned = thread::Builder::new()
            .name("halv-update-install".to_string())
            .spawn(move || manager.install_on_quit(staged, exit_code));
        if let Err(error) = spawned {
            eprintln!("app update: failed to start the install at quit: {error}");
            self.allow_exit();
            return ExitDecision::Allow;
        }
        ExitDecision::InstallThenExit
    }

    fn install_on_quit(&self, staged: StagedUpdate, code: i32) {
        // The user asked to quit, so Halv exits also when the install fails.
        // The supervisor keeps the agent sessions for the next start.
        match install_staged_update(self.inner.source.as_ref(), &staged) {
            Ok(()) => staged::remove(&staged),
            Err(InstallFailure::Damaged(message) | InstallFailure::Install(message)) => {
                eprintln!(
                    "app update: failed to install Halv {} at quit: {message}",
                    staged.version
                );
            }
        }
        self.lock().exit_allowed = true;
        self.inner.host.exit(code);
    }

    /// The JavaScript installer replaced the app already, so this exit must
    /// not install again.
    fn allow_exit(&self) {
        self.lock().exit_allowed = true;
    }
}

fn ready_status(release: &ReleaseInfo) -> AppUpdateStatus {
    AppUpdateStatus::Ready {
        version: release.version.clone(),
        notes: release.notes.clone(),
        release_url: release_url(&release.version),
    }
}

fn available_status(release: &ReleaseInfo) -> AppUpdateStatus {
    AppUpdateStatus::Available {
        version: release.version.clone(),
        notes: release.notes.clone(),
        release_url: release_url(&release.version),
    }
}

fn release_url(version: &str) -> String {
    format!("{RELEASE_URL_PREFIX}{version}")
}

// A restart request applies only to the staged update, so it ends here too.
fn discard_staged(state: &mut State) {
    if let Some(staged) = state.staged.take() {
        staged::remove(&staged);
        state.restart_when_idle = false;
        state.restart_wait_generation += 1;
        state.blockers = None;
    }
}

fn install_staged_update(
    source: &dyn UpdateSource,
    staged: &StagedUpdate,
) -> Result<(), InstallFailure> {
    let bytes = staged::read_verified(staged).map_err(|error| match error {
        StagedReadError::Damaged => {
            InstallFailure::Damaged("the downloaded update changed on disk".to_string())
        }
        StagedReadError::Unreadable(message) => InstallFailure::Damaged(message),
    })?;
    source
        .install(&staged.version, bytes)
        .map_err(InstallFailure::Install)
}

fn record_install_failure(state: &mut State, staged: &StagedUpdate, failure: InstallFailure) {
    let (operation, message) = match failure {
        InstallFailure::Damaged(message) => {
            // Only a new download can replace a damaged file.
            discard_staged(state);
            (UpdateOperation::Download, message)
        }
        InstallFailure::Install(message) => {
            state.install_failed_version = Some(staged.version.clone());
            (UpdateOperation::Install, message)
        }
    };
    eprintln!(
        "app update: failed to install Halv {}: {message}",
        staged.version
    );
    state.status = AppUpdateStatus::Error {
        operation,
        message,
        version: Some(staged.version.clone()),
        retryable: true,
        user_initiated: true,
    };
}

#[derive(Debug, Clone, Copy)]
struct ProgressState {
    downloaded: u64,
    total: Option<u64>,
    last_data: Instant,
}

struct DownloadProgress(Mutex<ProgressState>);

impl DownloadProgress {
    fn new() -> Self {
        Self(Mutex::new(ProgressState {
            downloaded: 0,
            total: None,
            last_data: Instant::now(),
        }))
    }

    fn record(&self, length: usize, total: Option<u64>) {
        let mut progress = self.0.lock().expect("download progress lock poisoned");
        progress.downloaded += length as u64;
        if total.is_some() {
            progress.total = total;
        }
        progress.last_data = Instant::now();
    }

    fn current(&self) -> ProgressState {
        *self.0.lock().expect("download progress lock poisoned")
    }
}

/// Returns when no data arrived for `timeout`.
async fn wait_for_stall(progress: Arc<DownloadProgress>, timeout: Duration) {
    let poll = (timeout / 4).min(Duration::from_secs(1));
    loop {
        tokio::time::sleep(poll).await;
        if progress.current().last_data.elapsed() >= timeout {
            return;
        }
    }
}

/// Compares release versions by their numeric core. A version that does not
/// parse counts as newer when it differs.
fn version_is_newer(current: &str, previous: &str) -> bool {
    match (parse_version(current), parse_version(previous)) {
        (Some(current), Some(previous)) => current > previous,
        _ => current != previous,
    }
}

fn parse_version(version: &str) -> Option<(u64, u64, u64)> {
    let core = version
        .trim()
        .trim_start_matches('v')
        .split(['-', '+'])
        .next()?;
    let mut parts = core.split('.').map(|part| part.parse::<u64>().ok());
    let parsed = (parts.next()??, parts.next()??, parts.next()??);
    parts.next().is_none().then_some(parsed)
}

/// Builds the note for the first start after an update. The newest boot
/// record is this run, and the next one is the run before it.
fn post_update_from_history(
    history: &[crate::boot_record::BootRecordEntry],
    current_version: &str,
    reconnected_sessions: usize,
) -> Option<PostUpdate> {
    let previous = history.get(1)?;
    if !version_is_newer(current_version, &previous.app_version) {
        return None;
    }
    Some(PostUpdate {
        from_version: previous.app_version.clone(),
        to_version: current_version.to_string(),
        reconnected_sessions,
        release_url: release_url(current_version),
    })
}

struct TauriHost {
    app: AppHandle,
}

impl UpdateHost for TauriHost {
    fn publish(&self, snapshot: &AppUpdateSnapshot) {
        if let Err(error) = self.app.emit(SNAPSHOT_EVENT, snapshot) {
            eprintln!("app update: failed to send the update status: {error}");
        }
    }

    fn request_flush(&self, request_id: u64) {
        if let Err(error) = self
            .app
            .emit(BEFORE_RESTART_EVENT, BeforeRestartPayload { request_id })
        {
            eprintln!("app update: failed to announce the restart: {error}");
        }
    }

    fn restart_blockers(&self) -> RestartBlockers {
        RestartBlockers {
            chat_replies: self
                .app
                .try_state::<crate::chat::ChatManager>()
                .map_or(0, |chat| chat.in_flight_count()),
            gui_agent_sessions: self
                .app
                .try_state::<crate::agent_sessions::AgentSessionManager>()
                .map_or(0, |sessions| sessions.busy_gui_session_count()),
            in_process_terminals: self
                .app
                .try_state::<crate::terminal::TerminalManager>()
                .map_or(0, |terminals| terminals.in_process_session_count()),
        }
    }

    fn sessions_survive_restart(&self) -> bool {
        self.app
            .try_state::<crate::terminal::TerminalManager>()
            .is_some_and(|terminals| terminals.sessions_survive_restart())
    }

    fn auto_download(&self) -> bool {
        self.app
            .try_state::<crate::preferences::PreferencesManager>()
            .and_then(|preferences| preferences.app_update_auto_download().ok())
            .unwrap_or(true)
    }

    fn set_auto_download(&self, enabled: bool) -> Result<(), String> {
        self.app
            .try_state::<crate::preferences::PreferencesManager>()
            .ok_or_else(|| "Halv preferences are not ready.".to_string())?
            .set_app_update_auto_download(enabled)
            .map(|_| ())
    }

    fn hide_windows(&self) {
        for window in self.app.webview_windows().values() {
            if let Err(error) = window.hide() {
                eprintln!("app update: failed to hide a window: {error}");
            }
        }
    }

    fn relaunch(&self) -> Result<(), String> {
        relaunch::relaunch(&self.app)
    }

    fn exit(&self, code: i32) {
        relaunch::arm_exit_watchdog(relaunch::EXIT_WATCHDOG_TIMEOUT, move || {
            std::process::exit(code)
        });
        self.app.exit(code);
    }
}

/// Creates the manager. Call it after the adoption of supervised sessions,
/// so the post-update note can count them.
pub(crate) fn setup(app: &AppHandle, app_data_dir: &Path) {
    let updates_dir = staged::updates_dir(app_data_dir);
    match staged::remove_leftovers(&updates_dir) {
        Ok(0) => {}
        Ok(count) => eprintln!("app update: removed {count} staged updates of an earlier run"),
        Err(error) => eprintln!("app update: failed to remove earlier staged updates: {error}"),
    }
    let current_version = app.package_info().version.to_string();
    let manager = AppUpdateManager::new(
        Arc::new(PluginUpdateSource::new(app.clone())),
        Arc::new(TauriHost { app: app.clone() }),
        Timings::default(),
        InstallMode::detect(),
        current_version.clone(),
        updates_dir,
    );
    let history = app
        .try_state::<crate::boot_record::BootRecordManager>()
        .map(|boot_record| boot_record.history(2))
        .unwrap_or_default();
    let reconnected_sessions = app
        .try_state::<crate::terminal::TerminalManager>()
        .map_or(0, |terminals| terminals.running_supervised_agent_count());
    manager.lock().post_update =
        post_update_from_history(&history, &current_version, reconnected_sessions);
    manager.start_scheduler();
    app.manage(manager);
}

/// Runs when the main window gets the focus.
pub(crate) fn on_main_window_focused(app: &AppHandle) {
    if let Some(manager) = app.try_state::<AppUpdateManager>() {
        manager.on_window_focused();
    }
}

/// Runs for each exit request. The app menu, the tray, and the frontend quit
/// through `AppHandle::exit`, so they all arrive here.
pub(crate) fn on_exit_requested(app: &AppHandle, code: Option<i32>, api: &tauri::ExitRequestApi) {
    let Some(manager) = app.try_state::<AppUpdateManager>() else {
        return;
    };
    if manager.exit_requested(code) != ExitDecision::Allow {
        api.prevent_exit();
    }
}

/// Runs a user check from the macOS app menu.
#[cfg(target_os = "macos")]
pub(crate) fn check_from_menu<R: tauri::Runtime>(app: &tauri::AppHandle<R>) {
    if let Some(manager) = app.try_state::<AppUpdateManager>() {
        manager.check(true);
    }
}

#[tauri::command]
pub fn app_update_snapshot(manager: tauri::State<'_, AppUpdateManager>) -> AppUpdateSnapshot {
    manager.snapshot()
}

#[tauri::command]
pub fn app_update_check(
    manager: tauri::State<'_, AppUpdateManager>,
    user_initiated: bool,
) -> AppUpdateSnapshot {
    manager.check(user_initiated)
}

#[tauri::command]
pub fn app_update_download(
    manager: tauri::State<'_, AppUpdateManager>,
) -> Result<AppUpdateSnapshot, String> {
    manager.download()
}

#[tauri::command]
pub fn app_update_restart_blockers(manager: tauri::State<'_, AppUpdateManager>) -> RestartBlockers {
    manager.inner.host.restart_blockers()
}

/// Returns `null` when the restart started, or the work that would stop.
#[tauri::command]
pub fn app_update_restart(
    manager: tauri::State<'_, AppUpdateManager>,
    force: bool,
) -> Result<Option<RestartBlockers>, String> {
    manager.restart(force)
}

#[tauri::command]
pub fn app_update_restart_when_idle(
    manager: tauri::State<'_, AppUpdateManager>,
) -> Result<AppUpdateSnapshot, String> {
    manager.restart_when_idle()
}

#[tauri::command]
pub fn app_update_cancel_restart(manager: tauri::State<'_, AppUpdateManager>) -> AppUpdateSnapshot {
    manager.cancel_restart()
}

#[tauri::command]
pub fn app_update_restart_ack(manager: tauri::State<'_, AppUpdateManager>, request_id: u64) {
    manager.acknowledge_restart(request_id);
}

#[tauri::command]
pub fn app_update_defer(
    manager: tauri::State<'_, AppUpdateManager>,
    version: String,
) -> AppUpdateSnapshot {
    manager.defer(version)
}

#[tauri::command]
pub fn app_update_set_auto_download(
    manager: tauri::State<'_, AppUpdateManager>,
    enabled: bool,
) -> Result<AppUpdateSnapshot, String> {
    manager.set_auto_download(enabled)
}

#[tauri::command]
pub fn app_update_dismiss_post_update(
    manager: tauri::State<'_, AppUpdateManager>,
) -> AppUpdateSnapshot {
    manager.dismiss_post_update()
}

/// Starts Halv again after the JavaScript installer. The update card uses
/// `app_update_restart` instead.
#[tauri::command]
pub fn app_update_relaunch(
    app: AppHandle,
    manager: tauri::State<'_, AppUpdateManager>,
) -> Result<(), String> {
    manager.allow_exit();
    relaunch::relaunch(&app)
}

#[cfg(test)]
mod tests {
    use super::{
        source::fake::{release, FakeDownload, FakeSource},
        *,
    };
    use crate::boot_record::{BootRecordEntry, ShutdownKind};
    use std::{fs, sync::atomic::Ordering};

    #[derive(Default)]
    struct FakeHost {
        snapshots: Mutex<Vec<AppUpdateSnapshot>>,
        flush_requests: Mutex<Vec<u64>>,
        blockers: Mutex<RestartBlockers>,
        manual_download: Mutex<bool>,
        relaunches: Mutex<usize>,
        relaunch_error: Mutex<Option<String>>,
        exits: Mutex<Vec<i32>>,
        hidden: Mutex<usize>,
        sessions_lost: Mutex<bool>,
    }

    impl FakeHost {
        fn set_blockers(&self, blockers: RestartBlockers) {
            *self.blockers.lock().unwrap() = blockers;
        }

        fn published(&self) -> Vec<AppUpdateSnapshot> {
            self.snapshots.lock().unwrap().clone()
        }
    }

    impl UpdateHost for FakeHost {
        fn publish(&self, snapshot: &AppUpdateSnapshot) {
            self.snapshots.lock().unwrap().push(snapshot.clone());
        }

        fn request_flush(&self, request_id: u64) {
            self.flush_requests.lock().unwrap().push(request_id);
        }

        fn restart_blockers(&self) -> RestartBlockers {
            *self.blockers.lock().unwrap()
        }

        fn sessions_survive_restart(&self) -> bool {
            !*self.sessions_lost.lock().unwrap()
        }

        fn auto_download(&self) -> bool {
            !*self.manual_download.lock().unwrap()
        }

        fn set_auto_download(&self, enabled: bool) -> Result<(), String> {
            *self.manual_download.lock().unwrap() = !enabled;
            Ok(())
        }

        fn hide_windows(&self) {
            *self.hidden.lock().unwrap() += 1;
        }

        fn relaunch(&self) -> Result<(), String> {
            if let Some(error) = self.relaunch_error.lock().unwrap().clone() {
                return Err(error);
            }
            *self.relaunches.lock().unwrap() += 1;
            Ok(())
        }

        fn exit(&self, code: i32) {
            self.exits.lock().unwrap().push(code);
        }
    }

    struct TestDirectory(PathBuf);

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    struct Harness {
        manager: AppUpdateManager,
        source: Arc<FakeSource>,
        host: Arc<FakeHost>,
        directory: TestDirectory,
    }

    impl Harness {
        fn new(install_mode: InstallMode) -> Self {
            Self::with_timings(install_mode, test_timings())
        }

        fn with_timings(install_mode: InstallMode, timings: Timings) -> Self {
            let directory = TestDirectory(
                std::env::temp_dir()
                    .join(format!("halv-app-update-{:032x}", rand::random::<u128>())),
            );
            let source = Arc::new(FakeSource::default());
            let host = Arc::new(FakeHost::default());
            let manager = AppUpdateManager::new(
                source.clone(),
                host.clone(),
                timings,
                install_mode,
                "0.7.1".to_string(),
                staged::updates_dir(&directory.0),
            );
            Self {
                manager,
                source,
                host,
                directory,
            }
        }

        fn staged_file(&self, version: &str) -> PathBuf {
            staged::updates_dir(&self.directory.0).join(format!("halv-{version}.archive"))
        }

        fn wait_for(&self, predicate: impl Fn(&AppUpdateSnapshot) -> bool) -> AppUpdateSnapshot {
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                let snapshot = self.manager.snapshot();
                if predicate(&snapshot) {
                    return snapshot;
                }
                assert!(
                    Instant::now() < deadline,
                    "the update manager did not reach the expected state: {snapshot:?}"
                );
                thread::sleep(Duration::from_millis(5));
            }
        }

        fn wait_for_flush_request(&self) -> u64 {
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                if let Some(request_id) = self.host.flush_requests.lock().unwrap().first() {
                    return *request_id;
                }
                assert!(Instant::now() < deadline, "no flush request arrived");
                thread::sleep(Duration::from_millis(5));
            }
        }

        /// Waits until the running check applied its result.
        fn wait_for_check(&self) {
            self.wait_for(|_| self.manager.lock().check_user_initiated.is_none());
        }

        /// Checks, downloads, and stages `version`.
        fn stage(&self, version: &str, bytes: &[u8]) {
            self.source.push_check(Ok(Some(release(version))));
            self.source.push_download(chunks(&[bytes]));
            self.manager.check(false);
            self.wait_for(|snapshot| is_ready(snapshot, version));
        }
    }

    fn test_timings() -> Timings {
        Timings {
            first_check_delay: Duration::from_millis(10),
            check_interval: Duration::from_secs(3600),
            recheck_after: Duration::from_secs(300),
            check_timeout: Duration::from_secs(5),
            stall_timeout: Duration::from_millis(150),
            download_retry_after: Duration::from_secs(3600),
            progress_interval: Duration::ZERO,
            flush_timeout: Duration::from_millis(20),
            blocker_poll_interval: Duration::from_millis(10),
        }
    }

    fn chunks(chunks: &[&[u8]]) -> FakeDownload {
        FakeDownload::Chunks {
            chunks: chunks.iter().map(|chunk| chunk.to_vec()).collect(),
            delay: Duration::from_millis(1),
        }
    }

    fn is_ready(snapshot: &AppUpdateSnapshot, version: &str) -> bool {
        matches!(&snapshot.status, AppUpdateStatus::Ready { version: ready, .. } if ready == version)
    }

    fn is_error(snapshot: &AppUpdateSnapshot, expected: UpdateOperation) -> bool {
        matches!(&snapshot.status, AppUpdateStatus::Error { operation, .. } if *operation == expected)
    }

    #[test]
    fn a_check_downloads_and_stages_the_release() {
        let harness = Harness::new(InstallMode::Replace);
        harness.source.push_check(Ok(Some(release("0.7.2"))));
        harness.source.push_download(chunks(&[b"abc", b"def"]));

        let started = harness.manager.check(false);
        assert_eq!(started.status, AppUpdateStatus::Idle);
        let ready = harness.wait_for(|snapshot| is_ready(snapshot, "0.7.2"));

        assert_eq!(
            ready.status,
            AppUpdateStatus::Ready {
                version: "0.7.2".to_string(),
                notes: Some("## What's new\n- Halv 0.7.2".to_string()),
                release_url: "https://github.com/pedr0v/halv-releases/releases/tag/v0.7.2"
                    .to_string(),
            }
        );
        assert!(ready.last_checked_at.is_some());
        assert_eq!(
            fs::read(harness.staged_file("0.7.2")).unwrap(),
            b"abcdef".to_vec()
        );
        let published = harness.host.published();
        assert!(published.iter().any(|snapshot| snapshot.status
            == AppUpdateStatus::Downloading {
                version: "0.7.2".to_string(),
                downloaded_bytes: 3,
                total_bytes: Some(6),
            }));
        assert!(!published
            .iter()
            .any(|snapshot| matches!(snapshot.status, AppUpdateStatus::Checking { .. })));
    }

    #[test]
    fn without_auto_download_the_card_offers_the_download() {
        let harness = Harness::new(InstallMode::Replace);
        harness.manager.set_auto_download(false).unwrap();
        harness.source.push_check(Ok(Some(release("0.7.2"))));

        harness.manager.check(false);
        let available = harness
            .wait_for(|snapshot| matches!(snapshot.status, AppUpdateStatus::Available { .. }));
        assert!(!available.auto_download);
        assert_eq!(harness.source.download_calls.load(Ordering::SeqCst), 0);

        harness.source.push_download(chunks(&[b"archive"]));
        let downloading = harness.manager.download().unwrap();
        assert!(matches!(
            downloading.status,
            AppUpdateStatus::Downloading { .. }
        ));
        harness.wait_for(|snapshot| is_ready(snapshot, "0.7.2"));
    }

    #[test]
    fn turning_on_auto_download_starts_the_offered_download() {
        let harness = Harness::new(InstallMode::Replace);
        harness.manager.set_auto_download(false).unwrap();
        harness.source.push_check(Ok(Some(release("0.7.2"))));
        harness.manager.check(false);
        harness.wait_for(|snapshot| matches!(snapshot.status, AppUpdateStatus::Available { .. }));

        harness.source.push_download(chunks(&[b"archive"]));
        let snapshot = harness.manager.set_auto_download(true).unwrap();
        assert!(snapshot.auto_download);
        harness.wait_for(|snapshot| is_ready(snapshot, "0.7.2"));
    }

    #[test]
    fn user_checks_report_the_result_and_automatic_failures_stay_quiet() {
        let harness = Harness::new(InstallMode::Replace);
        harness.source.push_check(Ok(None));
        let checking = harness.manager.check(true);
        assert_eq!(
            checking.status,
            AppUpdateStatus::Checking {
                user_initiated: true
            }
        );
        harness.wait_for(|snapshot| {
            snapshot.status
                == AppUpdateStatus::UpToDate {
                    user_initiated: true,
                }
        });

        harness
            .source
            .push_check(Err("error sending request".to_string()));
        harness.manager.check(false);
        harness.wait_for(|_| harness.source.check_calls.load(Ordering::SeqCst) == 2);
        harness.wait_for_check();
        assert_eq!(
            harness.manager.snapshot().status,
            AppUpdateStatus::UpToDate {
                user_initiated: true
            }
        );

        harness
            .source
            .push_check(Err("error sending request".to_string()));
        harness.manager.check(true);
        let failed = harness.wait_for(|snapshot| is_error(snapshot, UpdateOperation::Check));
        assert_eq!(
            failed.status,
            AppUpdateStatus::Error {
                operation: UpdateOperation::Check,
                message: "error sending request".to_string(),
                version: None,
                retryable: true,
                user_initiated: true,
            }
        );
    }

    #[test]
    fn automatic_checks_wait_for_the_recheck_interval() {
        let harness = Harness::new(InstallMode::Replace);
        harness.source.push_check(Ok(None));
        harness.manager.check_in_background();
        harness.wait_for(|snapshot| matches!(snapshot.status, AppUpdateStatus::UpToDate { .. }));

        harness.manager.on_window_focused();
        harness.manager.check_in_background();
        thread::sleep(Duration::from_millis(30));
        assert_eq!(harness.source.check_calls.load(Ordering::SeqCst), 1);

        // A user check does not wait.
        harness.manager.check(true);
        harness.wait_for(|_| harness.source.check_calls.load(Ordering::SeqCst) == 2);
    }

    #[test]
    fn the_scheduler_checks_after_the_first_delay() {
        let harness = Harness::new(InstallMode::Replace);
        harness.manager.set_auto_download(false).unwrap();
        harness.source.push_check(Ok(Some(release("0.7.2"))));
        harness.manager.start_scheduler();
        harness.wait_for(|snapshot| matches!(snapshot.status, AppUpdateStatus::Available { .. }));
        assert_eq!(harness.source.check_calls.load(Ordering::SeqCst), 1);

        let development = Harness::new(InstallMode::Unsupported);
        development.manager.start_scheduler();
        development.manager.on_window_focused();
        thread::sleep(Duration::from_millis(50));
        assert_eq!(development.source.check_calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn a_focus_right_after_the_start_waits_for_the_first_check() {
        let harness = Harness::with_timings(
            InstallMode::Replace,
            Timings {
                first_check_delay: Duration::from_secs(30),
                ..test_timings()
            },
        );
        harness.manager.on_window_focused();
        thread::sleep(Duration::from_millis(30));
        assert_eq!(harness.source.check_calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn a_stalled_download_fails_and_waits_for_the_retry_interval() {
        let harness = Harness::new(InstallMode::Replace);
        harness.source.push_check(Ok(Some(release("0.7.2"))));
        harness.source.push_download(FakeDownload::Hang);
        harness.manager.check(false);

        let failed = harness.wait_for(|snapshot| is_error(snapshot, UpdateOperation::Download));
        let AppUpdateStatus::Error {
            message,
            version,
            retryable,
            ..
        } = failed.status
        else {
            unreachable!();
        };
        assert!(message.contains("timed out"), "{message}");
        assert_eq!(version.as_deref(), Some("0.7.2"));
        assert!(retryable);

        // An automatic check keeps the error until the retry interval ends.
        harness.source.push_check(Ok(Some(release("0.7.2"))));
        harness.manager.check(false);
        harness.wait_for(|_| harness.source.check_calls.load(Ordering::SeqCst) == 2);
        harness.wait_for_check();
        assert_eq!(harness.source.download_calls.load(Ordering::SeqCst), 1);
        assert!(is_error(
            &harness.manager.snapshot(),
            UpdateOperation::Download
        ));

        harness.source.push_download(chunks(&[b"archive"]));
        harness.manager.download().unwrap();
        harness.wait_for(|snapshot| is_ready(snapshot, "0.7.2"));
        assert_eq!(harness.source.download_calls.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn a_failed_download_reports_the_plugin_error() {
        let harness = Harness::new(InstallMode::Replace);
        harness.source.push_check(Ok(Some(release("0.7.2"))));
        harness.source.push_download(FakeDownload::Fail(
            "signature verification failed".to_string(),
        ));
        harness.manager.check(true);
        let failed = harness.wait_for(|snapshot| is_error(snapshot, UpdateOperation::Download));
        assert!(matches!(
            failed.status,
            AppUpdateStatus::Error { ref message, user_initiated: true, .. }
                if message == "signature verification failed"
        ));
        assert!(!harness.staged_file("0.7.2").exists());
    }

    #[test]
    fn a_newer_release_replaces_the_staged_one() {
        let harness = Harness::new(InstallMode::Replace);
        harness.stage("0.7.2", b"old archive");

        harness.source.push_check(Ok(Some(release("0.7.3"))));
        harness.source.push_download(chunks(&[b"new archive"]));
        harness.manager.check(false);
        harness.wait_for(|snapshot| is_ready(snapshot, "0.7.3"));

        assert!(!harness.staged_file("0.7.2").exists());
        assert_eq!(
            fs::read(harness.staged_file("0.7.3")).unwrap(),
            b"new archive".to_vec()
        );
    }

    #[test]
    fn the_same_release_keeps_the_staged_file() {
        let harness = Harness::new(InstallMode::Replace);
        harness.stage("0.7.2", b"archive");

        harness.source.push_check(Ok(Some(release("0.7.2"))));
        let checking = harness.manager.check(true);
        // A user check keeps the ready card.
        assert!(is_ready(&checking, "0.7.2"));
        harness.wait_for(|_| harness.source.check_calls.load(Ordering::SeqCst) == 2);
        harness.wait_for_check();
        assert!(is_ready(&harness.manager.snapshot(), "0.7.2"));
        assert_eq!(harness.source.download_calls.load(Ordering::SeqCst), 1);
        assert!(harness.staged_file("0.7.2").exists());
    }

    #[test]
    fn a_pulled_release_discards_the_staged_update() {
        let harness = Harness::new(InstallMode::Replace);
        harness.stage("0.7.2", b"archive");

        harness.source.push_check(Ok(None));
        harness.manager.check(false);
        harness.wait_for(|snapshot| matches!(snapshot.status, AppUpdateStatus::UpToDate { .. }));
        assert!(!harness.staged_file("0.7.2").exists());
        assert_eq!(
            harness.manager.restart(true),
            Err(NOT_READY_MESSAGE.to_string())
        );
    }

    #[test]
    fn restart_reports_blockers_unless_forced() {
        let harness = Harness::new(InstallMode::Replace);
        harness.stage("0.7.2", b"archive");
        let blockers = RestartBlockers {
            chat_replies: 1,
            gui_agent_sessions: 0,
            in_process_terminals: 2,
        };
        harness.host.set_blockers(blockers);

        assert_eq!(harness.manager.restart(false), Ok(Some(blockers)));
        let blocked = harness.manager.snapshot();
        assert_eq!(blocked.blockers, Some(blockers));
        assert!(is_ready(&blocked, "0.7.2"));
        assert!(harness.source.installs.lock().unwrap().is_empty());

        assert_eq!(harness.manager.restart(true), Ok(None));
        let restarting = harness
            .wait_for(|snapshot| matches!(snapshot.status, AppUpdateStatus::Restarting { .. }));
        assert_eq!(restarting.blockers, None);
        assert_eq!(
            *harness.source.installs.lock().unwrap(),
            vec![("0.7.2".to_string(), b"archive".to_vec())]
        );
        harness.wait_for(|_| *harness.host.relaunches.lock().unwrap() == 1);
        assert_eq!(harness.host.flush_requests.lock().unwrap().len(), 1);
        assert!(harness
            .host
            .published()
            .iter()
            .any(|snapshot| snapshot.status
                == AppUpdateStatus::Installing {
                    version: "0.7.2".to_string(),
                    trigger: InstallTrigger::Restart,
                }));
        // The relaunch exit must not install again.
        assert_eq!(harness.manager.exit_requested(Some(0)), ExitDecision::Allow);
    }

    #[test]
    fn the_frontend_acknowledgement_ends_the_flush_wait() {
        let timings = Timings {
            flush_timeout: Duration::from_secs(30),
            ..test_timings()
        };
        let harness = Harness::with_timings(InstallMode::Replace, timings);
        harness.stage("0.7.2", b"archive");

        assert_eq!(harness.manager.restart(true), Ok(None));
        let request_id = loop {
            if let Some(request_id) = harness.host.flush_requests.lock().unwrap().first() {
                break *request_id;
            }
            thread::sleep(Duration::from_millis(5));
        };
        thread::sleep(Duration::from_millis(30));
        assert!(harness.source.installs.lock().unwrap().is_empty());

        harness.manager.acknowledge_restart(request_id + 1);
        thread::sleep(Duration::from_millis(30));
        assert!(harness.source.installs.lock().unwrap().is_empty());

        harness.manager.acknowledge_restart(request_id);
        harness.wait_for(|snapshot| matches!(snapshot.status, AppUpdateStatus::Restarting { .. }));
    }

    #[test]
    fn restart_when_idle_waits_for_the_running_work() {
        let harness = Harness::new(InstallMode::Replace);
        harness.stage("0.7.2", b"archive");
        harness.host.set_blockers(RestartBlockers {
            chat_replies: 1,
            gui_agent_sessions: 1,
            in_process_terminals: 1,
        });

        let waiting = harness.manager.restart_when_idle().unwrap();
        assert!(waiting.restart_when_idle);
        thread::sleep(Duration::from_millis(50));
        assert!(harness.source.installs.lock().unwrap().is_empty());

        harness.host.set_blockers(RestartBlockers {
            chat_replies: 0,
            gui_agent_sessions: 1,
            in_process_terminals: 1,
        });
        harness.wait_for(|snapshot| {
            snapshot
                .blockers
                .is_some_and(|blockers| blockers.chat_replies == 0)
        });
        assert!(harness.source.installs.lock().unwrap().is_empty());

        // A terminal does not end on its own, so the restart does not wait.
        harness.host.set_blockers(RestartBlockers {
            chat_replies: 0,
            gui_agent_sessions: 0,
            in_process_terminals: 1,
        });
        let restarting = harness
            .wait_for(|snapshot| matches!(snapshot.status, AppUpdateStatus::Restarting { .. }));
        assert!(!restarting.restart_when_idle);
        assert_eq!(harness.source.installs.lock().unwrap().len(), 1);
    }

    #[test]
    fn cancel_restart_stops_the_wait() {
        let harness = Harness::new(InstallMode::Replace);
        harness.stage("0.7.2", b"archive");
        harness.host.set_blockers(RestartBlockers {
            chat_replies: 1,
            ..RestartBlockers::default()
        });
        harness.manager.restart_when_idle().unwrap();

        let cancelled = harness.manager.cancel_restart();
        assert!(!cancelled.restart_when_idle);
        assert_eq!(cancelled.blockers, None);
        harness.host.set_blockers(RestartBlockers::default());
        thread::sleep(Duration::from_millis(50));
        assert!(harness.source.installs.lock().unwrap().is_empty());
        assert!(is_ready(&harness.manager.snapshot(), "0.7.2"));
    }

    #[test]
    fn a_damaged_staged_file_asks_for_a_new_download() {
        let harness = Harness::new(InstallMode::Replace);
        harness.stage("0.7.2", b"archive");
        fs::write(harness.staged_file("0.7.2"), b"archivE").unwrap();

        assert_eq!(harness.manager.restart(true), Ok(None));
        let failed = harness.wait_for(|snapshot| is_error(snapshot, UpdateOperation::Download));
        assert!(matches!(
            failed.status,
            AppUpdateStatus::Error {
                retryable: true,
                ..
            }
        ));
        assert!(harness.source.installs.lock().unwrap().is_empty());
        assert!(!harness.staged_file("0.7.2").exists());
        assert_eq!(*harness.host.relaunches.lock().unwrap(), 0);

        harness.source.push_download(chunks(&[b"archive"]));
        harness.manager.download().unwrap();
        harness.wait_for(|snapshot| is_ready(snapshot, "0.7.2"));
    }

    #[test]
    fn an_install_failure_keeps_the_staged_update() {
        let harness = Harness::new(InstallMode::Replace);
        harness.stage("0.7.2", b"archive");
        *harness.source.install_error.lock().unwrap() = Some("permission denied".to_string());

        harness.manager.restart(true).unwrap();
        let failed = harness.wait_for(|snapshot| is_error(snapshot, UpdateOperation::Install));
        assert!(matches!(
            failed.status,
            AppUpdateStatus::Error { ref message, .. } if message == "permission denied"
        ));
        assert!(harness.staged_file("0.7.2").exists());
        assert_eq!(*harness.host.relaunches.lock().unwrap(), 0);
        // A quit does not repeat the failed install.
        assert_eq!(harness.manager.exit_requested(Some(0)), ExitDecision::Allow);
        assert!(harness.source.installs.lock().unwrap().is_empty());
    }

    #[test]
    fn an_install_failure_can_be_retried() {
        let harness = Harness::new(InstallMode::Replace);
        harness.stage("0.7.2", b"archive");
        *harness.source.install_error.lock().unwrap() = Some("permission denied".to_string());
        harness.manager.restart(true).unwrap();
        harness.wait_for(|snapshot| is_error(snapshot, UpdateOperation::Install));

        *harness.source.install_error.lock().unwrap() = None;
        assert_eq!(harness.manager.restart(true), Ok(None));
        harness.wait_for(|snapshot| matches!(snapshot.status, AppUpdateStatus::Restarting { .. }));
        harness.wait_for(|_| *harness.host.relaunches.lock().unwrap() == 1);
    }

    #[test]
    fn a_relaunch_failure_reports_an_error() {
        let harness = Harness::new(InstallMode::Replace);
        harness.stage("0.7.2", b"archive");
        *harness.host.relaunch_error.lock().unwrap() =
            Some("Halv is not running from a macOS app bundle".to_string());

        harness.manager.restart(true).unwrap();
        harness.wait_for(|snapshot| is_error(snapshot, UpdateOperation::Relaunch));
        assert_eq!(harness.source.installs.lock().unwrap().len(), 1);
    }

    #[test]
    fn quitting_installs_the_staged_update_first() {
        let harness = Harness::new(InstallMode::Replace);
        harness.stage("0.7.2", b"archive");

        assert_eq!(
            harness.manager.exit_requested(Some(0)),
            ExitDecision::InstallThenExit
        );
        harness.wait_for(|_| !harness.host.exits.lock().unwrap().is_empty());
        assert_eq!(*harness.host.exits.lock().unwrap(), vec![0]);
        assert_eq!(*harness.host.hidden.lock().unwrap(), 1);
        assert_eq!(
            *harness.source.installs.lock().unwrap(),
            vec![("0.7.2".to_string(), b"archive".to_vec())]
        );
        assert!(harness
            .host
            .published()
            .iter()
            .any(|snapshot| snapshot.status
                == AppUpdateStatus::Installing {
                    version: "0.7.2".to_string(),
                    trigger: InstallTrigger::Quit,
                }));
        assert!(!harness.staged_file("0.7.2").exists());
        // The second exit request passes.
        assert_eq!(harness.manager.exit_requested(Some(0)), ExitDecision::Allow);
    }

    #[test]
    fn a_failed_install_at_quit_still_exits() {
        let harness = Harness::new(InstallMode::Replace);
        harness.stage("0.7.2", b"archive");
        *harness.source.install_error.lock().unwrap() = Some("permission denied".to_string());

        assert_eq!(
            harness.manager.exit_requested(None),
            ExitDecision::InstallThenExit
        );
        harness.wait_for(|_| !harness.host.exits.lock().unwrap().is_empty());
        assert_eq!(*harness.host.exits.lock().unwrap(), vec![0]);
    }

    #[test]
    fn only_an_in_place_replacement_installs_at_quit() {
        for install_mode in [
            InstallMode::Installer,
            InstallMode::Elevated,
            InstallMode::ReadOnly,
            InstallMode::Unsupported,
        ] {
            assert_eq!(
                exit_decision(Some(0), false, false, true, install_mode),
                ExitDecision::Allow,
                "{install_mode:?}"
            );
        }
        assert_eq!(
            exit_decision(Some(0), false, false, true, InstallMode::Replace),
            ExitDecision::InstallThenExit
        );
        assert_eq!(
            exit_decision(None, false, false, true, InstallMode::Replace),
            ExitDecision::InstallThenExit
        );
        assert_eq!(
            exit_decision(Some(0), false, false, false, InstallMode::Replace),
            ExitDecision::Allow
        );
        assert_eq!(
            exit_decision(Some(0), false, true, true, InstallMode::Replace),
            ExitDecision::Prevent
        );
        assert_eq!(
            exit_decision(Some(0), true, true, true, InstallMode::Replace),
            ExitDecision::Allow
        );
        assert_eq!(
            exit_decision(
                Some(tauri::RESTART_EXIT_CODE),
                false,
                true,
                true,
                InstallMode::Replace
            ),
            ExitDecision::Allow
        );
    }

    #[test]
    fn an_exit_during_the_restart_install_waits() {
        let harness = Harness::with_timings(
            InstallMode::Replace,
            Timings {
                flush_timeout: Duration::from_secs(30),
                ..test_timings()
            },
        );
        harness.stage("0.7.2", b"archive");
        harness.manager.restart(true).unwrap();
        assert_eq!(
            harness.manager.exit_requested(Some(0)),
            ExitDecision::Prevent
        );
    }

    #[test]
    fn automatic_downloads_retry_after_the_interval_and_stop_after_three_failures() {
        let harness = Harness::with_timings(
            InstallMode::Replace,
            Timings {
                download_retry_after: Duration::ZERO,
                ..test_timings()
            },
        );
        for attempt in 1..=3 {
            harness.source.push_check(Ok(Some(release("0.7.2"))));
            harness
                .source
                .push_download(FakeDownload::Fail("connection reset".to_string()));
            harness.manager.check(false);
            harness.wait_for(|_| {
                harness.source.download_calls.load(Ordering::SeqCst) == attempt
                    && is_error(&harness.manager.snapshot(), UpdateOperation::Download)
            });
            harness.wait_for_check();
        }

        // The fourth automatic check keeps the error.
        harness.source.push_check(Ok(Some(release("0.7.2"))));
        harness.manager.check(false);
        harness.wait_for(|_| harness.source.check_calls.load(Ordering::SeqCst) == 4);
        harness.wait_for_check();
        assert_eq!(harness.source.download_calls.load(Ordering::SeqCst), 3);
        assert!(is_error(
            &harness.manager.snapshot(),
            UpdateOperation::Download
        ));

        // A newer release starts a new count.
        harness.source.push_check(Ok(Some(release("0.7.3"))));
        harness.source.push_download(chunks(&[b"archive"]));
        harness.manager.check(false);
        harness.wait_for(|snapshot| is_ready(snapshot, "0.7.3"));
    }

    #[test]
    fn turning_off_auto_download_cancels_the_automatic_download() {
        let harness = Harness::with_timings(
            InstallMode::Replace,
            Timings {
                stall_timeout: Duration::from_secs(30),
                ..test_timings()
            },
        );
        harness.source.push_check(Ok(Some(release("0.7.2"))));
        harness.source.push_download(FakeDownload::Hang);
        harness.manager.check(false);
        harness.wait_for(|snapshot| matches!(snapshot.status, AppUpdateStatus::Downloading { .. }));

        let snapshot = harness.manager.set_auto_download(false).unwrap();
        assert!(!snapshot.auto_download);
        assert!(matches!(
            snapshot.status,
            AppUpdateStatus::Available { ref version, .. } if version == "0.7.2"
        ));
        // The manager drops the transfer.
        harness.wait_for(|_| harness.source.ended_downloads.load(Ordering::SeqCst) == 1);
        thread::sleep(Duration::from_millis(30));
        assert!(matches!(
            harness.manager.snapshot().status,
            AppUpdateStatus::Available { .. }
        ));
        assert!(!harness.staged_file("0.7.2").exists());
        assert_eq!(harness.manager.exit_requested(Some(0)), ExitDecision::Allow);
    }

    #[test]
    fn turning_off_auto_download_keeps_a_download_that_the_user_started() {
        let harness = Harness::new(InstallMode::Replace);
        harness.manager.set_auto_download(false).unwrap();
        harness.source.push_check(Ok(Some(release("0.7.2"))));
        harness.manager.check(false);
        harness.wait_for(|snapshot| matches!(snapshot.status, AppUpdateStatus::Available { .. }));

        harness.source.push_download(FakeDownload::Chunks {
            chunks: vec![b"arch".to_vec(), b"ive".to_vec()],
            delay: Duration::from_millis(20),
        });
        let started = harness.manager.download();
        assert!(matches!(
            started.map(|snapshot| snapshot.status),
            Ok(AppUpdateStatus::Downloading { .. })
        ));
        let snapshot = harness.manager.set_auto_download(false).unwrap();
        assert!(matches!(
            snapshot.status,
            AppUpdateStatus::Downloading { .. }
        ));
        harness.wait_for(|snapshot| is_ready(snapshot, "0.7.2"));
    }

    #[test]
    fn a_quit_during_the_restart_install_quits_after_the_install() {
        let harness = Harness::with_timings(
            InstallMode::Replace,
            Timings {
                flush_timeout: Duration::from_secs(30),
                ..test_timings()
            },
        );
        harness.stage("0.7.2", b"archive");
        harness.manager.restart(true).unwrap();
        assert_eq!(
            harness.manager.exit_requested(Some(0)),
            ExitDecision::Prevent
        );
        let request_id = harness.wait_for_flush_request();
        harness.manager.acknowledge_restart(request_id);

        harness.wait_for(|_| !harness.host.exits.lock().unwrap().is_empty());
        assert_eq!(*harness.host.exits.lock().unwrap(), vec![0]);
        assert_eq!(*harness.host.relaunches.lock().unwrap(), 0);
        assert_eq!(harness.source.installs.lock().unwrap().len(), 1);
        assert!(!harness.staged_file("0.7.2").exists());
        assert_eq!(harness.manager.exit_requested(Some(0)), ExitDecision::Allow);
    }

    #[test]
    fn a_quit_during_a_failed_restart_install_still_quits() {
        let harness = Harness::with_timings(
            InstallMode::Replace,
            Timings {
                flush_timeout: Duration::from_secs(30),
                ..test_timings()
            },
        );
        harness.stage("0.7.2", b"archive");
        *harness.source.install_error.lock().unwrap() = Some("disk full".to_string());
        harness.manager.restart(true).unwrap();
        assert_eq!(
            harness.manager.exit_requested(Some(3)),
            ExitDecision::Prevent
        );
        let request_id = harness.wait_for_flush_request();
        harness.manager.acknowledge_restart(request_id);

        harness.wait_for(|_| !harness.host.exits.lock().unwrap().is_empty());
        assert_eq!(*harness.host.exits.lock().unwrap(), vec![3]);
        assert!(is_error(
            &harness.manager.snapshot(),
            UpdateOperation::Install
        ));
        assert_eq!(harness.manager.exit_requested(Some(3)), ExitDecision::Allow);
    }

    #[test]
    fn a_read_only_location_only_checks() {
        let harness = Harness::new(InstallMode::ReadOnly);
        harness.source.push_check(Ok(Some(release("0.7.2"))));
        harness.manager.start_scheduler();
        let available = harness
            .wait_for(|snapshot| matches!(snapshot.status, AppUpdateStatus::Available { .. }));
        assert_eq!(available.install_mode, InstallMode::ReadOnly);
        assert_eq!(harness.source.download_calls.load(Ordering::SeqCst), 0);
        assert_eq!(
            harness.manager.download(),
            Err(UNSUPPORTED_MESSAGE.to_string())
        );
        assert_eq!(
            harness.manager.restart(true),
            Err(UNSUPPORTED_MESSAGE.to_string())
        );
        assert_eq!(harness.manager.exit_requested(Some(0)), ExitDecision::Allow);
    }

    #[test]
    fn the_snapshot_says_when_sessions_end_with_the_restart() {
        let harness = Harness::new(InstallMode::Replace);
        assert!(harness.manager.snapshot().sessions_survive_restart);
        *harness.host.sessions_lost.lock().unwrap() = true;
        assert!(!harness.manager.snapshot().sessions_survive_restart);
    }

    #[test]
    fn a_development_build_only_checks() {
        let harness = Harness::new(InstallMode::Unsupported);
        harness.source.push_check(Ok(Some(release("0.7.2"))));
        harness.manager.check(true);
        let available = harness
            .wait_for(|snapshot| matches!(snapshot.status, AppUpdateStatus::Available { .. }));
        assert_eq!(available.install_mode, InstallMode::Unsupported);
        assert_eq!(harness.source.download_calls.load(Ordering::SeqCst), 0);
        assert_eq!(
            harness.manager.download(),
            Err(UNSUPPORTED_MESSAGE.to_string())
        );
        assert_eq!(
            harness.manager.restart(true),
            Err(UNSUPPORTED_MESSAGE.to_string())
        );
        assert_eq!(harness.manager.exit_requested(Some(0)), ExitDecision::Allow);
    }

    #[test]
    fn later_hides_the_version_until_a_user_check() {
        let harness = Harness::new(InstallMode::Replace);
        harness.stage("0.7.2", b"archive");

        let deferred = harness.manager.defer("0.7.2".to_string());
        assert_eq!(deferred.deferred_version.as_deref(), Some("0.7.2"));
        assert!(is_ready(&deferred, "0.7.2"));

        harness.source.push_check(Ok(Some(release("0.7.2"))));
        let checked = harness.manager.check(true);
        assert_eq!(checked.deferred_version, None);
    }

    #[test]
    fn install_modes_follow_the_bundle_and_its_access() {
        use TargetAccess::{AdminOnly, ReadOnly, Writable};
        let cases = [
            (Some(BundleType::App), Some(Writable), InstallMode::Replace),
            (
                Some(BundleType::App),
                Some(AdminOnly),
                InstallMode::Elevated,
            ),
            (Some(BundleType::App), Some(ReadOnly), InstallMode::ReadOnly),
            (Some(BundleType::App), None, InstallMode::Unsupported),
            (
                Some(BundleType::AppImage),
                Some(Writable),
                InstallMode::Replace,
            ),
            // The AppImage install never asks for an administrator.
            (
                Some(BundleType::AppImage),
                Some(AdminOnly),
                InstallMode::ReadOnly,
            ),
            (
                Some(BundleType::AppImage),
                Some(ReadOnly),
                InstallMode::ReadOnly,
            ),
            (Some(BundleType::AppImage), None, InstallMode::Unsupported),
            (Some(BundleType::Deb), None, InstallMode::Elevated),
            (Some(BundleType::Rpm), Some(Writable), InstallMode::Elevated),
            (Some(BundleType::Msi), None, InstallMode::Installer),
            (Some(BundleType::Nsis), None, InstallMode::Installer),
            (None, Some(Writable), InstallMode::Unsupported),
        ];
        for (bundle, access, expected) in cases {
            let label = format!("{bundle:?} {access:?}");
            assert_eq!(InstallMode::for_bundle(bundle, access), expected, "{label}");
        }
        assert!(InstallMode::ReadOnly.checks_automatically());
        assert!(!InstallMode::ReadOnly.can_install());
        assert!(!InstallMode::Unsupported.checks_automatically());
        assert_eq!(Writable.max(AdminOnly), AdminOnly);
        assert_eq!(AdminOnly.max(ReadOnly), ReadOnly);
        if cfg!(debug_assertions) {
            assert_eq!(InstallMode::detect(), InstallMode::Unsupported);
        }
    }

    #[test]
    fn a_translocated_bundle_is_read_only() {
        assert!(is_translocated(Path::new(
            "/private/var/folders/x/T/AppTranslocation/0A1B/d/Halv.app"
        )));
        assert!(!is_translocated(Path::new("/Applications/Halv.app")));
        assert!(!is_translocated(Path::new("/Volumes/Halv/Halv.app")));
    }

    #[cfg(unix)]
    #[test]
    fn the_access_probe_reads_the_folder_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let directory = TestDirectory(std::env::temp_dir().join(format!(
            "halv-update-access-{:032x}",
            rand::random::<u128>()
        )));
        fs::create_dir_all(&directory.0).unwrap();
        assert_eq!(path_access(&directory.0), TargetAccess::Writable);
        assert_eq!(
            path_access(&directory.0.join("missing")),
            TargetAccess::ReadOnly
        );
        fs::set_permissions(&directory.0, fs::Permissions::from_mode(0o555)).unwrap();
        // The root user can write to any folder.
        // SAFETY: geteuid reads the effective user ID and has no preconditions.
        if unsafe { libc::geteuid() } != 0 {
            assert_eq!(path_access(&directory.0), TargetAccess::AdminOnly);
        }
        fs::set_permissions(&directory.0, fs::Permissions::from_mode(0o755)).unwrap();
    }

    fn boot_entry(app_version: &str) -> BootRecordEntry {
        BootRecordEntry {
            boot_id: format!("boot-{app_version}"),
            app_version: app_version.to_string(),
            started_at: "2026-10-08T19:55:52.639Z".to_string(),
            last_alive_at: "2026-10-08T19:55:52.639Z".to_string(),
            shutdown_at: None,
            shutdown_kind: ShutdownKind::Running,
            max_rss_bytes: None,
        }
    }

    #[test]
    fn the_post_update_note_follows_the_boot_record() {
        let history = [boot_entry("0.7.2"), boot_entry("0.7.1")];
        assert_eq!(
            post_update_from_history(&history, "0.7.2", 11),
            Some(PostUpdate {
                from_version: "0.7.1".to_string(),
                to_version: "0.7.2".to_string(),
                reconnected_sessions: 11,
                release_url: "https://github.com/pedr0v/halv-releases/releases/tag/v0.7.2"
                    .to_string(),
            })
        );
        let same = [boot_entry("0.7.2"), boot_entry("0.7.2")];
        assert_eq!(post_update_from_history(&same, "0.7.2", 1), None);
        let downgrade = [boot_entry("0.7.1"), boot_entry("0.7.2")];
        assert_eq!(post_update_from_history(&downgrade, "0.7.1", 1), None);
        assert_eq!(
            post_update_from_history(&[boot_entry("0.7.2")], "0.7.2", 1),
            None
        );
        assert_eq!(post_update_from_history(&[], "0.7.2", 1), None);
        assert!(version_is_newer("0.10.0", "0.9.12"));
        assert!(!version_is_newer("0.8.0-beta.1", "0.8.0"));
        assert!(version_is_newer("nightly-2", "nightly-1"));
    }

    #[test]
    fn the_snapshot_uses_the_frontend_shape() {
        let harness = Harness::new(InstallMode::Replace);
        harness.manager.lock().post_update = Some(PostUpdate {
            from_version: "0.7.0".to_string(),
            to_version: "0.7.1".to_string(),
            reconnected_sessions: 3,
            release_url: release_url("0.7.1"),
        });
        harness.manager.lock().status = AppUpdateStatus::Downloading {
            version: "0.7.2".to_string(),
            downloaded_bytes: 1024,
            total_bytes: None,
        };
        let value = serde_json::to_value(harness.manager.snapshot()).unwrap();
        assert_eq!(
            value,
            serde_json::json!({
                "status": {
                    "state": "downloading",
                    "version": "0.7.2",
                    "downloadedBytes": 1024,
                    "totalBytes": null,
                },
                "currentVersion": "0.7.1",
                "autoDownload": true,
                "installMode": "replace",
                "deferredVersion": null,
                "restartWhenIdle": false,
                "blockers": null,
                "postUpdate": {
                    "fromVersion": "0.7.0",
                    "toVersion": "0.7.1",
                    "reconnectedSessions": 3,
                    "releaseUrl": "https://github.com/pedr0v/halv-releases/releases/tag/v0.7.1",
                },
                "lastCheckedAt": null,
                "sessionsSurviveRestart": true,
            })
        );

        let error = serde_json::to_value(AppUpdateStatus::Error {
            operation: UpdateOperation::Check,
            message: "offline".to_string(),
            version: None,
            retryable: true,
            user_initiated: false,
        })
        .unwrap();
        assert_eq!(
            error,
            serde_json::json!({
                "state": "error",
                "operation": "check",
                "message": "offline",
                "retryable": true,
                "userInitiated": false,
            })
        );
        assert_eq!(
            serde_json::to_value(AppUpdateStatus::UpToDate {
                user_initiated: true
            })
            .unwrap(),
            serde_json::json!({ "state": "upToDate", "userInitiated": true })
        );
        assert_eq!(
            serde_json::to_value(AppUpdateStatus::Installing {
                version: "0.7.2".to_string(),
                trigger: InstallTrigger::Quit,
            })
            .unwrap(),
            serde_json::json!({ "state": "installing", "version": "0.7.2", "trigger": "quit" })
        );
        assert_eq!(
            serde_json::to_value(RestartBlockers {
                chat_replies: 1,
                gui_agent_sessions: 2,
                in_process_terminals: 3,
            })
            .unwrap(),
            serde_json::json!({ "chatReplies": 1, "guiAgentSessions": 2, "inProcessTerminals": 3 })
        );
        assert_eq!(
            serde_json::to_value(BeforeRestartPayload { request_id: 7 }).unwrap(),
            serde_json::json!({ "requestId": 7 })
        );
    }
}
