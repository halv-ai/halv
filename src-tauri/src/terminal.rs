#[cfg(target_os = "linux")]
use crate::host_env;
use crate::{
    projects,
    storage::Storage,
    terminal_state::{
        ScopedTerminalPanelState, TerminalPaneStateInput, TerminalPanelState, TerminalPersistence,
        MAX_PERSISTED_SCROLLBACK_BYTES,
    },
};
use chrono::Utc;
use halv_supervisor::ScrollbackRing;
#[cfg(unix)]
use halv_supervisor::{
    Event as SupervisorEvent, OpenRequest, SessionStatus, SessionSummary, SupervisorClient,
};
use portable_pty::{native_pty_system, ChildKiller, CommandBuilder, MasterPty, PtySize};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    ffi::OsString,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};
use tauri::{AppHandle, Emitter};

mod input;

use input::{InputOrder, InputQueue, InputRequest, InputSink};
use tokio::sync::oneshot;

// A runaway backstop, not a product limit — Orca runs with no cap at all.
const MAX_TERMINAL_SESSIONS: usize = 32;
const SCROLLBACK_FLUSH_INTERVAL: Duration = Duration::from_secs(2);
const CONVERSATION_ACTIVITY_THROTTLE: Duration = Duration::from_secs(5);
const MAX_TERMINAL_WRITE_BYTES: usize = 64 * 1024;
const MAX_SCROLLBACK_BYTES: usize = MAX_PERSISTED_SCROLLBACK_BYTES;
const TERMINAL_FORGET_RUNNING_ERROR: &str = "close the terminal before removing it";
const TERMINALS_STOPPING_ERROR: &str = "terminals are stopping";
#[cfg(unix)]
const SUPERVISOR_FALLBACK_INTERVAL: Duration = Duration::from_secs(30);
#[cfg(unix)]
const SUPERVISOR_PROTOCOL_FALLBACK_INTERVAL: Duration = Duration::from_secs(5 * 60);
#[cfg(unix)]
const SUPERVISOR_RECONNECT_DELAYS: [Duration; 5] = [
    Duration::from_millis(250),
    Duration::from_millis(500),
    Duration::from_secs(1),
    Duration::from_secs(2),
    Duration::from_secs(5),
];
#[cfg(unix)]
const SUPERVISOR_RECONNECT_STABLE_INTERVAL: Duration = Duration::from_secs(30);
#[cfg(unix)]
const SUPERVISOR_ATTACH_TIMEOUT: Duration = Duration::from_secs(10);

type TerminalResult<T> = Result<T, String>;
type TerminalEventSink = Arc<dyn Fn(TerminalEvent) + Send + Sync>;
#[cfg(unix)]
type SupervisorConnectionEventSink = Arc<dyn Fn(SupervisorConnectionEvent) + Send + Sync>;

#[cfg(unix)]
struct SupervisorAttachSinks {
    terminal: TerminalEventSink,
    connection: SupervisorConnectionEventSink,
}

#[cfg(unix)]
struct SupervisorAttachHooks<Ensure, Attach, Wait> {
    ensure: Ensure,
    attach: Attach,
    wait: Wait,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ConversationActivityEvent {
    conversation_id: String,
}

pub(crate) fn emit_conversation_activity(
    app: &AppHandle,
    conversation_id: &str,
) -> Result<(), tauri::Error> {
    app.emit(
        "conversation-activity",
        ConversationActivityEvent {
            conversation_id: conversation_id.to_string(),
        },
    )
}

fn terminal_event_sink(app: &AppHandle) -> TerminalEventSink {
    let app = app.clone();
    Arc::new(move |event| {
        if let (Some(data), None) = (event.data.as_deref(), event.replay) {
            // Codex reports approvals and questions in its live output.
            crate::agent_status::note_terminal_output(&app, &event.session_id, data);
        }
        tap_terminal_event(&event);
        let _ = app.emit("terminal_event", event);
    })
}

/// A copy of a terminal event for the remote gateway (`crate::remote`).
pub(crate) enum TerminalTapEvent<'a> {
    /// Output that the ring already holds under this sequence number.
    Data {
        session_id: &'a str,
        sequence: u64,
        data: &'a str,
    },
    /// The process exited.
    Exit {
        session_id: &'a str,
        exit_code: Option<i32>,
        signal: Option<&'a str>,
    },
    /// A terminal failed, or its title changed.
    Changed,
    /// The PTY size changed.
    Resized {
        session_id: &'a str,
        cols: u16,
        rows: u16,
    },
    /// The session left the manager.
    Removed { session_id: &'a str },
}

/// Receives each tap event. Reader threads call it, so it must return at once.
pub(crate) type TerminalTap = Arc<dyn Fn(TerminalTapEvent<'_>) + Send + Sync>;

static TERMINAL_TAP: std::sync::RwLock<Option<TerminalTap>> = std::sync::RwLock::new(None);

/// Sets the tap of the remote gateway. `None` removes it.
pub(crate) fn set_terminal_tap(tap: Option<TerminalTap>) {
    *TERMINAL_TAP
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = tap;
}

fn notify_terminal_tap(event: TerminalTapEvent<'_>) {
    let tap = TERMINAL_TAP
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    if let Some(tap) = tap {
        tap(event);
    }
}

fn tap_terminal_event(event: &TerminalEvent) {
    let session_id = event.session_id.as_str();
    let tap_event = match (event.kind, event.data.as_deref()) {
        ("data", Some(data)) => TerminalTapEvent::Data {
            session_id,
            sequence: event.sequence,
            data,
        },
        ("data", None) => return,
        ("exit", _) => TerminalTapEvent::Exit {
            session_id,
            exit_code: event.exit_code,
            signal: event.signal.as_deref(),
        },
        _ => TerminalTapEvent::Changed,
    };
    notify_terminal_tap(tap_event);
}

/// The scrollback ring of a session for the remote gateway.
pub(crate) struct RemoteRing {
    pub sequence: u64,
    pub cols: u16,
    pub rows: u16,
    pub data: String,
}

/// Returns the last sequence number and the output after `after`.
/// Returns `None` when the ring no longer holds all of that output.
pub(crate) fn ring_since(output: &ScrollbackRing, after: u64) -> Option<(u64, String)> {
    let (sequence, full) = output.snapshot(None);
    let (_, data) = output.snapshot(Some(after));
    // Trimming removes the oldest bytes first. When the ring still holds bytes of a
    // chunk at or before `after`, it holds every chunk after `after`.
    let complete = after == sequence || (after < sequence && data.len() < full.len());
    complete.then_some((sequence, data))
}

#[cfg(unix)]
fn supervisor_connection_event_sink(app: &AppHandle) -> SupervisorConnectionEventSink {
    let app = app.clone();
    Arc::new(move |event| match event {
        SupervisorConnectionEvent::Reconnecting(payload) => {
            let _ = app.emit("terminal_supervisor_reconnecting", payload);
        }
        SupervisorConnectionEvent::Reconnected(payload) => {
            let _ = app.emit("terminal_supervisor_reconnected", payload);
        }
    })
}

fn input_marks_activity(data: &str) -> bool {
    data.contains('\r') || data.contains('\n')
}

/// The supervisor keeps this record as session metadata. A newer app must
/// decode the metadata of an older app, so a new field must be an `Option`
/// or use `#[serde(default)]`. See ADR 0008.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TerminalSession {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub project_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conversation_id: Option<String>,
    pub project_name: String,
    pub cwd: String,
    pub shell: String,
    pub pid: u32,
    pub execution_profile: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_kind: Option<String>,
    pub status: String,
    pub cols: u16,
    pub rows: u16,
    pub started_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exited_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signal: Option<String>,
    // The ended session that this shell continues in the same pane. The
    // field travels in the supervisor metadata. Old metadata parses as None.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub continued_from: Option<String>,
    // The shell tab that owns this session (ADR 0010): the uuid of the tab
    // key `shell:<uuid>`. A session has one owner at most, so a session with
    // a conversation has no shell tab. Old metadata parses as None.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shell_tab_id: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalAttach {
    pub session: TerminalSession,
    pub sequence: u64,
    pub scrollback: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prelude: Option<TerminalPrelude>,
}

/// The last output of the ended session that a continuation shell replaces.
/// The pane shows it above the shell prompt. It lives in memory only.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TerminalPrelude {
    pub scrollback: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
}

/// A link from a new shell to the ended session that it continues.
struct TerminalContinuation {
    continued_from: String,
    prelude: TerminalPrelude,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct TerminalContinuedEvent {
    from_session_id: String,
    session: TerminalSession,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct TerminalEvent {
    session_id: String,
    sequence: u64,
    kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    replay: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    exit_code: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    signal: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<String>,
}

struct TerminalFinish {
    sequence: Option<u64>,
    status: &'static str,
    exit_code: Option<i32>,
    signal: Option<String>,
    message: Option<String>,
}

#[cfg(unix)]
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct TerminalSupervisorFallbackEvent {
    message: String,
}

#[cfg(unix)]
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
struct TerminalSupervisorReconnectingEvent {
    id: String,
    attempt: u32,
    message: String,
}

#[cfg(unix)]
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
struct TerminalSupervisorReconnectedEvent {
    id: String,
    attempt: u32,
}

#[cfg(unix)]
#[derive(Debug, Clone, PartialEq)]
enum SupervisorConnectionEvent {
    Reconnecting(TerminalSupervisorReconnectingEvent),
    Reconnected(TerminalSupervisorReconnectedEvent),
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalSupervisorStatus {
    available: bool,
    socket_path: String,
    pid: Option<u32>,
    sessions: usize,
    running_sessions: usize,
    supervisor_version: Option<String>,
    supervisor_stale: bool,
    app_version: String,
    message: Option<String>,
}

/// The terminal supervisor commands return this report.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalSupervisorReport {
    #[serde(flatten)]
    status: TerminalSupervisorStatus,
    /// A supervisor with another protocol still holds the socket. Its agents
    /// can still run, so the panel must not resume them. See ADR 0008.
    protocol_mismatch: bool,
}

#[cfg(unix)]
const SUPERVISOR_RUNNING_SESSIONS_ERROR: &str = "terminal sessions are still running";

#[cfg(unix)]
trait SupervisorRestartClient {
    fn session_counts(&mut self) -> TerminalResult<(usize, usize)>;
    fn shutdown(&mut self, kill_sessions: bool) -> TerminalResult<()>;
}

#[cfg(unix)]
impl SupervisorRestartClient for SupervisorClient {
    fn session_counts(&mut self) -> TerminalResult<(usize, usize)> {
        let sessions = self.list()?;
        let running_sessions = sessions
            .iter()
            .filter(|session| session.status == SessionStatus::Running)
            .count();
        Ok((sessions.len(), running_sessions))
    }

    fn shutdown(&mut self, kill_sessions: bool) -> TerminalResult<()> {
        SupervisorClient::shutdown(self, kill_sessions)
    }
}

#[cfg(unix)]
enum SupervisorRestartOutcome<Client> {
    Restarted(Client),
    Refused {
        client: Client,
        sessions: usize,
        running_sessions: usize,
        message: String,
    },
    Failed {
        client: Client,
        sessions: usize,
        running_sessions: usize,
        message: String,
    },
}

#[cfg(unix)]
fn restart_supervisor_with<Client>(
    mut client: Client,
    kill_sessions: bool,
    wait_for_release: impl FnOnce() -> TerminalResult<()>,
    start_supervisor: impl FnOnce() -> TerminalResult<Client>,
) -> TerminalResult<SupervisorRestartOutcome<Client>>
where
    Client: SupervisorRestartClient,
{
    let (sessions, running_sessions) = client.session_counts()?;
    match client.shutdown(kill_sessions) {
        Ok(()) => {
            wait_for_release()?;
            start_supervisor().map(SupervisorRestartOutcome::Restarted)
        }
        Err(message)
            if !kill_sessions
                && running_sessions > 0
                && message == SUPERVISOR_RUNNING_SESSIONS_ERROR =>
        {
            Ok(SupervisorRestartOutcome::Refused {
                client,
                sessions,
                running_sessions,
                message,
            })
        }
        Err(message) => Ok(SupervisorRestartOutcome::Failed {
            client,
            sessions,
            running_sessions,
            message,
        }),
    }
}

#[cfg(unix)]
enum StaleSwapOutcome<Client> {
    /// The supervisor is current, runs a session, or refused the shutdown.
    Kept(Client),
    /// A new supervisor replaced the stale one.
    Swapped(Client),
    /// The app has no supervisor connection after a failed swap.
    Lost(String),
}

/// Replaces a stale supervisor only when it runs no session. The shutdown is
/// non-destructive, so the supervisor refuses it when a session started in
/// the meantime. Then the old supervisor keeps its sessions.
/// `app_running_sessions` counts the sessions that the app still shows as
/// running. The supervisor keeps the last output and the exit status of a
/// session until the app reads them, so the swap waits for them too.
#[cfg(unix)]
fn swap_stale_supervisor_with<Client>(
    mut client: Client,
    stale: bool,
    app_running_sessions: usize,
    wait_for_release: impl FnOnce() -> TerminalResult<()>,
    start_supervisor: impl FnOnce() -> TerminalResult<Client>,
) -> StaleSwapOutcome<Client>
where
    Client: SupervisorRestartClient,
{
    if !stale || app_running_sessions > 0 {
        return StaleSwapOutcome::Kept(client);
    }
    match client.session_counts() {
        Ok((_, 0)) => {}
        Ok(_) => return StaleSwapOutcome::Kept(client),
        Err(error) => {
            eprintln!("terminal supervisor: failed to inspect a stale supervisor: {error}");
            return StaleSwapOutcome::Kept(client);
        }
    }
    match restart_supervisor_with(client, false, wait_for_release, start_supervisor) {
        Ok(SupervisorRestartOutcome::Restarted(client)) => StaleSwapOutcome::Swapped(client),
        Ok(
            SupervisorRestartOutcome::Refused { client, .. }
            | SupervisorRestartOutcome::Failed { client, .. },
        ) => StaleSwapOutcome::Kept(client),
        Err(message) => StaleSwapOutcome::Lost(message),
    }
}

#[cfg(unix)]
const SUPERVISOR_VERSION_MISMATCH_PREFIX: &str = "Supervisor is outdated (";
#[cfg(unix)]
const SUPERVISOR_BUILD_MISMATCH_MESSAGE: &str =
    "Supervisor is from an older build. Restart it to update.";

#[cfg(unix)]
fn is_supervisor_stale(
    supervisor_fingerprint: Option<&str>,
    supervisor_version: &str,
    app_version: &str,
) -> bool {
    supervisor_fingerprint.map_or(supervisor_version != app_version, |fingerprint| {
        fingerprint != halv_supervisor::SUPERVISOR_FINGERPRINT
    })
}

#[cfg(unix)]
fn supervisor_version_mismatch_message(
    supervisor_stale: bool,
    supervisor_version: &str,
    app_version: &str,
) -> Option<String> {
    if !supervisor_stale {
        None
    } else if supervisor_version != app_version {
        Some(format!(
            "{SUPERVISOR_VERSION_MISMATCH_PREFIX}{supervisor_version}). Restart it to use Halv {app_version}."
        ))
    } else {
        Some(SUPERVISOR_BUILD_MISMATCH_MESSAGE.to_string())
    }
}

#[cfg(unix)]
fn is_supervisor_version_mismatch(message: &str) -> bool {
    message.starts_with(SUPERVISOR_VERSION_MISMATCH_PREFIX)
        || message == SUPERVISOR_BUILD_MISMATCH_MESSAGE
}

enum Backend {
    InProcess {
        master: Mutex<Box<dyn MasterPty + Send>>,
        writer: Mutex<Option<Box<dyn Write + Send>>>,
        killer: Mutex<Option<Box<dyn ChildKiller + Send + Sync>>>,
    },
    #[cfg(unix)]
    Supervised,
}

struct ManagedTerminal {
    record: Mutex<TerminalSession>,
    output: Mutex<ScrollbackRing>,
    closing: AtomicBool,
    // A bulk kill (supervisor restart, quit) sets this mark. Unlike
    // `closing`, the mark does not stop the supervisor attach thread.
    stop_requested: AtomicBool,
    // A continuation shell gets its prelude once, at spawn.
    prelude: Option<TerminalPrelude>,
    // On an ended session: the id of the shell that continues it.
    continuation: Mutex<Option<String>>,
    backend: Backend,
}

impl ManagedTerminal {
    fn snapshot(&self) -> TerminalSession {
        self.record
            .lock()
            .expect("terminal record lock poisoned")
            .clone()
    }

    fn attach(&self) -> TerminalAttach {
        let (sequence, scrollback) = self
            .output
            .lock()
            .expect("terminal output lock poisoned")
            .snapshot(None);
        let session = self.snapshot();
        TerminalAttach {
            session,
            sequence,
            scrollback,
            prelude: None,
        }
    }

    // Only the webview attach needs the prelude. Internal snapshots (for
    // example the scrollback persistence on each output chunk) skip the copy.
    fn attach_with_prelude(&self) -> TerminalAttach {
        TerminalAttach {
            prelude: self.prelude.clone(),
            ..self.attach()
        }
    }

    fn emit_data(&self, sink: &TerminalEventSink, manager: &TerminalManager, data: &[u8]) {
        let output = self
            .output
            .lock()
            .expect("terminal output lock poisoned")
            .append(data);
        let Some((sequence, data)) = output else {
            return;
        };
        manager.persist_scrollback(self);
        let event = TerminalEvent {
            session_id: self.snapshot().id,
            sequence,
            kind: "data",
            data: Some(data),
            replay: None,
            exit_code: None,
            signal: None,
            message: None,
        };
        sink(event);
    }

    #[cfg(unix)]
    fn emit_supervised_data(
        &self,
        sink: &TerminalEventSink,
        manager: &TerminalManager,
        sequence: u64,
        data: &str,
        replay: bool,
    ) {
        let output = self
            .output
            .lock()
            .expect("terminal output lock poisoned")
            .append_decoded(sequence, data);
        let Some((sequence, data)) = output else {
            return;
        };
        manager.persist_scrollback(self);
        sink(TerminalEvent {
            session_id: self.snapshot().id,
            sequence,
            kind: "data",
            data: Some(data),
            replay: replay.then_some(true),
            exit_code: None,
            signal: None,
            message: None,
        });
    }

    fn emit_terminal(
        &self,
        sink: &TerminalEventSink,
        sequence: Option<u64>,
        kind: &'static str,
        exit_code: Option<i32>,
        signal: Option<String>,
        message: Option<String>,
    ) {
        let sequence = {
            let mut output = self.output.lock().expect("terminal output lock poisoned");
            if let Some(sequence) = sequence {
                output.set_sequence(sequence);
                sequence
            } else {
                output.next_sequence()
            }
        };
        let event = TerminalEvent {
            session_id: self.snapshot().id,
            sequence,
            kind,
            data: None,
            replay: None,
            exit_code,
            signal,
            message,
        };
        sink(event);
    }

    fn finish(
        &self,
        sink: &TerminalEventSink,
        manager: &TerminalManager,
        completion: TerminalFinish,
    ) {
        if let Backend::InProcess { writer, killer, .. } = &self.backend {
            writer.lock().expect("terminal writer lock poisoned").take();
            killer.lock().expect("terminal killer lock poisoned").take();
        }
        self.finish_record(
            completion.status,
            completion.exit_code,
            completion.signal.clone(),
        );
        manager.persist_scrollback_now(self);
        self.emit_terminal(
            sink,
            completion.sequence,
            if completion.status == "exited" {
                "exit"
            } else {
                "error"
            },
            completion.exit_code,
            completion.signal,
            completion.message,
        );
    }

    fn finish_record(&self, status: &str, exit_code: Option<i32>, signal: Option<String>) {
        {
            let mut record = self.record.lock().expect("terminal record lock poisoned");
            record.status = status.to_string();
            record.exited_at = Some(Utc::now().to_rfc3339());
            record.exit_code = exit_code;
            record.signal.clone_from(&signal);
        }
    }
}

#[cfg(unix)]
struct SupervisorFallback {
    decided_at: Instant,
    message: String,
    retry_after: Duration,
}

#[derive(Clone, Default)]
pub struct TerminalManager {
    sessions: Arc<Mutex<HashMap<String, Arc<ManagedTerminal>>>>,
    persistence: Arc<Mutex<TerminalPersistence>>,
    scrollback_dirty: Arc<AtomicBool>,
    flusher_started: Arc<AtomicBool>,
    conversation_activity: Arc<Mutex<HashMap<String, Instant>>>,
    /// One ordered input queue for each session that received input.
    inputs: Arc<Mutex<HashMap<String, InputQueue>>>,
    #[cfg(unix)]
    app_data_dir: Arc<Mutex<Option<PathBuf>>>,
    #[cfg(unix)]
    supervisor: Arc<Mutex<Option<SupervisorClient>>>,
    #[cfg(unix)]
    supervisor_fallback: Arc<Mutex<Option<SupervisorFallback>>>,
    #[cfg(unix)]
    supervisor_protocol_mismatch: Arc<Mutex<Option<String>>>,
    #[cfg(unix)]
    stale_supervisor_swapped: Arc<AtomicBool>,
    // Set when the app quits. A late continuation shell must not start a
    // supervisor during the quit.
    stopping: Arc<AtomicBool>,
}

pub(crate) struct TerminalProgram {
    pub program: PathBuf,
    pub args: Vec<OsString>,
    pub env: Vec<(OsString, OsString)>,
    pub env_remove: Vec<OsString>,
}

#[derive(Clone)]
struct ResolvedCommand {
    program: PathBuf,
    args: Vec<OsString>,
    env: Vec<(OsString, OsString)>,
    env_remove: Vec<OsString>,
}

pub(crate) struct TerminalSpawnRequest {
    pub id: Option<String>,
    pub project_id: Option<i64>,
    pub conversation_id: Option<String>,
    pub project_name: String,
    pub cwd: PathBuf,
    pub cols: u16,
    pub rows: u16,
    pub execution_profile: String,
    pub agent_kind: Option<String>,
    pub program: Option<TerminalProgram>,
    /// The shell tab that owns the session. Every request sets the owner
    /// explicitly, so no spawn path forgets it (ADR 0010).
    pub shell_tab_id: Option<String>,
}

impl TerminalManager {
    #[cfg(unix)]
    fn supervisor_app_data_dir(&self) -> TerminalResult<PathBuf> {
        self.app_data_dir
            .lock()
            .expect("terminal app data lock poisoned")
            .clone()
            .ok_or_else(|| {
                "failed to connect to supervisor: app data directory is not set".to_string()
            })
    }

    #[cfg(unix)]
    fn with_supervisor<T>(
        &self,
        app_version: &str,
        operation: impl FnOnce(&mut SupervisorClient) -> TerminalResult<T>,
    ) -> TerminalResult<T> {
        let app_data_dir = self.supervisor_app_data_dir()?;
        let mut client = self
            .supervisor
            .lock()
            .expect("terminal supervisor lock poisoned");
        // The quit sets `stopping` before it takes this lock. A late call,
        // for example a continuation shell, then cannot start a supervisor
        // that outlives the quit.
        if client.is_none() && self.stopping.load(Ordering::SeqCst) {
            return Err(TERMINALS_STOPPING_ERROR.to_string());
        }
        if client.is_none() {
            *client = Some(crate::supervisor_link::ensure_running(
                &app_data_dir,
                app_version,
            )?);
        }
        let result = operation(client.as_mut().expect("terminal supervisor must exist"));
        if result
            .as_ref()
            .err()
            .is_some_and(|error| crate::supervisor_link::is_broken_connection(error))
        {
            client.take();
        }
        result
    }

    #[cfg(unix)]
    fn fallback_message(&self) -> Option<String> {
        let mut fallback = self
            .supervisor_fallback
            .lock()
            .expect("terminal supervisor fallback lock poisoned");
        if fallback
            .as_ref()
            .is_some_and(|fallback| fallback.decided_at.elapsed() < fallback.retry_after)
        {
            return fallback.as_ref().map(|fallback| fallback.message.clone());
        }
        fallback.take();
        None
    }

    #[cfg(unix)]
    fn record_fallback(&self, message: String) -> bool {
        let mut fallback = self
            .supervisor_fallback
            .lock()
            .expect("terminal supervisor fallback lock poisoned");
        let emit = !fallback
            .as_ref()
            .is_some_and(|fallback| fallback.decided_at.elapsed() < fallback.retry_after);
        if emit {
            let retry_after = if crate::supervisor_link::is_protocol_mismatch(&message) {
                SUPERVISOR_PROTOCOL_FALLBACK_INTERVAL
            } else {
                SUPERVISOR_FALLBACK_INTERVAL
            };
            *fallback = Some(SupervisorFallback {
                decided_at: Instant::now(),
                message,
                retry_after,
            });
        }
        emit
    }

    #[cfg(unix)]
    fn clear_fallback(&self) {
        self.supervisor_fallback
            .lock()
            .expect("terminal supervisor fallback lock poisoned")
            .take();
    }

    #[cfg(unix)]
    fn retry_supervisor(&self) -> TerminalSupervisorStatus {
        self.clear_fallback();
        let app_data_dir = match self.supervisor_app_data_dir() {
            Ok(app_data_dir) => app_data_dir,
            Err(message) => {
                return TerminalSupervisorStatus {
                    available: false,
                    socket_path: String::new(),
                    pid: None,
                    sessions: 0,
                    running_sessions: 0,
                    supervisor_version: None,
                    supervisor_stale: false,
                    app_version: env!("CARGO_PKG_VERSION").to_string(),
                    message: Some(message),
                };
            }
        };
        let socket_path = crate::supervisor_link::socket_path(&app_data_dir)
            .to_string_lossy()
            .into_owned();
        self.supervisor
            .lock()
            .expect("terminal supervisor lock poisoned")
            .take();
        match crate::supervisor_link::ensure_running(&app_data_dir, env!("CARGO_PKG_VERSION")) {
            Ok(client) => {
                *self
                    .supervisor
                    .lock()
                    .expect("terminal supervisor lock poisoned") = Some(client);
                self.supervisor_status()
            }
            Err(message) => {
                self.record_fallback(message.clone());
                TerminalSupervisorStatus {
                    available: false,
                    socket_path,
                    pid: None,
                    sessions: 0,
                    running_sessions: 0,
                    supervisor_version: None,
                    supervisor_stale: false,
                    app_version: env!("CARGO_PKG_VERSION").to_string(),
                    message: Some(message),
                }
            }
        }
    }

    #[cfg(unix)]
    fn restart_supervisor(&self, kill_sessions: bool) -> TerminalSupervisorStatus {
        self.clear_fallback();
        let app_data_dir = match self.supervisor_app_data_dir() {
            Ok(app_data_dir) => app_data_dir,
            Err(message) => {
                return TerminalSupervisorStatus {
                    available: false,
                    socket_path: String::new(),
                    pid: None,
                    sessions: 0,
                    running_sessions: 0,
                    supervisor_version: None,
                    supervisor_stale: false,
                    app_version: env!("CARGO_PKG_VERSION").to_string(),
                    message: Some(message),
                };
            }
        };
        let socket_path = crate::supervisor_link::socket_path(&app_data_dir)
            .to_string_lossy()
            .into_owned();
        let mut cached_client = self
            .supervisor
            .lock()
            .expect("terminal supervisor lock poisoned");
        let client = match cached_client.take() {
            Some(client) => client,
            None => match crate::supervisor_link::ensure_running(
                &app_data_dir,
                env!("CARGO_PKG_VERSION"),
            ) {
                Ok(client) => client,
                Err(message) => {
                    self.record_fallback(message.clone());
                    return TerminalSupervisorStatus {
                        available: false,
                        socket_path,
                        pid: None,
                        sessions: 0,
                        running_sessions: 0,
                        supervisor_version: None,
                        supervisor_stale: false,
                        app_version: env!("CARGO_PKG_VERSION").to_string(),
                        message: Some(message),
                    };
                }
            },
        };
        // A killed session must not continue in a shell.
        let marked = if kill_sessions {
            self.mark_supervised_sessions_stopped()
        } else {
            Vec::new()
        };
        let outcome = restart_supervisor_with(
            client,
            kill_sessions,
            || crate::supervisor_link::wait_for_socket_release(&app_data_dir),
            || crate::supervisor_link::ensure_running(&app_data_dir, env!("CARGO_PKG_VERSION")),
        );
        match outcome {
            Ok(SupervisorRestartOutcome::Restarted(mut client)) => {
                let pid = client.supervisor_pid();
                let supervisor_version = client.supervisor_version().to_string();
                let supervisor_stale = is_supervisor_stale(
                    client.supervisor_fingerprint(),
                    &supervisor_version,
                    env!("CARGO_PKG_VERSION"),
                );
                match client.session_counts() {
                    Ok((sessions, running_sessions)) => {
                        let message = supervisor_version_mismatch_message(
                            supervisor_stale,
                            &supervisor_version,
                            env!("CARGO_PKG_VERSION"),
                        );
                        *cached_client = Some(client);
                        TerminalSupervisorStatus {
                            available: true,
                            socket_path,
                            pid: Some(pid),
                            sessions,
                            running_sessions,
                            supervisor_version: Some(supervisor_version),
                            supervisor_stale,
                            app_version: env!("CARGO_PKG_VERSION").to_string(),
                            message,
                        }
                    }
                    Err(message) => TerminalSupervisorStatus {
                        available: false,
                        socket_path,
                        pid: None,
                        sessions: 0,
                        running_sessions: 0,
                        supervisor_version: None,
                        supervisor_stale: false,
                        app_version: env!("CARGO_PKG_VERSION").to_string(),
                        message: Some(message),
                    },
                }
            }
            Ok(SupervisorRestartOutcome::Refused {
                mut client,
                sessions,
                running_sessions,
                message,
            })
            | Ok(SupervisorRestartOutcome::Failed {
                mut client,
                sessions,
                running_sessions,
                message,
            }) => {
                self.release_surviving_stop_marks(&marked, Some(&mut client));
                let pid = client.supervisor_pid();
                let supervisor_version = client.supervisor_version().to_string();
                let supervisor_stale = is_supervisor_stale(
                    client.supervisor_fingerprint(),
                    &supervisor_version,
                    env!("CARGO_PKG_VERSION"),
                );
                *cached_client = Some(client);
                TerminalSupervisorStatus {
                    available: true,
                    socket_path,
                    pid: Some(pid),
                    sessions,
                    running_sessions,
                    supervisor_version: Some(supervisor_version),
                    supervisor_stale,
                    app_version: env!("CARGO_PKG_VERSION").to_string(),
                    message: Some(message),
                }
            }
            Err(message) => {
                // The error can come before the kill (a stale client) or
                // after it (a failed start). A new connection finds out which
                // sessions still run.
                self.release_surviving_stop_marks(&marked, None);
                self.record_fallback(message.clone());
                TerminalSupervisorStatus {
                    available: false,
                    socket_path,
                    pid: None,
                    sessions: 0,
                    running_sessions: 0,
                    supervisor_version: None,
                    supervisor_stale: false,
                    app_version: env!("CARGO_PKG_VERSION").to_string(),
                    message: Some(message),
                }
            }
        }
    }

    #[cfg(not(unix))]
    fn retry_supervisor(&self) -> TerminalSupervisorStatus {
        self.supervisor_status()
    }

    #[cfg(not(unix))]
    fn restart_supervisor(&self, _kill_sessions: bool) -> TerminalSupervisorStatus {
        let mut status = self.supervisor_status();
        status.message = Some("terminal supervisor restart is unavailable".to_string());
        status
    }

    fn mark_conversation_activity(&self, conversation_id: &str) -> bool {
        let now = Instant::now();
        let mut activity = self
            .conversation_activity
            .lock()
            .expect("conversation activity lock poisoned");
        if activity
            .get(conversation_id)
            .is_some_and(|last| now.duration_since(*last) < CONVERSATION_ACTIVITY_THROTTLE)
        {
            return false;
        }
        activity.insert(conversation_id.to_string(), now);
        true
    }

    pub fn set_app_data_dir(&self, app_data_dir: &Path) -> TerminalResult<()> {
        #[cfg(unix)]
        {
            *self
                .app_data_dir
                .lock()
                .expect("terminal app data lock poisoned") = Some(app_data_dir.to_path_buf());
        }
        let persistence = TerminalPersistence::open(app_data_dir)
            .map_err(|error| format!("failed to open terminal state: {error}"))?;
        *self
            .persistence
            .lock()
            .expect("terminal persistence lock poisoned") = persistence;
        self.start_scrollback_flusher();
        Ok(())
    }

    #[cfg(unix)]
    pub fn adopt_supervised_sessions(&self, app: &AppHandle) {
        let app_version = app.package_info().version.to_string();
        if let Err(error) = self.adopt_supervised_sessions_with_sink(
            &app_version,
            terminal_event_sink(app),
            supervisor_connection_event_sink(app),
        ) {
            eprintln!("failed to adopt supervised terminals: {error}");
        }
        // The swap can wait for the old socket, so it must not block setup.
        let manager = self.clone();
        thread::spawn(move || {
            manager.maybe_swap_stale_supervisor();
        });
    }

    /// Replaces a supervisor from an older build when it runs no session.
    /// The app calls it after adoption and after each supervised session
    /// exits. It swaps at most once for each app run. See ADR 0008.
    #[cfg(unix)]
    pub(crate) fn maybe_swap_stale_supervisor(&self) -> bool {
        if self.stale_supervisor_swapped.load(Ordering::SeqCst) {
            return false;
        }
        let Ok(app_data_dir) = self.supervisor_app_data_dir() else {
            return false;
        };
        // Count before the supervisor lock, so the two locks never nest.
        let app_running_sessions = self.running_supervised_session_count();
        let mut cached_client = self
            .supervisor
            .lock()
            .expect("terminal supervisor lock poisoned");
        // The quit sets `stopping` before it takes this lock. The bulk kill of
        // "Quit and Stop Agents" ends each session, so without this check the
        // last exit could start a new supervisor that outlives the quit.
        if self.stopping.load(Ordering::SeqCst) {
            return false;
        }
        let Some(client) = cached_client.take() else {
            return false;
        };
        let stale = is_supervisor_stale(
            client.supervisor_fingerprint(),
            client.supervisor_version(),
            env!("CARGO_PKG_VERSION"),
        );
        match swap_stale_supervisor_with(
            client,
            stale,
            app_running_sessions,
            || crate::supervisor_link::wait_for_socket_release(&app_data_dir),
            || crate::supervisor_link::ensure_running(&app_data_dir, env!("CARGO_PKG_VERSION")),
        ) {
            StaleSwapOutcome::Kept(client) => {
                *cached_client = Some(client);
                false
            }
            StaleSwapOutcome::Swapped(client) => {
                eprintln!(
                    "terminal supervisor: replaced a stale supervisor with no running session"
                );
                self.stale_supervisor_swapped.store(true, Ordering::SeqCst);
                *cached_client = Some(client);
                true
            }
            StaleSwapOutcome::Lost(message) => {
                eprintln!("terminal supervisor: failed to replace a stale supervisor: {message}");
                false
            }
        }
    }

    /// Returns the error of a supervisor that answered with another protocol
    /// at startup.
    #[cfg(unix)]
    pub(crate) fn supervisor_protocol_mismatch(&self) -> Option<String> {
        self.supervisor_protocol_mismatch
            .lock()
            .expect("terminal supervisor mismatch lock poisoned")
            .clone()
    }

    #[cfg(not(unix))]
    pub(crate) fn supervisor_protocol_mismatch(&self) -> Option<String> {
        None
    }

    fn supervisor_report(&self, status: TerminalSupervisorStatus) -> TerminalSupervisorReport {
        // A successful hello means that the old supervisor is gone.
        let protocol_mismatch = !status.available && self.supervisor_protocol_mismatch().is_some();
        TerminalSupervisorReport {
            status,
            protocol_mismatch,
        }
    }

    /// Counts the running terminals inside the app process. An app restart
    /// ends them. Supervised terminals keep running.
    pub(crate) fn in_process_session_count(&self) -> usize {
        self.sessions
            .lock()
            .expect("terminal sessions lock poisoned")
            .values()
            .filter(|session| {
                matches!(session.backend, Backend::InProcess { .. })
                    && session.snapshot().status == "running"
            })
            .count()
    }

    /// Counts the running terminals that the supervisor keeps. They survive
    /// an app restart.
    #[cfg(unix)]
    pub(crate) fn running_supervised_session_count(&self) -> usize {
        self.count_running_supervised(|_| true)
    }

    /// Counts the running supervised terminals that run an agent.
    #[cfg(unix)]
    pub(crate) fn running_supervised_agent_count(&self) -> usize {
        self.count_running_supervised(|session| session.agent_kind.is_some())
    }

    #[cfg(not(unix))]
    pub(crate) fn running_supervised_agent_count(&self) -> usize {
        0
    }

    #[cfg(unix)]
    fn count_running_supervised(&self, include: impl Fn(&TerminalSession) -> bool) -> usize {
        self.sessions
            .lock()
            .expect("terminal sessions lock poisoned")
            .values()
            .filter(|session| matches!(session.backend, Backend::Supervised))
            .map(|session| session.snapshot())
            .filter(|session| session.status == "running" && include(session))
            .count()
    }

    /// The sessions survive an app restart: the supervisor answers with
    /// this protocol, and no terminal runs inside the app.
    #[cfg(unix)]
    pub(crate) fn sessions_survive_restart(&self) -> bool {
        self.fallback_message().is_none()
            && self.supervisor_protocol_mismatch().is_none()
            && self.in_process_session_count() == 0
    }

    #[cfg(not(unix))]
    pub(crate) fn sessions_survive_restart(&self) -> bool {
        false
    }

    #[cfg(unix)]
    fn adopt_supervised_sessions_with_sink(
        &self,
        app_version: &str,
        sink: TerminalEventSink,
        supervisor_sink: SupervisorConnectionEventSink,
    ) -> TerminalResult<()> {
        let app_data_dir = self.supervisor_app_data_dir()?;
        let socket = crate::supervisor_link::socket_path(&app_data_dir);
        let mut client = match crate::supervisor_link::connect_existing(&app_data_dir, app_version)
        {
            Ok(Some(client)) => client,
            Ok(None) => return Ok(()),
            Err(error) => {
                // The old supervisor can still run agents. Remember it, so
                // nothing marks their rows exited or resumes them.
                *self
                    .supervisor_protocol_mismatch
                    .lock()
                    .expect("terminal supervisor mismatch lock poisoned") = Some(error);
                return Ok(());
            }
        };
        let summaries = client.list()?;
        for summary in summaries {
            let mut record = match serde_json::from_value::<TerminalSession>(summary.meta.clone()) {
                Ok(record) => record,
                Err(error) => {
                    eprintln!(
                        "failed to deserialize supervised terminal `{}` metadata: {error}",
                        summary.id
                    );
                    continue;
                }
            };
            apply_supervisor_summary(&mut record, &summary);
            match summary.status {
                SessionStatus::Running => {
                    let attach_socket = socket.clone();
                    let attach_version = app_version.to_string();
                    let ensure_app_data_dir = app_data_dir.clone();
                    let ensure_version = app_version.to_string();
                    let ensure_manager = self.clone();
                    self.adopt_running_supervised_session_with(
                        &summary,
                        record,
                        SupervisorAttachSinks {
                            terminal: sink.clone(),
                            connection: supervisor_sink.clone(),
                        },
                        SupervisorAttachHooks {
                            ensure: move || {
                                let ensured = crate::supervisor_link::ensure_running(
                                    &ensure_app_data_dir,
                                    &ensure_version,
                                )?;
                                *ensure_manager
                                    .supervisor
                                    .lock()
                                    .expect("terminal supervisor lock poisoned") = Some(ensured);
                                Ok(())
                            },
                            attach: move |id: &str, from_sequence: Option<u64>| {
                                SupervisorClient::attach_from_sequence(
                                    &attach_socket,
                                    &attach_version,
                                    id,
                                    from_sequence,
                                )
                            },
                            wait: thread::sleep,
                        },
                        |id| client.remove(id),
                    );
                }
                SessionStatus::Exited => {
                    match SupervisorClient::attach(&socket, app_version, &summary.id) {
                        Ok((snapshot, _events)) => {
                            self.persist_scrollback_snapshot_now(&summary.id, &snapshot.scrollback);
                            if let Err(error) = client.remove(&summary.id) {
                                eprintln!(
                                    "failed to remove adopted terminal `{}`: {error}",
                                    summary.id
                                );
                            }
                        }
                        Err(error) => {
                            eprintln!("failed to attach exited terminal `{}`: {error}", summary.id)
                        }
                    }
                }
            }
        }
        *self
            .supervisor
            .lock()
            .expect("terminal supervisor lock poisoned") = Some(client);
        Ok(())
    }

    #[cfg(unix)]
    fn adopt_running_supervised_session_with<Ensure, Attach, Events, Wait, Remove>(
        &self,
        summary: &SessionSummary,
        record: TerminalSession,
        sinks: SupervisorAttachSinks,
        hooks: SupervisorAttachHooks<Ensure, Attach, Wait>,
        mut remove: Remove,
    ) where
        Ensure: FnMut() -> TerminalResult<()> + Send + 'static,
        Attach: FnMut(&str, Option<u64>) -> TerminalResult<(halv_supervisor::Snapshot, Events)>
            + Send
            + 'static,
        Events: Iterator<Item = TerminalResult<SupervisorEvent>> + Send + 'static,
        Wait: FnMut(Duration) + Send + 'static,
        Remove: FnMut(&str) -> TerminalResult<()>,
    {
        let managed = Arc::new(ManagedTerminal {
            record: Mutex::new(record),
            output: Mutex::new(ScrollbackRing::new(MAX_SCROLLBACK_BYTES)),
            closing: AtomicBool::new(false),
            stop_requested: AtomicBool::new(false),
            // The prelude lives in memory only. An adopted shell has none.
            prelude: None,
            continuation: Mutex::new(None),
            backend: Backend::Supervised,
        });
        let attach_result = spawn_supervisor_attach_with(self.clone(), managed, sinks, true, hooks);
        if let Err(error) = attach_result {
            if unattachable_supervisor_session(&error) {
                if let Err(remove_error) = remove(&summary.id) {
                    eprintln!(
                        "failed to remove adopted terminal `{}`: {remove_error}",
                        summary.id
                    );
                }
            }
        }
    }

    // One background thread persists staged scrollback. Exit paths flush
    // directly, so a crash loses at most one flush interval of output.
    fn start_scrollback_flusher(&self) {
        if self.flusher_started.swap(true, Ordering::SeqCst) {
            return;
        }
        let dirty = self.scrollback_dirty.clone();
        let persistence = self.persistence.clone();
        thread::spawn(move || loop {
            thread::sleep(SCROLLBACK_FLUSH_INTERVAL);
            if !dirty.swap(false, Ordering::SeqCst) {
                continue;
            }
            if let Err(error) = persistence
                .lock()
                .expect("terminal persistence lock poisoned")
                .flush()
            {
                eprintln!("failed to flush terminal scrollback: {error}");
            }
        });
    }

    fn get(&self, id: &str) -> TerminalResult<Arc<ManagedTerminal>> {
        self.sessions
            .lock()
            .expect("terminal sessions lock poisoned")
            .get(id)
            .cloned()
            .ok_or_else(|| format!("terminal session `{id}` was not found"))
    }

    pub(crate) fn list(&self) -> Vec<TerminalSession> {
        let sessions = self
            .sessions
            .lock()
            .expect("terminal sessions lock poisoned");
        let mut rows = sessions
            .values()
            .map(|session| session.snapshot())
            .collect::<Vec<_>>();
        rows.sort_by(|left, right| right.started_at.cmp(&left.started_at));
        rows
    }

    pub(crate) fn session(&self, id: &str) -> TerminalResult<TerminalSession> {
        Ok(self.get(id)?.snapshot())
    }

    /// True when Halv ended the session or ends it soon: a pane close, a
    /// bulk kill, or the app quit. Such an exit never continues in a shell.
    pub(crate) fn halv_stopped(&self, id: &str) -> bool {
        self.stopping.load(Ordering::SeqCst)
            || self.get(id).is_ok_and(|session| {
                session.closing.load(Ordering::SeqCst)
                    || session.stop_requested.load(Ordering::SeqCst)
            })
    }

    // A bulk kill ends every running supervised session. The supervisor can
    // end a CLI with SIGHUP, and some CLIs then exit with code 0, so each
    // session gets a mark before the kill. The result holds the marked
    // sessions, so that a failed kill can clear the marks again.
    #[cfg(unix)]
    fn mark_supervised_sessions_stopped(&self) -> Vec<Arc<ManagedTerminal>> {
        let sessions = self
            .sessions
            .lock()
            .expect("terminal sessions lock poisoned");
        let mut marked = Vec::new();
        for session in sessions.values() {
            if matches!(&session.backend, Backend::Supervised)
                && session.snapshot().status == "running"
            {
                session.stop_requested.store(true, Ordering::SeqCst);
                marked.push(session.clone());
            }
        }
        marked
    }

    // A failed bulk kill can stop part way, so some sessions keep running.
    // Each of them keeps the normal exit rules: a later clean exit continues
    // in a shell. A session that the supervisor does not list as running
    // keeps its mark. Without a supervisor list, all marks stay.
    #[cfg(unix)]
    fn release_surviving_stop_marks(
        &self,
        marked: &[Arc<ManagedTerminal>],
        client: Option<&mut SupervisorClient>,
    ) {
        if marked.is_empty() {
            return;
        }
        let listed = match client.map(SupervisorClient::list) {
            Some(Ok(sessions)) => Some(sessions),
            _ => self
                .supervisor_app_data_dir()
                .ok()
                .and_then(|app_data_dir| {
                    crate::supervisor_link::connect_existing(
                        &app_data_dir,
                        env!("CARGO_PKG_VERSION"),
                    )
                    .ok()
                    .flatten()
                    .and_then(|mut client| client.list().ok())
                }),
        };
        if let Some(sessions) = listed {
            clear_stop_marks_of_running_sessions(marked, &sessions);
        }
    }

    /// Starts a plain shell that continues an ended agent session in the
    /// same pane. Each source gets at most one shell: a second call returns
    /// the first shell. The returned flag is true for a new shell.
    fn continue_with_shell_with(
        &self,
        source_id: &str,
        spawn: impl FnOnce(
            TerminalSpawnRequest,
            TerminalContinuation,
        ) -> TerminalResult<TerminalSession>,
    ) -> TerminalResult<(TerminalSession, bool)> {
        if self.stopping.load(Ordering::SeqCst) {
            return Err(TERMINALS_STOPPING_ERROR.to_string());
        }
        let source = self.get(source_id)?;
        // The lock stays held during the spawn. The lifecycle thread and an
        // "Open shell here" click then cannot both start a shell.
        let mut continuation = source
            .continuation
            .lock()
            .expect("terminal continuation lock poisoned");
        if let Some(existing) = continuation.as_deref() {
            if let Ok(session) = self.session(existing) {
                return Ok((session, false));
            }
        }
        let record = source.snapshot();
        if record.status == "running" {
            return Err("the terminal is still running".to_string());
        }
        if record.agent_kind.is_none() {
            return Err("only agent terminals continue in a shell".to_string());
        }
        let prelude = TerminalPrelude {
            scrollback: source.attach().scrollback,
            agent_kind: record.agent_kind.clone(),
            exit_code: record.exit_code,
        };
        let request =
            plain_shell_request(ShellSource::from_session(&record), record.cols, record.rows);
        let session = spawn(
            request,
            TerminalContinuation {
                continued_from: source_id.to_string(),
                prelude,
            },
        )?;
        *continuation = Some(session.id.clone());
        Ok((session, true))
    }

    /// Returns the full ring of a session for the remote gateway.
    pub(crate) fn remote_ring_snapshot(&self, id: &str) -> Option<RemoteRing> {
        let session = self.get(id).ok()?;
        let record = session.snapshot();
        let (sequence, data) = session
            .output
            .lock()
            .expect("terminal output lock poisoned")
            .snapshot(None);
        Some(RemoteRing {
            sequence,
            cols: record.cols,
            rows: record.rows,
            data,
        })
    }

    /// Returns the ring output after `after` for the remote gateway.
    /// Returns `None` when the ring no longer holds all of that output.
    pub(crate) fn remote_ring_since(&self, id: &str, after: u64) -> Option<RemoteRing> {
        let session = self.get(id).ok()?;
        let record = session.snapshot();
        let (sequence, data) = ring_since(
            &session
                .output
                .lock()
                .expect("terminal output lock poisoned"),
            after,
        )?;
        Some(RemoteRing {
            sequence,
            cols: record.cols,
            rows: record.rows,
            data,
        })
    }

    /// Writes input from the remote gateway through the same backend as `terminal_write`.
    pub(crate) fn remote_write(&self, id: &str, data: &[u8]) -> TerminalResult<()> {
        if data.len() > MAX_TERMINAL_WRITE_BYTES {
            return Err(format!(
                "terminal input exceeds the {MAX_TERMINAL_WRITE_BYTES}-byte limit"
            ));
        }
        let data = String::from_utf8(data.to_vec())
            .map_err(|_| "terminal input is not valid UTF-8".to_string())?;
        // The session queue keeps phone input in order with the keyboard input of the pane.
        // The gateway calls this function in `spawn_blocking`, so a blocking wait is safe.
        let written = self.queue_input(id, data, None, env!("CARGO_PKG_VERSION"))?;
        written
            .blocking_recv()
            .map_err(|_| "the terminal input writer stopped".to_string())?
    }

    pub(crate) fn close_session(&self, id: &str) -> TerminalResult<TerminalSession> {
        let session = self.get(id)?;
        session.closing.store(true, Ordering::SeqCst);
        match &session.backend {
            Backend::InProcess { killer, .. } => {
                if session.snapshot().status == "running" {
                    if let Some(killer) = killer
                        .lock()
                        .expect("terminal killer lock poisoned")
                        .as_mut()
                    {
                        if let Err(error) = killer.kill() {
                            session.closing.store(false, Ordering::SeqCst);
                            return Err(format!("failed to close terminal: {error}"));
                        }
                    }
                }
            }
            #[cfg(unix)]
            Backend::Supervised => {
                // The close waits up to 4 s for the process to exit. It uses
                // its own connection, so the wait never holds the shared one.
                let result = self.with_own_supervisor_connection(|client| {
                    let summary = client.close(id)?;
                    if let Err(error) = client.remove(id) {
                        if !supervisor_not_found(&error) {
                            return Err(error);
                        }
                    }
                    Ok(summary)
                });
                let summary = match result {
                    Ok(summary) => summary,
                    Err(error) => {
                        session.closing.store(false, Ordering::SeqCst);
                        return Err(error);
                    }
                };
                apply_supervisor_summary(
                    &mut session
                        .record
                        .lock()
                        .expect("terminal record lock poisoned"),
                    &summary,
                );
            }
        }
        let snapshot = session.snapshot();
        self.stop_input(id);
        self.remove_persisted_session(id)?;
        Ok(snapshot)
    }

    /// Queues input for a session. The receiver gets the result of the write.
    ///
    /// The writes of one `order.writer` reach the PTY in sequence order, and a
    /// repeated sequence reaches it once. Writes without an order go out in
    /// arrival order.
    pub(crate) fn queue_input(
        &self,
        session_id: &str,
        data: String,
        order: Option<InputOrder>,
        app_version: &str,
    ) -> TerminalResult<oneshot::Receiver<TerminalResult<()>>> {
        let session = self.get(session_id)?;
        if session.snapshot().status != "running" {
            return Err("terminal session is not running".to_string());
        }
        self.queue_input_with(session_id, data, order, || {
            self.input_sink(session_id, &session, app_version)
        })
    }

    fn queue_input_with(
        &self,
        session_id: &str,
        data: String,
        order: Option<InputOrder>,
        sink: impl FnOnce() -> TerminalResult<InputSink>,
    ) -> TerminalResult<oneshot::Receiver<TerminalResult<()>>> {
        let (done, written) = oneshot::channel();
        let mut request = InputRequest { data, order, done };
        let mut inputs = self.inputs.lock().expect("terminal input lock poisoned");
        if let Some(queue) = inputs.get(session_id) {
            match queue.push(request) {
                Ok(()) => return Ok(written),
                // The writer thread stopped. A new queue replaces it.
                Err(returned) => request = returned,
            }
        }
        let queue = InputQueue::start(sink()?)?;
        queue
            .push(request)
            .map_err(|_| "the terminal input writer stopped".to_string())?;
        inputs.insert(session_id.to_string(), queue);
        Ok(written)
    }

    fn input_sink(
        &self,
        session_id: &str,
        session: &Arc<ManagedTerminal>,
        app_version: &str,
    ) -> TerminalResult<InputSink> {
        #[cfg(not(unix))]
        let _ = (session_id, app_version);
        let terminal = session.clone();
        match &session.backend {
            Backend::InProcess { .. } => Ok(Box::new(move |data: &str| {
                write_in_process_input(&terminal, data)
            })),
            #[cfg(unix)]
            Backend::Supervised => {
                let socket = crate::supervisor_link::socket_path(&self.supervisor_app_data_dir()?);
                let app_version = app_version.to_string();
                Ok(supervised_input_sink(
                    terminal,
                    session_id.to_string(),
                    move || SupervisorClient::connect(&socket, &app_version),
                ))
            }
        }
    }

    /// Stops the input queue of a session. The writes that wait still finish.
    fn stop_input(&self, id: &str) {
        self.inputs
            .lock()
            .expect("terminal input lock poisoned")
            .remove(id);
    }

    /// Runs `operation` on a new supervisor connection, so a slow operation
    /// never holds the shared connection. Without an answer from the
    /// supervisor, the shared connection starts one, as before.
    #[cfg(unix)]
    fn with_own_supervisor_connection<T>(
        &self,
        operation: impl FnOnce(&mut SupervisorClient) -> TerminalResult<T>,
    ) -> TerminalResult<T> {
        let socket = crate::supervisor_link::socket_path(&self.supervisor_app_data_dir()?);
        match SupervisorClient::connect(&socket, env!("CARGO_PKG_VERSION")) {
            Ok(mut client) => operation(&mut client),
            Err(_) => self.with_supervisor(env!("CARGO_PKG_VERSION"), operation),
        }
    }

    fn forget_session(&self, id: &str) -> TerminalResult<()> {
        let session = self.get(id)?;
        #[cfg(unix)]
        if matches!(&session.backend, Backend::Supervised) {
            return self.forget_supervised_session_with(id, |session_id| {
                self.with_supervisor(env!("CARGO_PKG_VERSION"), |client| {
                    client.remove(session_id)
                })
            });
        }
        if session.snapshot().status == "running" {
            return Err(TERMINAL_FORGET_RUNNING_ERROR.to_string());
        }
        self.remove_forgotten_session(id)
    }

    #[cfg(unix)]
    fn forget_supervised_session_with(
        &self,
        id: &str,
        mut remove: impl FnMut(&str) -> TerminalResult<()>,
    ) -> TerminalResult<()> {
        self.get(id)?;
        match remove(id) {
            Ok(()) => {}
            Err(error) if supervisor_not_found(&error) => {}
            Err(error) if supervisor_still_running(&error) => {
                return Err(TERMINAL_FORGET_RUNNING_ERROR.to_string());
            }
            Err(error) => return Err(error),
        }
        self.remove_forgotten_session(id)
    }

    fn remove_forgotten_session(&self, id: &str) -> TerminalResult<()> {
        self.sessions
            .lock()
            .expect("terminal sessions lock poisoned")
            .remove(id);
        self.stop_input(id);
        notify_terminal_tap(TerminalTapEvent::Removed { session_id: id });
        self.remove_persisted_session(id)
    }

    fn persist_scrollback(&self, session: &ManagedTerminal) {
        let attached = session.attach();
        let changed = self
            .persistence
            .lock()
            .expect("terminal persistence lock poisoned")
            .stage_scrollback(&attached.session.id, &attached.scrollback);
        if changed {
            self.scrollback_dirty.store(true, Ordering::SeqCst);
        }
    }

    fn persist_scrollback_now(&self, session: &ManagedTerminal) {
        let attached = session.attach();
        let mut persistence = self
            .persistence
            .lock()
            .expect("terminal persistence lock poisoned");
        if persistence.stage_scrollback(&attached.session.id, &attached.scrollback) {
            if let Err(error) = persistence.flush() {
                eprintln!("failed to persist terminal scrollback: {error}");
            }
        }
    }

    fn persist_scrollback_snapshot_now(&self, session_id: &str, scrollback: &str) {
        let mut persistence = self
            .persistence
            .lock()
            .expect("terminal persistence lock poisoned");
        if persistence.stage_scrollback(session_id, scrollback) {
            if let Err(error) = persistence.flush() {
                eprintln!("failed to persist terminal scrollback: {error}");
            }
        }
    }

    fn remove_persisted_session(&self, session_id: &str) -> TerminalResult<()> {
        self.persistence
            .lock()
            .expect("terminal persistence lock poisoned")
            .remove_session(session_id)
            .map_err(|error| format!("failed to remove saved terminal: {error}"))
    }

    #[cfg(unix)]
    pub fn stop_all_supervised(&self) -> TerminalResult<()> {
        // This is the quit path ("Quit and Stop Agents"). A session that the
        // kill ends must not continue in a shell.
        self.stopping.store(true, Ordering::SeqCst);
        let result = self.kill_supervised_sessions();
        // The tray keeps the app open when the stop fails. Later exits can
        // then continue in a shell again. The session marks still cover each
        // session that the failed kill ended.
        if result.is_err() {
            self.stopping.store(false, Ordering::SeqCst);
        }
        result
    }

    #[cfg(unix)]
    fn kill_supervised_sessions(&self) -> TerminalResult<()> {
        let app_data_dir = self.supervisor_app_data_dir()?;
        let mut client = self
            .supervisor
            .lock()
            .expect("terminal supervisor lock poisoned");
        if client.is_none() {
            *client =
                crate::supervisor_link::connect_existing(&app_data_dir, env!("CARGO_PKG_VERSION"))
                    .ok()
                    .flatten();
        }
        let Some(supervisor) = client.as_mut() else {
            return Ok(());
        };
        let marked = self.mark_supervised_sessions_stopped();
        let result = supervisor.shutdown(true);
        if result.is_err() {
            self.release_surviving_stop_marks(&marked, Some(supervisor));
        }
        if result.is_ok()
            || result
                .as_ref()
                .err()
                .is_some_and(|error| crate::supervisor_link::is_broken_connection(error))
        {
            client.take();
        }
        result
    }

    #[cfg(not(unix))]
    pub fn stop_all_supervised(&self) -> TerminalResult<()> {
        Ok(())
    }

    #[cfg(unix)]
    fn supervisor_status(&self) -> TerminalSupervisorStatus {
        let app_data_dir = match self.supervisor_app_data_dir() {
            Ok(app_data_dir) => app_data_dir,
            Err(message) => {
                return TerminalSupervisorStatus {
                    available: false,
                    socket_path: String::new(),
                    pid: None,
                    sessions: 0,
                    running_sessions: 0,
                    supervisor_version: None,
                    supervisor_stale: false,
                    app_version: env!("CARGO_PKG_VERSION").to_string(),
                    message: Some(message),
                };
            }
        };
        let socket = crate::supervisor_link::socket_path(&app_data_dir);
        let socket_path = socket.to_string_lossy().into_owned();
        if let Some(message) = self.fallback_message() {
            return TerminalSupervisorStatus {
                available: false,
                socket_path,
                pid: None,
                sessions: 0,
                running_sessions: 0,
                supervisor_version: None,
                supervisor_stale: false,
                app_version: env!("CARGO_PKG_VERSION").to_string(),
                message: Some(message),
            };
        }
        let mut client = self
            .supervisor
            .lock()
            .expect("terminal supervisor lock poisoned");
        if let Some(existing) = client.as_mut() {
            let pid = existing.supervisor_pid();
            let supervisor_version = existing.supervisor_version().to_string();
            let supervisor_stale = is_supervisor_stale(
                existing.supervisor_fingerprint(),
                &supervisor_version,
                env!("CARGO_PKG_VERSION"),
            );
            match existing.list() {
                Ok(sessions) => {
                    let running_sessions = sessions
                        .iter()
                        .filter(|session| session.status == SessionStatus::Running)
                        .count();
                    self.clear_fallback();
                    let message = supervisor_version_mismatch_message(
                        supervisor_stale,
                        &supervisor_version,
                        env!("CARGO_PKG_VERSION"),
                    );
                    return TerminalSupervisorStatus {
                        available: true,
                        socket_path,
                        pid: Some(pid),
                        sessions: sessions.len(),
                        running_sessions,
                        supervisor_version: Some(supervisor_version),
                        supervisor_stale,
                        app_version: env!("CARGO_PKG_VERSION").to_string(),
                        message,
                    };
                }
                Err(error) => {
                    eprintln!("failed to inspect terminal supervisor: {error}");
                    client.take();
                }
            }
        }

        let result =
            crate::supervisor_link::ensure_running(&app_data_dir, env!("CARGO_PKG_VERSION"))
                .and_then(|mut started| {
                    let pid = started.supervisor_pid();
                    let supervisor_version = started.supervisor_version().to_string();
                    let supervisor_stale = is_supervisor_stale(
                        started.supervisor_fingerprint(),
                        &supervisor_version,
                        env!("CARGO_PKG_VERSION"),
                    );
                    let sessions = started.list()?;
                    let running_sessions = sessions
                        .iter()
                        .filter(|session| session.status == SessionStatus::Running)
                        .count();
                    let sessions = sessions.len();
                    *client = Some(started);
                    Ok((
                        pid,
                        sessions,
                        running_sessions,
                        supervisor_version,
                        supervisor_stale,
                    ))
                });
        match result {
            Ok((pid, sessions, running_sessions, supervisor_version, supervisor_stale)) => {
                self.clear_fallback();
                let message = supervisor_version_mismatch_message(
                    supervisor_stale,
                    &supervisor_version,
                    env!("CARGO_PKG_VERSION"),
                );
                TerminalSupervisorStatus {
                    available: true,
                    socket_path,
                    pid: Some(pid),
                    sessions,
                    running_sessions,
                    supervisor_version: Some(supervisor_version),
                    supervisor_stale,
                    app_version: env!("CARGO_PKG_VERSION").to_string(),
                    message,
                }
            }
            Err(error) => {
                if crate::supervisor_link::is_broken_connection(&error) {
                    client.take();
                }
                eprintln!("failed to inspect terminal supervisor: {error}");
                TerminalSupervisorStatus {
                    available: false,
                    socket_path,
                    pid: None,
                    sessions: 0,
                    running_sessions: 0,
                    supervisor_version: None,
                    supervisor_stale: false,
                    app_version: env!("CARGO_PKG_VERSION").to_string(),
                    message: Some(error),
                }
            }
        }
    }

    #[cfg(not(unix))]
    fn supervisor_status(&self) -> TerminalSupervisorStatus {
        TerminalSupervisorStatus {
            available: false,
            socket_path: String::new(),
            pid: None,
            sessions: 0,
            running_sessions: 0,
            supervisor_version: None,
            supervisor_stale: false,
            app_version: env!("CARGO_PKG_VERSION").to_string(),
            message: None,
        }
    }

    pub fn shutdown(&self) {
        // Set first: a late continuation shell must not call ensure_running
        // and start a supervisor during the quit.
        self.stopping.store(true, Ordering::SeqCst);
        let sessions = self
            .sessions
            .lock()
            .expect("terminal sessions lock poisoned")
            .values()
            .cloned()
            .collect::<Vec<_>>();
        for session in sessions {
            self.persist_scrollback(&session);
            if let Backend::InProcess { killer, .. } = &session.backend {
                session.closing.store(true, Ordering::SeqCst);
                if let Some(killer) = killer
                    .lock()
                    .expect("terminal killer lock poisoned")
                    .as_mut()
                {
                    let _ = killer.kill();
                }
            }
        }
        if let Err(error) = self
            .persistence
            .lock()
            .expect("terminal persistence lock poisoned")
            .flush()
        {
            eprintln!("failed to flush terminal state: {error}");
        }
        #[cfg(unix)]
        self.supervisor
            .lock()
            .expect("terminal supervisor lock poisoned")
            .take();
    }
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub fn terminal_open(
    app: AppHandle,
    manager: tauri::State<'_, TerminalManager>,
    storage: tauri::State<'_, Storage>,
    project_id: i64,
    workspace_id: Option<String>,
    conversation_id: Option<String>,
    shell_tab_id: Option<String>,
    cols: u16,
    rows: u16,
    execution_profile: Option<String>,
) -> TerminalResult<TerminalSession> {
    let execution_profile = execution_profile.as_deref().unwrap_or("full_access").trim();
    if execution_profile != "full_access" {
        return Err(
            "interactive terminals currently support only explicit full_access profile".to_string(),
        );
    }
    let shell_tab_id = shell_tab_id.filter(|id| !id.trim().is_empty());
    check_terminal_owner(
        conversation_id.as_deref(),
        shell_tab_id.as_deref(),
        workspace_id.as_deref(),
        |key| {
            storage
                .workspace_tab_shell_workspace(key)
                .map_err(|error| error.to_string())
        },
    )?;

    let project = storage
        .get_project(project_id)
        .map_err(|error| error.to_string())?;
    let cwd = if let Some(workspace_id) = workspace_id.as_deref() {
        let workspace = storage
            .get_workspace(workspace_id)
            .map_err(|error| error.to_string())?;
        if workspace.project_id != project_id || workspace.state != "available" {
            return Err("workspace is unavailable or belongs to another project".to_string());
        }
        projects::validate_registered_project_root(Path::new(&workspace.root_path))?
    } else {
        projects::validate_registered_project(&project)?
    };

    spawn_terminal(
        app,
        manager.inner(),
        TerminalSpawnRequest {
            id: None,
            project_id: Some(project_id),
            conversation_id,
            project_name: project.name,
            cwd,
            cols,
            rows,
            execution_profile: execution_profile.to_string(),
            agent_kind: None,
            program: None,
            shell_tab_id,
        },
    )
}

/// A terminal has one owner at most: a conversation or a shell tab. A shell
/// tab must exist, and it must belong to the requested workspace.
/// `shell_tab_workspace` gives the workspace of a shell tab key, or None
/// when no such tab exists.
fn check_terminal_owner(
    conversation_id: Option<&str>,
    shell_tab_id: Option<&str>,
    workspace_id: Option<&str>,
    shell_tab_workspace: impl FnOnce(&str) -> TerminalResult<Option<String>>,
) -> TerminalResult<()> {
    let Some(shell_tab_id) = shell_tab_id else {
        return Ok(());
    };
    if conversation_id.is_some_and(|id| !id.trim().is_empty()) {
        return Err("a terminal belongs to a conversation or to a shell tab, not both".to_string());
    }
    let tab_workspace = shell_tab_workspace(&format!("shell:{shell_tab_id}"))?
        .ok_or_else(|| format!("shell tab `{shell_tab_id}` was not found"))?;
    if workspace_id.is_some_and(|id| id != tab_workspace) {
        return Err(format!(
            "shell tab `{shell_tab_id}` belongs to another workspace"
        ));
    }
    Ok(())
}

/// The shell tab of a panel scope. The frontend sends the panel storage key,
/// for example `halv.terminal-layout.v1:shell:<uuid>`, so the prefix is
/// optional.
fn shell_tab_id_from_scope(scope: &str) -> Option<String> {
    let scope = scope
        .strip_prefix(TERMINAL_PANEL_SCOPE_PREFIX)
        .unwrap_or(scope);
    scope
        .strip_prefix("shell:")
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(ToString::to_string)
}

const TERMINAL_PANEL_SCOPE_PREFIX: &str = "halv.terminal-layout.v1:";

#[tauri::command]
pub fn terminal_duplicate(
    app: AppHandle,
    manager: tauri::State<'_, TerminalManager>,
    session_id: String,
    cols: u16,
    rows: u16,
) -> TerminalResult<TerminalSession> {
    let source = manager.session(&session_id)?;
    if source.agent_kind.is_some() {
        return Err("agent terminals must be duplicated through Agent Sessions".to_string());
    }
    spawn_terminal(
        app,
        manager.inner(),
        TerminalSpawnRequest {
            id: None,
            project_id: source.project_id,
            conversation_id: source.conversation_id,
            project_name: source.project_name,
            cwd: PathBuf::from(source.cwd),
            cols,
            rows,
            execution_profile: source.execution_profile,
            agent_kind: None,
            program: None,
            shell_tab_id: source.shell_tab_id,
        },
    )
}

/// The folder and the bindings of the pane that a new plain shell starts
/// next to.
#[derive(Debug, Clone, PartialEq)]
struct ShellSource {
    project_id: Option<i64>,
    conversation_id: Option<String>,
    project_name: String,
    cwd: PathBuf,
    shell_tab_id: Option<String>,
}

impl ShellSource {
    fn from_session(session: &TerminalSession) -> Self {
        Self {
            project_id: session.project_id,
            conversation_id: session.conversation_id.clone(),
            project_name: session.project_name.clone(),
            cwd: PathBuf::from(&session.cwd),
            shell_tab_id: session.shell_tab_id.clone(),
        }
    }

    /// A saved pane has no owner field. The scope of a shell tab panel
    /// names the owner. A conversation pane keeps its conversation.
    fn from_saved_pane(pane: &crate::terminal_state::TerminalPaneState, scope: &str) -> Self {
        Self {
            project_id: pane.project_id,
            conversation_id: pane.conversation_id.clone(),
            project_name: pane.project_name.clone(),
            cwd: PathBuf::from(&pane.cwd),
            shell_tab_id: if pane.conversation_id.is_none() {
                shell_tab_id_from_scope(scope)
            } else {
                None
            },
        }
    }
}

/// A plain login shell next to a source pane. `program: None` selects the
/// login shell and the plain-shell environment. No agent key, route, or
/// ledger variable of the source crosses into the shell.
fn plain_shell_request(source: ShellSource, cols: u16, rows: u16) -> TerminalSpawnRequest {
    TerminalSpawnRequest {
        id: None,
        project_id: source.project_id,
        conversation_id: source.conversation_id,
        project_name: source.project_name,
        cwd: source.cwd,
        cols,
        rows,
        execution_profile: "full_access".to_string(),
        agent_kind: None,
        program: None,
        shell_tab_id: source.shell_tab_id,
    }
}

// A live session is the source. A restored pane has no live session, so
// its saved state in `scope` names the folder.
fn shell_source_for(
    manager: &TerminalManager,
    session_id: &str,
    scope: Option<&str>,
) -> TerminalResult<ShellSource> {
    match manager.session(session_id) {
        Ok(session) => Ok(ShellSource::from_session(&session)),
        Err(error) => scope
            .and_then(|scope| {
                manager
                    .persistence
                    .lock()
                    .expect("terminal persistence lock poisoned")
                    .pane(scope, session_id)
                    .map(|pane| ShellSource::from_saved_pane(&pane, scope))
            })
            .ok_or(error),
    }
}

/// True when an agent CLI ended on its own with success, for example after
/// `/exit`. A Halv stop, a signal, or a failure keeps the exit card.
pub(crate) fn clean_self_exit(terminal: &TerminalSession, halv_stopped: bool) -> bool {
    !halv_stopped
        && terminal.status == "exited"
        && terminal.exit_code == Some(0)
        && terminal.signal.is_none()
}

/// Opens a plain shell in the pane of an ended agent session. The webview
/// gets `terminal_continued` and binds the pane to the new shell.
pub(crate) fn continue_with_shell(
    app: &AppHandle,
    manager: &TerminalManager,
    source_id: &str,
) -> TerminalResult<TerminalSession> {
    let (session, created) =
        manager.continue_with_shell_with(source_id, |request, continuation| {
            spawn_terminal_with(app.clone(), manager, request, Some(continuation))
        })?;
    if created {
        if let Err(error) = app.emit(
            "terminal_continued",
            TerminalContinuedEvent {
                from_session_id: source_id.to_string(),
                session: session.clone(),
            },
        ) {
            eprintln!("failed to emit the terminal continuation: {error}");
        }
    }
    Ok(session)
}

pub(crate) fn spawn_terminal(
    app: AppHandle,
    manager: &TerminalManager,
    request: TerminalSpawnRequest,
) -> TerminalResult<TerminalSession> {
    spawn_terminal_with(app, manager, request, None)
}

fn spawn_terminal_with(
    app: AppHandle,
    manager: &TerminalManager,
    request: TerminalSpawnRequest,
    continuation: Option<TerminalContinuation>,
) -> TerminalResult<TerminalSession> {
    if request.execution_profile != "full_access" {
        return Err(
            "interactive terminals currently support only explicit full_access profile".to_string(),
        );
    }
    if !request.cwd.is_dir() {
        return Err(format!(
            "terminal cwd `{}` is not a directory",
            request.cwd.display()
        ));
    }
    let active_count = manager
        .sessions
        .lock()
        .expect("terminal sessions lock poisoned")
        .values()
        .filter(|session| session.snapshot().status == "running")
        .count();
    if active_count >= MAX_TERMINAL_SESSIONS {
        return Err(format!(
            "at most {MAX_TERMINAL_SESSIONS} terminal sessions can run at once"
        ));
    }

    let size = terminal_size(request.cols, request.rows);
    let cwd = request.cwd;
    let id = request
        .id
        .unwrap_or_else(|| format!("terminal-{:032x}", rand::random::<u128>()));
    let command = build_terminal_command(&cwd, request.program);
    let (continued_from, prelude) = match continuation {
        Some(continuation) => (
            Some(continuation.continued_from),
            Some(continuation.prelude),
        ),
        None => (None, None),
    };
    let session = TerminalSession {
        id,
        title: None,
        project_id: request.project_id,
        conversation_id: request.conversation_id,
        project_name: request.project_name,
        cwd: cwd.to_string_lossy().into_owned(),
        shell: command.program.to_string_lossy().into_owned(),
        pid: 0,
        execution_profile: request.execution_profile,
        agent_kind: request.agent_kind,
        status: "running".to_string(),
        cols: size.cols,
        rows: size.rows,
        started_at: Utc::now().to_rfc3339(),
        exited_at: None,
        exit_code: None,
        signal: None,
        continued_from,
        shell_tab_id: request.shell_tab_id,
    };
    let sink = terminal_event_sink(&app);
    #[cfg(unix)]
    let supervisor_sink = supervisor_connection_event_sink(&app);

    #[cfg(unix)]
    {
        let supervised = if let Some(message) = manager.fallback_message() {
            Err(message)
        } else {
            spawn_supervised_terminal(
                manager,
                session.clone(),
                &cwd,
                command.clone(),
                app.package_info().version.to_string(),
                sink.clone(),
                supervisor_sink,
                prelude.clone(),
            )
        };
        match supervised {
            Ok(session) => {
                manager.clear_fallback();
                return Ok(session);
            }
            // A quit refuses new terminals. This is not a supervisor fault,
            // so no in-process terminal starts.
            Err(error) if error == TERMINALS_STOPPING_ERROR => return Err(error),
            Err(error) => {
                eprintln!("terminal supervisor fallback: {error}");
                if manager.record_fallback(error.clone()) {
                    let _ = app.emit(
                        "terminal_supervisor_fallback",
                        TerminalSupervisorFallbackEvent { message: error },
                    );
                }
            }
        }
    }

    spawn_in_process_terminal(manager, session, cwd, command, sink, prelude)
}

fn spawn_in_process_terminal(
    manager: &TerminalManager,
    mut session: TerminalSession,
    cwd: PathBuf,
    command: ResolvedCommand,
    sink: TerminalEventSink,
    prelude: Option<TerminalPrelude>,
) -> TerminalResult<TerminalSession> {
    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(terminal_size(session.cols, session.rows))
        .map_err(|error| format!("failed to open terminal: {error}"))?;
    let command = portable_pty_command(&command, &cwd);
    let mut child = pair
        .slave
        .spawn_command(command)
        .map_err(|error| format!("failed to start terminal program: {error}"))?;
    let pid = child.process_id().unwrap_or(0);
    let killer = child.clone_killer();
    let reader = pair
        .master
        .try_clone_reader()
        .map_err(|error| format!("failed to attach terminal output: {error}"))?;
    let writer = pair
        .master
        .take_writer()
        .map_err(|error| format!("failed to attach terminal input: {error}"))?;
    drop(pair.slave);

    session.pid = pid;
    let managed = Arc::new(ManagedTerminal {
        record: Mutex::new(session.clone()),
        output: Mutex::new(ScrollbackRing::new(MAX_SCROLLBACK_BYTES)),
        closing: AtomicBool::new(false),
        stop_requested: AtomicBool::new(false),
        prelude,
        continuation: Mutex::new(None),
        backend: Backend::InProcess {
            master: Mutex::new(pair.master),
            writer: Mutex::new(Some(writer)),
            killer: Mutex::new(Some(killer)),
        },
    });
    manager
        .sessions
        .lock()
        .expect("terminal sessions lock poisoned")
        .insert(session.id.clone(), managed.clone());

    spawn_output_reader(sink.clone(), manager.clone(), managed.clone(), reader);
    let wait_manager = manager.clone();
    thread::spawn(move || match child.wait() {
        Ok(exit) => managed.finish(
            &sink,
            &wait_manager,
            TerminalFinish {
                sequence: None,
                status: "exited",
                exit_code: Some(exit.exit_code().min(i32::MAX as u32) as i32),
                signal: exit.signal().map(ToString::to_string),
                message: None,
            },
        ),
        Err(error) => managed.finish(
            &sink,
            &wait_manager,
            TerminalFinish {
                sequence: None,
                status: "failed",
                exit_code: None,
                signal: None,
                message: Some(format!("failed to wait for terminal program: {error}")),
            },
        ),
    });

    Ok(session)
}

#[cfg(unix)]
#[allow(clippy::too_many_arguments)]
fn spawn_supervised_terminal(
    manager: &TerminalManager,
    mut session: TerminalSession,
    cwd: &Path,
    command: ResolvedCommand,
    app_version: String,
    sink: TerminalEventSink,
    supervisor_sink: SupervisorConnectionEventSink,
    prelude: Option<TerminalPrelude>,
) -> TerminalResult<TerminalSession> {
    let meta = serde_json::to_value(&session)
        .map_err(|error| format!("failed to encode terminal metadata: {error}"))?;
    let open_request = OpenRequest {
        id: session.id.clone(),
        program: protocol_string(command.program.as_os_str()),
        args: command
            .args
            .iter()
            .map(|argument| protocol_string(argument))
            .collect(),
        cwd: protocol_string(cwd.as_os_str()),
        env: command
            .env
            .iter()
            .map(|(name, value)| [protocol_string(name), protocol_string(value)])
            .collect(),
        env_remove: command
            .env_remove
            .iter()
            .map(|name| protocol_string(name))
            .collect(),
        cols: session.cols,
        rows: session.rows,
        meta,
    };
    let summary = manager.with_supervisor(&app_version, |client| client.open(open_request))?;
    apply_supervisor_summary(&mut session, &summary);
    let managed = Arc::new(ManagedTerminal {
        record: Mutex::new(session.clone()),
        output: Mutex::new(ScrollbackRing::new(MAX_SCROLLBACK_BYTES)),
        closing: AtomicBool::new(false),
        stop_requested: AtomicBool::new(false),
        prelude,
        continuation: Mutex::new(None),
        backend: Backend::Supervised,
    });
    manager
        .sessions
        .lock()
        .expect("terminal sessions lock poisoned")
        .insert(session.id.clone(), managed.clone());
    let app_data_dir = manager.supervisor_app_data_dir()?;
    let _ = spawn_supervisor_attach(
        app_data_dir,
        app_version,
        manager.clone(),
        managed.clone(),
        sink,
        supervisor_sink,
        false,
    );
    Ok(managed.snapshot())
}

#[cfg(unix)]
fn protocol_string(value: &std::ffi::OsStr) -> String {
    value.to_string_lossy().into_owned()
}

fn build_terminal_command(_cwd: &Path, program: Option<TerminalProgram>) -> ResolvedCommand {
    let mut command = if let Some(program) = program {
        ResolvedCommand {
            program: program.program,
            args: program.args,
            env: program.env,
            env_remove: program.env_remove,
        }
    } else {
        let shell = terminal_shell();
        let mut args = Vec::new();
        add_login_argument(&mut args, &shell);
        // A CLI typed in a new shell uses the active wallet accounts.
        let account = crate::wallet::shell_environment();
        ResolvedCommand {
            program: shell,
            args,
            env: account.set,
            // A shell is not an agent pane, even in a Halv that runs in one.
            env_remove: crate::agent_status::pane_variable_names().to_vec(),
        }
    };
    // The app can inherit these markers from a live Claude Code session
    // (a dev server started inside one). A CLI in the PTY then thinks it
    // is a child session and turns off transcript saving, which breaks
    // resume. Halv terminals are always top-level sessions.
    for name in CLAUDE_CODE_SESSION_MARKERS {
        remove_resolved_env(&mut command, OsString::from(name));
    }
    // Orca agent hooks report to the Orca pane that these variables name.
    for name in crate::agent_status::orca_variables() {
        remove_resolved_env(&mut command, name);
    }
    apply_appimage_environment(&mut command);
    for (name, value) in crate::locale::locale_environment() {
        set_resolved_env(&mut command, name, value);
    }
    set_resolved_env(
        &mut command,
        OsString::from("TERM"),
        OsString::from("xterm-256color"),
    );
    set_resolved_env(
        &mut command,
        OsString::from("COLORTERM"),
        OsString::from("truecolor"),
    );
    set_resolved_env(
        &mut command,
        OsString::from("HALV_TERMINAL"),
        OsString::from("1"),
    );
    command
}

#[cfg(target_os = "linux")]
fn apply_appimage_environment(command: &mut ResolvedCommand) {
    if !host_env::running_from_appimage() {
        return;
    }
    for name in host_env::APPIMAGE_RUNTIME_VARS {
        remove_resolved_env(command, OsString::from(name));
    }
    let path = resolved_environment_value(command, "PATH").unwrap_or_default();
    set_resolved_env(
        command,
        OsString::from("PATH"),
        host_env::scrubbed_appimage_path(&path),
    );
    match resolved_environment_value(command, "XDG_DATA_DIRS")
        .as_deref()
        .and_then(|value| host_env::scrubbed_appimage_var("XDG_DATA_DIRS", value))
    {
        Some(value) => set_resolved_env(command, OsString::from("XDG_DATA_DIRS"), value),
        None => remove_resolved_env(command, OsString::from("XDG_DATA_DIRS")),
    }
}

#[cfg(not(target_os = "linux"))]
fn apply_appimage_environment(_command: &mut ResolvedCommand) {}

#[cfg(target_os = "linux")]
fn resolved_environment_value(command: &ResolvedCommand, name: &str) -> Option<OsString> {
    let name = std::ffi::OsStr::new(name);
    if let Some((_, value)) = command
        .env
        .iter()
        .rev()
        .find(|(candidate, _)| candidate.as_os_str() == name)
    {
        return Some(value.clone());
    }
    if command
        .env_remove
        .iter()
        .any(|candidate| candidate.as_os_str() == name)
    {
        return None;
    }
    std::env::var_os(name)
}

fn set_resolved_env(command: &mut ResolvedCommand, name: OsString, value: OsString) {
    command.env_remove.retain(|candidate| candidate != &name);
    command.env.retain(|(candidate, _)| candidate != &name);
    command.env.push((name, value));
}

fn remove_resolved_env(command: &mut ResolvedCommand, name: OsString) {
    command.env.retain(|(candidate, _)| candidate != &name);
    if !command.env_remove.contains(&name) {
        command.env_remove.push(name);
    }
}

fn portable_pty_command(command: &ResolvedCommand, cwd: &Path) -> CommandBuilder {
    let mut builder = CommandBuilder::new(&command.program);
    for argument in &command.args {
        builder.arg(argument);
    }
    for name in &command.env_remove {
        builder.env_remove(name);
    }
    for (name, value) in &command.env {
        builder.env(name, value);
    }
    builder.cwd(cwd);
    builder
}

const CLAUDE_CODE_SESSION_MARKERS: [&str; 4] = [
    "CLAUDECODE",
    "CLAUDE_CODE_CHILD_SESSION",
    "CLAUDE_CODE_SSE_PORT",
    "CLAUDE_CODE_ENTRYPOINT",
];

fn spawn_output_reader(
    sink: TerminalEventSink,
    manager: TerminalManager,
    session: Arc<ManagedTerminal>,
    mut reader: Box<dyn Read + Send>,
) {
    thread::spawn(move || {
        let mut buffer = [0_u8; 8192];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) => break,
                Ok(read) => session.emit_data(&sink, &manager, &buffer[..read]),
                Err(error) => {
                    if !session.closing.load(Ordering::SeqCst) {
                        session.emit_terminal(
                            &sink,
                            None,
                            "error",
                            None,
                            None,
                            Some(format!("terminal output failed: {error}")),
                        );
                    }
                    break;
                }
            }
        }
    });
}

#[cfg(unix)]
fn spawn_supervisor_attach(
    app_data_dir: PathBuf,
    app_version: String,
    manager: TerminalManager,
    session: Arc<ManagedTerminal>,
    sink: TerminalEventSink,
    supervisor_sink: SupervisorConnectionEventSink,
    register_on_success: bool,
) -> TerminalResult<()> {
    let socket = crate::supervisor_link::socket_path(&app_data_dir);
    let attach_version = app_version.clone();
    let ensure_manager = manager.clone();
    spawn_supervisor_attach_with(
        manager,
        session,
        SupervisorAttachSinks {
            terminal: sink,
            connection: supervisor_sink,
        },
        register_on_success,
        SupervisorAttachHooks {
            ensure: move || {
                let client = crate::supervisor_link::ensure_running(&app_data_dir, &app_version)?;
                *ensure_manager
                    .supervisor
                    .lock()
                    .expect("terminal supervisor lock poisoned") = Some(client);
                Ok(())
            },
            attach: move |id: &str, from_sequence: Option<u64>| {
                SupervisorClient::attach_from_sequence(&socket, &attach_version, id, from_sequence)
            },
            wait: thread::sleep,
        },
    )
}

#[cfg(unix)]
fn spawn_supervisor_attach_with<Ensure, Attach, Events, Wait>(
    manager: TerminalManager,
    session: Arc<ManagedTerminal>,
    sinks: SupervisorAttachSinks,
    register_on_success: bool,
    hooks: SupervisorAttachHooks<Ensure, Attach, Wait>,
) -> TerminalResult<()>
where
    Ensure: FnMut() -> TerminalResult<()> + Send + 'static,
    Attach: FnMut(&str, Option<u64>) -> TerminalResult<(halv_supervisor::Snapshot, Events)>
        + Send
        + 'static,
    Events: Iterator<Item = TerminalResult<SupervisorEvent>> + Send + 'static,
    Wait: FnMut(Duration) + Send + 'static,
{
    let SupervisorAttachSinks {
        terminal: sink,
        connection: supervisor_sink,
    } = sinks;
    let SupervisorAttachHooks {
        mut ensure,
        mut attach,
        mut wait,
    } = hooks;
    let (snapshot_ready_tx, snapshot_ready_rx) = std::sync::mpsc::channel();
    thread::spawn(move || {
        let id = session.snapshot().id;
        let mut registered = !register_on_success;
        let mut reconnect_message = None;
        let mut events = match attach(&id, None) {
            Ok((snapshot, events)) => {
                apply_supervisor_attach_snapshot(&session, &manager, &sink, snapshot, None);
                if register_on_success {
                    manager
                        .sessions
                        .lock()
                        .expect("terminal sessions lock poisoned")
                        .insert(id.clone(), session.clone());
                    registered = true;
                }
                let _ = snapshot_ready_tx.send(Ok(()));
                Some(events)
            }
            Err(error) => {
                eprintln!("terminal supervisor: initial attach for `{id}` failed: {error}");
                let _ = snapshot_ready_tx.send(Err(error.clone()));
                match classify_supervisor_reconnect_error(&error) {
                    SupervisorReconnectDecision::Retry => {
                        reconnect_message = Some(error);
                        None
                    }
                    decision => {
                        if registered {
                            finish_supervisor_attach(&session, &manager, &sink, decision, &error);
                        }
                        return;
                    }
                }
            }
        };

        let mut attempt = 0_u32;
        loop {
            if let Some(mut attached_events) = events.take() {
                let connected_at = Instant::now();
                reconnect_message = Some("connection closed".to_string());
                for event in &mut attached_events {
                    if session.closing.load(Ordering::SeqCst) {
                        return;
                    }
                    match event {
                        Ok(SupervisorEvent::Data { sequence, data }) => {
                            session.emit_supervised_data(&sink, &manager, sequence, &data, false);
                        }
                        Ok(SupervisorEvent::Exit {
                            sequence,
                            exit_code,
                            signal,
                        }) => {
                            session.finish(
                                &sink,
                                &manager,
                                TerminalFinish {
                                    sequence: Some(sequence),
                                    status: "exited",
                                    exit_code,
                                    signal,
                                    message: None,
                                },
                            );
                            manager.maybe_swap_stale_supervisor();
                            return;
                        }
                        Err(error) => match classify_supervisor_reconnect_error(&error) {
                            SupervisorReconnectDecision::Retry => {
                                reconnect_message = Some(error);
                                break;
                            }
                            decision => {
                                if registered {
                                    finish_supervisor_attach(
                                        &session, &manager, &sink, decision, &error,
                                    );
                                }
                                return;
                            }
                        },
                    }
                }
                attempt = reconnect_attempt_after_connection(attempt, connected_at.elapsed());
            }

            if session.closing.load(Ordering::SeqCst) {
                return;
            }
            let last_sequence = session
                .output
                .lock()
                .expect("terminal output lock poisoned")
                .sequence();
            attempt = attempt.saturating_add(1);
            let message = reconnect_message
                .take()
                .unwrap_or_else(|| "connection closed".to_string());
            supervisor_sink(SupervisorConnectionEvent::Reconnecting(
                TerminalSupervisorReconnectingEvent {
                    id: id.clone(),
                    attempt,
                    message: message.clone(),
                },
            ));
            let delay = supervisor_reconnect_delay(attempt);
            eprintln!(
                "terminal supervisor: reconnecting `{id}` on attempt {attempt} in {} ms: {message}",
                delay.as_millis()
            );
            wait(delay);
            if session.closing.load(Ordering::SeqCst) {
                return;
            }

            if let Err(error) = ensure() {
                eprintln!(
                    "terminal supervisor: ensure failed for `{id}` on attempt {attempt}: {error}"
                );
                match classify_supervisor_reconnect_error(&error) {
                    SupervisorReconnectDecision::Retry => {
                        reconnect_message = Some(error);
                        continue;
                    }
                    decision => {
                        if registered {
                            finish_supervisor_attach(&session, &manager, &sink, decision, &error);
                        }
                        return;
                    }
                }
            }
            if session.closing.load(Ordering::SeqCst) {
                return;
            }

            // The client accepts the first desired sequence and converts it to a protocol cursor.
            let resume_sequence = last_sequence.saturating_add(1);
            match attach(&id, Some(resume_sequence)) {
                Ok((snapshot, resumed_events)) => {
                    apply_supervisor_attach_snapshot(
                        &session,
                        &manager,
                        &sink,
                        snapshot,
                        Some(resume_sequence),
                    );
                    if register_on_success && !registered {
                        manager
                            .sessions
                            .lock()
                            .expect("terminal sessions lock poisoned")
                            .insert(id.clone(), session.clone());
                        registered = true;
                    }
                    eprintln!("terminal supervisor: reconnected `{id}` on attempt {attempt}");
                    supervisor_sink(SupervisorConnectionEvent::Reconnected(
                        TerminalSupervisorReconnectedEvent {
                            id: id.clone(),
                            attempt,
                        },
                    ));
                    events = Some(resumed_events);
                }
                Err(error) => {
                    eprintln!(
                        "terminal supervisor: attach failed for `{id}` on attempt {attempt}: {error}"
                    );
                    match classify_supervisor_reconnect_error(&error) {
                        SupervisorReconnectDecision::Retry => {
                            reconnect_message = Some(error);
                        }
                        decision => {
                            if registered {
                                finish_supervisor_attach(
                                    &session, &manager, &sink, decision, &error,
                                );
                            }
                            return;
                        }
                    }
                }
            }
        }
    });
    // Bound the wait so a dead supervisor socket cannot hang the command.
    match snapshot_ready_rx.recv_timeout(SUPERVISOR_ATTACH_TIMEOUT) {
        Ok(result) => result,
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => Err(format!(
            "supervisor attach timed out after {} seconds",
            SUPERVISOR_ATTACH_TIMEOUT.as_secs()
        )),
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
            Err("failed to receive supervisor attach result".to_string())
        }
    }
}

#[cfg(unix)]
fn apply_supervisor_attach_snapshot(
    session: &ManagedTerminal,
    manager: &TerminalManager,
    sink: &TerminalEventSink,
    snapshot: halv_supervisor::Snapshot,
    resume_sequence: Option<u64>,
) {
    let current_sequence = session
        .output
        .lock()
        .expect("terminal output lock poisoned")
        .sequence();
    if resume_sequence.is_none() {
        session
            .output
            .lock()
            .expect("terminal output lock poisoned")
            .replace_with_snapshot(snapshot.sequence, &snapshot.scrollback);
    } else if snapshot.sequence > current_sequence {
        // A resumed snapshot contains only chunks after the requested sequence.
        session.emit_supervised_data(sink, manager, snapshot.sequence, &snapshot.scrollback, true);
    }
    apply_supervisor_summary(
        &mut session
            .record
            .lock()
            .expect("terminal record lock poisoned"),
        &snapshot.session,
    );
    manager.persist_scrollback(session);
}

#[cfg(unix)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SupervisorReconnectDecision {
    Retry,
    SessionEnded,
    ProtocolMismatch,
    VersionMismatch,
}

#[cfg(unix)]
fn supervisor_reconnect_delay(attempt: u32) -> Duration {
    let index = attempt.saturating_sub(1) as usize;
    SUPERVISOR_RECONNECT_DELAYS[index.min(SUPERVISOR_RECONNECT_DELAYS.len() - 1)]
}

#[cfg(unix)]
fn reconnect_attempt_after_connection(attempt: u32, connected_for: Duration) -> u32 {
    if connected_for >= SUPERVISOR_RECONNECT_STABLE_INTERVAL {
        0
    } else {
        attempt
    }
}

#[cfg(unix)]
fn classify_supervisor_reconnect_error(error: &str) -> SupervisorReconnectDecision {
    if supervisor_not_found(error) {
        SupervisorReconnectDecision::SessionEnded
    } else if is_supervisor_version_mismatch(error) {
        SupervisorReconnectDecision::VersionMismatch
    } else if crate::supervisor_link::is_protocol_mismatch(error) {
        SupervisorReconnectDecision::ProtocolMismatch
    } else {
        SupervisorReconnectDecision::Retry
    }
}

#[cfg(unix)]
fn finish_supervisor_attach(
    session: &ManagedTerminal,
    manager: &TerminalManager,
    sink: &TerminalEventSink,
    decision: SupervisorReconnectDecision,
    error: &str,
) {
    if session.closing.load(Ordering::SeqCst) {
        return;
    }
    let message = match decision {
        SupervisorReconnectDecision::SessionEnded => {
            "terminal session ended while the supervisor was unavailable".to_string()
        }
        SupervisorReconnectDecision::ProtocolMismatch => error.to_string(),
        SupervisorReconnectDecision::VersionMismatch => error.to_string(),
        SupervisorReconnectDecision::Retry => return,
    };
    let id = session.snapshot().id;
    eprintln!("terminal supervisor: reconnect for `{id}` stopped: {message}");
    let sequence = session
        .output
        .lock()
        .expect("terminal output lock poisoned")
        .sequence()
        .saturating_add(1);
    session.finish(
        sink,
        manager,
        TerminalFinish {
            sequence: Some(sequence),
            status: "failed",
            exit_code: None,
            signal: None,
            message: Some(message),
        },
    );
}

#[cfg(unix)]
fn apply_supervisor_summary(record: &mut TerminalSession, summary: &SessionSummary) {
    record.id.clone_from(&summary.id);
    record.pid = summary.pid;
    record.cwd.clone_from(&summary.cwd);
    record.shell.clone_from(&summary.program);
    record.status = match summary.status {
        SessionStatus::Running => "running",
        SessionStatus::Exited => "exited",
    }
    .to_string();
    record.cols = summary.cols;
    record.rows = summary.rows;
    record.started_at.clone_from(&summary.started_at);
    record.exited_at.clone_from(&summary.exited_at);
    record.exit_code = summary.exit_code;
    record.signal.clone_from(&summary.signal);
}

// The supervisor list after a failed bulk kill: a session that still runs
// was not killed, so its stop mark goes away.
#[cfg(unix)]
fn clear_stop_marks_of_running_sessions(
    marked: &[Arc<ManagedTerminal>],
    listed: &[SessionSummary],
) {
    for session in marked {
        let id = session.snapshot().id;
        if listed
            .iter()
            .any(|summary| summary.id == id && summary.status == SessionStatus::Running)
        {
            session.stop_requested.store(false, Ordering::SeqCst);
        }
    }
}

#[cfg(unix)]
fn supervisor_not_found(error: &str) -> bool {
    error.contains("terminal session `") && error.ends_with("` was not found")
}

#[cfg(unix)]
fn supervisor_still_running(error: &str) -> bool {
    error.contains("terminal session `") && error.ends_with("` still running")
}

#[cfg(unix)]
fn unattachable_supervisor_session(error: &str) -> bool {
    if crate::supervisor_link::is_broken_connection(error)
        || crate::supervisor_link::is_protocol_mismatch(error)
    {
        return false;
    }
    supervisor_not_found(error) || error == "terminal session is not running"
}

#[tauri::command]
pub(crate) fn terminal_restore_read(
    manager: tauri::State<'_, TerminalManager>,
    scope: String,
) -> TerminalResult<Option<TerminalPanelState>> {
    if scope.trim().is_empty() {
        return Err("terminal restore scope cannot be empty".to_string());
    }
    let panel = manager
        .persistence
        .lock()
        .expect("terminal persistence lock poisoned")
        .panel(&scope);
    if let Some(panel) = &panel {
        log_restore_shape("read", &scope, &panel.layout, panel.panes.len());
    }
    Ok(panel)
}

#[tauri::command]
pub(crate) fn terminal_restore_read_all(
    manager: tauri::State<'_, TerminalManager>,
) -> TerminalResult<Vec<ScopedTerminalPanelState>> {
    Ok(manager
        .persistence
        .lock()
        .expect("terminal persistence lock poisoned")
        .panels())
}

// Diagnostic shape line for the dev log: layout degradation (a split saved
// as flat tabs) is invisible in the UI until the next restore, so every
// write and read reports its shape.
fn log_restore_shape(operation: &str, scope: &str, layout: &serde_json::Value, panes: usize) {
    let tabs = layout
        .get("tabs")
        .and_then(serde_json::Value::as_array)
        .map(Vec::len)
        .unwrap_or(0);
    let splits = layout.to_string().matches("\"split\"").count();
    let scope_tail: String = scope
        .chars()
        .rev()
        .take(12)
        .collect::<String>()
        .chars()
        .rev()
        .collect();
    eprintln!(
        "terminal-restore {operation} scope=…{scope_tail} tabs={tabs} splits={splits} panes={panes}"
    );
}

#[tauri::command]
pub(crate) fn terminal_restore_write(
    manager: tauri::State<'_, TerminalManager>,
    scope: String,
    layout: serde_json::Value,
    panes: Vec<TerminalPaneStateInput>,
) -> TerminalResult<()> {
    if scope.trim().is_empty() {
        return Err("terminal restore scope cannot be empty".to_string());
    }
    log_restore_shape("write", &scope, &layout, panes.len());
    let live_scrollback = panes
        .iter()
        .filter_map(|pane| {
            manager
                .get(&pane.session_id)
                .ok()
                .map(|session| (pane.session_id.clone(), session.attach().scrollback))
        })
        .collect::<HashMap<_, _>>();
    manager
        .persistence
        .lock()
        .expect("terminal persistence lock poisoned")
        .save_panel(scope, layout, panes, &live_scrollback)
        .map_err(|error| format!("failed to save terminal state: {error}"))
}

#[tauri::command]
pub(crate) fn terminal_restore_shell(
    app: AppHandle,
    manager: tauri::State<'_, TerminalManager>,
    scope: String,
    session_id: String,
    cols: u16,
    rows: u16,
) -> TerminalResult<TerminalSession> {
    let pane = manager
        .persistence
        .lock()
        .expect("terminal persistence lock poisoned")
        .pane(&scope, &session_id)
        .ok_or_else(|| format!("saved terminal `{session_id}` was not found"))?;
    if pane.agent_kind.is_some() {
        return Err("agent terminals must resume through Agent Sessions".to_string());
    }
    // The scope of a shell tab panel gives the owner back.
    let shell_tab_id = ShellSource::from_saved_pane(&pane, &scope).shell_tab_id;
    spawn_terminal(
        app,
        manager.inner(),
        TerminalSpawnRequest {
            id: None,
            project_id: pane.project_id,
            conversation_id: pane.conversation_id,
            project_name: pane.project_name,
            cwd: PathBuf::from(pane.cwd),
            cols,
            rows,
            execution_profile: pane.execution_profile,
            agent_kind: None,
            program: None,
            shell_tab_id,
        },
    )
}

#[tauri::command]
pub fn terminal_list(
    manager: tauri::State<'_, TerminalManager>,
) -> TerminalResult<Vec<TerminalSession>> {
    Ok(manager.list())
}

#[tauri::command]
pub fn terminal_attach(
    manager: tauri::State<'_, TerminalManager>,
    session_id: String,
) -> TerminalResult<TerminalAttach> {
    Ok(manager.get(&session_id)?.attach_with_prelude())
}

/// Checks a chunk of input on the writer thread, just before the write.
fn check_input(session: &ManagedTerminal, data: &str) -> TerminalResult<()> {
    if data.len() > MAX_TERMINAL_WRITE_BYTES {
        return Err(format!(
            "terminal input exceeds the {MAX_TERMINAL_WRITE_BYTES}-byte limit"
        ));
    }
    if session.snapshot().status != "running" {
        return Err("terminal session is not running".to_string());
    }
    Ok(())
}

fn write_in_process_input(session: &ManagedTerminal, data: &str) -> TerminalResult<()> {
    check_input(session, data)?;
    let Backend::InProcess { writer, .. } = &session.backend else {
        return Err("terminal input is closed".to_string());
    };
    let mut writer = writer.lock().expect("terminal writer lock poisoned");
    let writer = writer
        .as_mut()
        .ok_or_else(|| "terminal input is closed".to_string())?;
    writer
        .write_all(data.as_bytes())
        .and_then(|()| writer.flush())
        .map_err(|error| format!("failed to write terminal input: {error}"))
}

/// Sends input over a supervisor connection that only the writer thread of
/// the session uses. A broken connection opens again for the next write. A
/// failed write is never sent again, because the supervisor can have written
/// it already.
#[cfg(unix)]
fn supervised_input_sink(
    session: Arc<ManagedTerminal>,
    session_id: String,
    connect: impl Fn() -> TerminalResult<SupervisorClient> + Send + 'static,
) -> InputSink {
    let mut client: Option<SupervisorClient> = None;
    Box::new(move |data: &str| {
        check_input(&session, data)?;
        let connected = match client.take() {
            Some(connected) => connected,
            None => connect()?,
        };
        let result = client.insert(connected).write(&session_id, data.as_bytes());
        if result
            .as_ref()
            .err()
            .is_some_and(|error| crate::supervisor_link::is_broken_connection(error))
        {
            client = None;
        }
        result
    })
}

// A sync command runs on the app main thread, which also delivers the keys to
// the webview. This command only queues the input, so it never waits for the
// supervisor. Tauri runs async calls in any order, so a pane sends `writer`
// and `sequence`, and the queue restores the order.
#[tauri::command]
pub async fn terminal_write(
    app: AppHandle,
    manager: tauri::State<'_, TerminalManager>,
    storage: tauri::State<'_, Storage>,
    session_id: String,
    data: String,
    writer: Option<String>,
    sequence: Option<u64>,
) -> TerminalResult<()> {
    let order = match (writer, sequence) {
        (Some(writer), Some(sequence)) if sequence > 0 => Some(InputOrder { writer, sequence }),
        (None, None) => None,
        _ => return Err("terminal input needs a writer and a sequence from 1".to_string()),
    };
    let activity_conversation = if input_marks_activity(&data) {
        manager.session(&session_id)?.conversation_id
    } else {
        None
    };
    let agent_input = crate::agent_status::input_signal(&data);
    let app_version = app.package_info().version.to_string();
    let written = manager.queue_input(&session_id, data, order, &app_version)?;
    written
        .await
        .map_err(|_| "the terminal input writer stopped".to_string())??;
    // Agent status reads single keys at the time that they reach the PTY.
    if let Some(signal) = agent_input {
        crate::agent_status::note_terminal_input(&app, &session_id, signal);
    }

    if let Some(conversation_id) = activity_conversation {
        if manager.mark_conversation_activity(&conversation_id) {
            if let Err(error) = storage.touch_conversation(&conversation_id) {
                eprintln!("failed to update conversation activity: {error}");
            }
            if let Err(error) = emit_conversation_activity(&app, &conversation_id) {
                eprintln!("failed to emit conversation activity: {error}");
            }
        }
    }

    Ok(())
}

#[tauri::command]
pub fn terminal_resize(
    manager: tauri::State<'_, TerminalManager>,
    session_id: String,
    cols: u16,
    rows: u16,
) -> TerminalResult<TerminalSession> {
    let session = manager.get(&session_id)?;
    let size = terminal_size(cols, rows);
    match &session.backend {
        Backend::InProcess { master, .. } => {
            master
                .lock()
                .expect("terminal master lock poisoned")
                .resize(size)
                .map_err(|error| format!("failed to resize terminal: {error}"))?;
            let mut record = session
                .record
                .lock()
                .expect("terminal record lock poisoned");
            record.cols = size.cols;
            record.rows = size.rows;
        }
        #[cfg(unix)]
        Backend::Supervised => {
            let summary = manager.with_supervisor(env!("CARGO_PKG_VERSION"), |client| {
                client.resize(&session_id, size.cols, size.rows)
            })?;
            apply_supervisor_summary(
                &mut session
                    .record
                    .lock()
                    .expect("terminal record lock poisoned"),
                &summary,
            );
        }
    }
    let resized = session.snapshot();
    notify_terminal_tap(TerminalTapEvent::Resized {
        session_id: &resized.id,
        cols: resized.cols,
        rows: resized.rows,
    });
    Ok(resized)
}

#[tauri::command]
pub fn terminal_rename(
    manager: tauri::State<'_, TerminalManager>,
    session_id: String,
    title: Option<String>,
) -> TerminalResult<TerminalSession> {
    let session = manager.get(&session_id)?;
    let title = title
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    let mut updated = session.snapshot();
    updated.title.clone_from(&title);
    #[cfg(unix)]
    if matches!(&session.backend, Backend::Supervised) {
        let meta = serde_json::to_value(&updated)
            .map_err(|error| format!("failed to encode terminal metadata: {error}"))?;
        manager.with_supervisor(env!("CARGO_PKG_VERSION"), |client| {
            client.update_meta(&session_id, meta).map(|_| ())
        })?;
    }
    session
        .record
        .lock()
        .expect("terminal record lock poisoned")
        .title
        .clone_from(&title);
    manager
        .persistence
        .lock()
        .expect("terminal persistence lock poisoned")
        .update_title(&session_id, title.as_deref())
        .map_err(|error| format!("failed to rename saved terminal: {error}"))?;
    notify_terminal_tap(TerminalTapEvent::Changed);
    Ok(session.snapshot())
}

/// "Open in tab" (ADR 0010) and terminal adoption (ADR 0020): a shell tab
/// adopts a running terminal that has no owner, a plain shell or an agent
/// terminal without a conversation. The owner travels in the supervisor
/// metadata, as a rename does, so a supervisor restart keeps it.
#[tauri::command]
pub fn terminal_set_shell_tab(
    manager: tauri::State<'_, TerminalManager>,
    storage: tauri::State<'_, Storage>,
    session_id: String,
    shell_tab_id: String,
) -> TerminalResult<TerminalSession> {
    let session = manager.get(&session_id)?;
    let mut updated = session.snapshot();
    let shell_tab_id = check_shell_adoption(&updated, &shell_tab_id, |key| {
        storage
            .workspace_tab_shell_workspace(key)
            .map_err(|error| error.to_string())
    })?;
    if updated.shell_tab_id.as_deref() == Some(shell_tab_id.as_str()) {
        return Ok(updated);
    }
    updated.shell_tab_id = Some(shell_tab_id.clone());
    #[cfg(unix)]
    if matches!(&session.backend, Backend::Supervised) {
        let meta = serde_json::to_value(&updated)
            .map_err(|error| format!("failed to encode terminal metadata: {error}"))?;
        manager.with_supervisor(env!("CARGO_PKG_VERSION"), |client| {
            client.update_meta(&session_id, meta).map(|_| ())
        })?;
    }
    session
        .record
        .lock()
        .expect("terminal record lock poisoned")
        .shell_tab_id = Some(shell_tab_id);
    Ok(session.snapshot())
}

/// The rules of "Open in tab". Only a running terminal without an owner
/// moves, and only into a shell tab that exists. A standalone agent
/// terminal moves too (ADR 0020). A conversation terminal never moves. A
/// repeat of the same move is accepted. Gives the trimmed shell tab id.
fn check_shell_adoption(
    session: &TerminalSession,
    shell_tab_id: &str,
    shell_tab_workspace: impl FnOnce(&str) -> TerminalResult<Option<String>>,
) -> TerminalResult<String> {
    let shell_tab_id = shell_tab_id.trim();
    if shell_tab_id.is_empty() {
        return Err("a shell tab id is required".to_string());
    }
    if session
        .conversation_id
        .as_deref()
        .is_some_and(|id| !id.trim().is_empty())
    {
        return Err("a conversation terminal cannot move to a shell tab".to_string());
    }
    if session.status != "running" {
        return Err("only a running shell can move to a shell tab".to_string());
    }
    match session.shell_tab_id.as_deref() {
        Some(current) if current == shell_tab_id => {}
        Some(_) => return Err("the shell already belongs to another shell tab".to_string()),
        None => {}
    }
    check_terminal_owner(None, Some(shell_tab_id), None, shell_tab_workspace)?;
    Ok(shell_tab_id.to_string())
}

// A close can wait seconds for the process to exit, so it runs on a blocking
// thread and never on the app main thread.
#[tauri::command]
pub async fn terminal_close(
    manager: tauri::State<'_, TerminalManager>,
    session_id: String,
) -> TerminalResult<TerminalSession> {
    let manager = manager.inner().clone();
    tauri::async_runtime::spawn_blocking(move || manager.close_session(&session_id))
        .await
        .map_err(|error| format!("failed to close terminal: {error}"))?
}

#[tauri::command]
pub fn terminal_forget(
    manager: tauri::State<'_, TerminalManager>,
    session_id: String,
) -> TerminalResult<()> {
    manager.forget_session(&session_id)
}

#[tauri::command]
pub fn terminal_supervisor_stop_all(
    manager: tauri::State<'_, TerminalManager>,
) -> TerminalResult<()> {
    let result = manager.stop_all_supervised();
    // This command does not quit the app, so a later clean exit can continue
    // in a shell again. The marks still cover each session that the kill ends.
    manager.stopping.store(false, Ordering::SeqCst);
    result
}

#[tauri::command]
pub fn terminal_supervisor_status(
    manager: tauri::State<'_, TerminalManager>,
) -> TerminalSupervisorReport {
    manager.supervisor_report(manager.supervisor_status())
}

#[tauri::command]
pub fn terminal_supervisor_retry(
    manager: tauri::State<'_, TerminalManager>,
) -> TerminalSupervisorReport {
    manager.supervisor_report(manager.retry_supervisor())
}

#[tauri::command]
pub fn terminal_supervisor_restart(
    manager: tauri::State<'_, TerminalManager>,
    kill_sessions: bool,
) -> TerminalSupervisorReport {
    manager.supervisor_report(manager.restart_supervisor(kill_sessions))
}

/// Opens a plain shell next to any pane: a live session of any kind, or a
/// restored pane that `scope` saved.
#[tauri::command]
pub(crate) fn terminal_open_shell(
    app: AppHandle,
    manager: tauri::State<'_, TerminalManager>,
    session_id: String,
    scope: Option<String>,
    cols: u16,
    rows: u16,
) -> TerminalResult<TerminalSession> {
    let source = shell_source_for(manager.inner(), &session_id, scope.as_deref())?;
    spawn_terminal(
        app,
        manager.inner(),
        plain_shell_request(source, cols, rows),
    )
}

/// "Open shell here" on an ended agent pane. The automatic path after a
/// clean exit uses the same function.
#[tauri::command]
pub(crate) fn terminal_continue_shell(
    app: AppHandle,
    manager: tauri::State<'_, TerminalManager>,
    session_id: String,
) -> TerminalResult<TerminalSession> {
    continue_with_shell(&app, manager.inner(), &session_id)
}

/// The foreground state of one shell, for the close prompt of a tab.
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ForegroundState {
    Busy,
    Idle,
    Unknown,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TerminalForegroundBusy {
    session_id: String,
    state: ForegroundState,
}

/// The probe stops after this time. Each shell that it did not read is
/// unknown, and the close prompt counts unknown as busy.
const FOREGROUND_PROBE_LIMIT: Duration = Duration::from_secs(1);

/// A shell is busy while a foreground job runs. On Unix, the foreground
/// process group of the terminal is then not the group of the shell.
/// Windows has no such probe, so a running shell there is unknown.
#[tauri::command]
pub async fn terminal_foreground_busy(
    manager: tauri::State<'_, TerminalManager>,
    session_ids: Vec<String>,
) -> TerminalResult<Vec<TerminalForegroundBusy>> {
    let manager = manager.inner().clone();
    tauri::async_runtime::spawn_blocking(move || foreground_busy(&manager, &session_ids))
        .await
        .map_err(|error| format!("failed to probe the terminals: {error}"))
}

fn foreground_busy(
    manager: &TerminalManager,
    session_ids: &[String],
) -> Vec<TerminalForegroundBusy> {
    let records = session_ids
        .iter()
        .map(|id| (id.clone(), manager.session(id).ok()))
        .collect::<Vec<_>>();
    let pids = records
        .iter()
        .filter_map(|(_, record)| record.as_ref())
        .filter(|record| record.status == "running" && record.pid > 0)
        .map(|record| record.pid)
        .collect::<Vec<_>>();
    let groups = if pids.is_empty() {
        HashMap::new()
    } else {
        probe_process_groups(&pids)
    };
    records
        .into_iter()
        .map(|(session_id, record)| {
            let state = match record {
                None => ForegroundState::Unknown,
                Some(record) if record.status != "running" => ForegroundState::Idle,
                Some(record) => foreground_state(groups.get(&record.pid).copied()),
            };
            TerminalForegroundBusy { session_id, state }
        })
        .collect()
}

/// The process group and the terminal foreground group of one process.
type ProcessGroups = (i64, i64);

/// Reads `pid pgid tpgid` lines from `ps -o pid=,pgid=,tpgid=`.
fn parse_process_groups(output: &str) -> HashMap<u32, ProcessGroups> {
    output
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let pid = fields.next()?.parse::<u32>().ok()?;
            let pgid = fields.next()?.parse::<i64>().ok()?;
            let tpgid = fields.next()?.parse::<i64>().ok()?;
            Some((pid, (pgid, tpgid)))
        })
        .collect()
}

fn foreground_state(groups: Option<ProcessGroups>) -> ForegroundState {
    match groups {
        Some((pgid, tpgid)) if pgid > 0 && tpgid > 0 => {
            if tpgid == pgid {
                ForegroundState::Idle
            } else {
                ForegroundState::Busy
            }
        }
        _ => ForegroundState::Unknown,
    }
}

#[cfg(unix)]
fn probe_process_groups(pids: &[u32]) -> HashMap<u32, ProcessGroups> {
    let list = pids
        .iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(",");
    let Ok(mut child) = std::process::Command::new("ps")
        .args(["-o", "pid=,pgid=,tpgid=", "-p", &list])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
    else {
        return HashMap::new();
    };
    let deadline = Instant::now() + FOREGROUND_PROBE_LIMIT;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(10)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return HashMap::new();
            }
        }
    }
    let mut output = String::new();
    if let Some(mut stdout) = child.stdout.take() {
        let _ = stdout.read_to_string(&mut output);
    }
    parse_process_groups(&output)
}

#[cfg(not(unix))]
fn probe_process_groups(_pids: &[u32]) -> HashMap<u32, ProcessGroups> {
    HashMap::new()
}

fn terminal_size(cols: u16, rows: u16) -> PtySize {
    PtySize {
        cols: cols.clamp(20, 500),
        rows: rows.clamp(5, 300),
        pixel_width: 0,
        pixel_height: 0,
    }
}

fn terminal_shell() -> PathBuf {
    #[cfg(windows)]
    {
        return std::env::var_os("COMSPEC")
            .map(PathBuf::from)
            .filter(|path| path.is_file())
            .unwrap_or_else(|| PathBuf::from("powershell.exe"));
    }
    #[cfg(not(windows))]
    {
        std::env::var_os("SHELL")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute() && path.is_file())
            .unwrap_or_else(|| PathBuf::from("/bin/sh"))
    }
}

fn add_login_argument(arguments: &mut Vec<OsString>, shell: &Path) {
    #[cfg(not(windows))]
    {
        let name = shell.file_name().and_then(|value| value.to_str());
        if matches!(name, Some("bash" | "zsh" | "fish" | "sh" | "ksh")) {
            arguments.push(OsString::from("-l"));
        }
    }
    #[cfg(windows)]
    let _ = (arguments, shell);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[derive(Clone, Default)]
    struct RestartTestState {
        shutdown_calls: Arc<Mutex<Vec<bool>>>,
        closed_sessions: Arc<std::sync::atomic::AtomicUsize>,
    }

    #[cfg(unix)]
    struct RestartTestClient {
        sessions: usize,
        running_sessions: usize,
        state: RestartTestState,
    }

    #[cfg(unix)]
    impl SupervisorRestartClient for RestartTestClient {
        fn session_counts(&mut self) -> TerminalResult<(usize, usize)> {
            Ok((self.sessions, self.running_sessions))
        }

        fn shutdown(&mut self, kill_sessions: bool) -> TerminalResult<()> {
            self.state
                .shutdown_calls
                .lock()
                .expect("shutdown calls lock poisoned")
                .push(kill_sessions);
            if !kill_sessions && self.running_sessions > 0 {
                return Err(SUPERVISOR_RUNNING_SESSIONS_ERROR.to_string());
            }
            if kill_sessions {
                self.state
                    .closed_sessions
                    .fetch_add(self.running_sessions, Ordering::SeqCst);
                self.running_sessions = 0;
            }
            Ok(())
        }
    }

    #[cfg(unix)]
    fn restart_test_client(
        sessions: usize,
        running_sessions: usize,
        state: RestartTestState,
    ) -> RestartTestClient {
        RestartTestClient {
            sessions,
            running_sessions,
            state,
        }
    }

    #[test]
    fn input_activity_requires_enter() {
        assert!(input_marks_activity("\r"));
        assert!(input_marks_activity("\n"));
        assert!(!input_marks_activity("abc"));
        assert!(!input_marks_activity("\x1b[A"));
    }

    #[cfg(unix)]
    #[test]
    fn clean_supervisor_restart_uses_non_destructive_shutdown() {
        let state = RestartTestState::default();
        let waited = Arc::new(AtomicBool::new(false));
        let started = Arc::new(AtomicBool::new(false));
        let outcome = restart_supervisor_with(
            restart_test_client(2, 0, state.clone()),
            false,
            {
                let waited = waited.clone();
                move || {
                    waited.store(true, Ordering::SeqCst);
                    Ok(())
                }
            },
            {
                let started = started.clone();
                let state = state.clone();
                move || {
                    started.store(true, Ordering::SeqCst);
                    Ok(restart_test_client(0, 0, state))
                }
            },
        )
        .expect("clean restart should succeed");

        assert!(matches!(outcome, SupervisorRestartOutcome::Restarted(_)));
        assert_eq!(
            *state
                .shutdown_calls
                .lock()
                .expect("shutdown calls lock poisoned"),
            [false]
        );
        assert!(waited.load(Ordering::SeqCst));
        assert!(started.load(Ordering::SeqCst));
        assert_eq!(state.closed_sessions.load(Ordering::SeqCst), 0);
    }

    #[cfg(unix)]
    #[test]
    fn non_destructive_restart_reports_running_sessions_without_closing_them() {
        let state = RestartTestState::default();
        let outcome = restart_supervisor_with(
            restart_test_client(5, 3, state.clone()),
            false,
            || panic!("a refused restart must not wait for socket release"),
            || panic!("a refused restart must not start a supervisor"),
        )
        .expect("running sessions should produce a refusal state");

        match outcome {
            SupervisorRestartOutcome::Refused {
                sessions,
                running_sessions,
                message,
                ..
            } => {
                assert_eq!(sessions, 5);
                assert_eq!(running_sessions, 3);
                assert_eq!(message, SUPERVISOR_RUNNING_SESSIONS_ERROR);
            }
            _ => panic!("restart should be refused"),
        }
        assert_eq!(
            *state
                .shutdown_calls
                .lock()
                .expect("shutdown calls lock poisoned"),
            [false]
        );
        assert_eq!(state.closed_sessions.load(Ordering::SeqCst), 0);
    }

    #[cfg(unix)]
    #[test]
    fn destructive_supervisor_restart_closes_sessions_and_starts_again() {
        let state = RestartTestState::default();
        let outcome =
            restart_supervisor_with(restart_test_client(5, 3, state.clone()), true, || Ok(()), {
                let state = state.clone();
                move || Ok(restart_test_client(0, 0, state))
            })
            .expect("confirmed restart should succeed");

        assert!(matches!(outcome, SupervisorRestartOutcome::Restarted(_)));
        assert_eq!(
            *state
                .shutdown_calls
                .lock()
                .expect("shutdown calls lock poisoned"),
            [true]
        );
        assert_eq!(state.closed_sessions.load(Ordering::SeqCst), 3);
    }

    #[cfg(unix)]
    #[test]
    fn stale_supervisor_swaps_only_without_running_sessions() {
        let state = RestartTestState::default();
        let outcome = swap_stale_supervisor_with(
            restart_test_client(2, 0, state.clone()),
            true,
            0,
            || Ok(()),
            {
                let state = state.clone();
                move || Ok(restart_test_client(0, 0, state))
            },
        );
        assert!(matches!(outcome, StaleSwapOutcome::Swapped(_)));
        assert_eq!(
            *state
                .shutdown_calls
                .lock()
                .expect("shutdown calls lock poisoned"),
            [false],
            "a swap must use the non-destructive shutdown"
        );

        let state = RestartTestState::default();
        let outcome = swap_stale_supervisor_with(
            restart_test_client(3, 1, state.clone()),
            true,
            0,
            || panic!("a busy supervisor must not stop"),
            || panic!("a busy supervisor must not be replaced"),
        );
        match outcome {
            StaleSwapOutcome::Kept(client) => assert_eq!(client.running_sessions, 1),
            _ => panic!("a supervisor with a running session must stay"),
        }
        assert!(state
            .shutdown_calls
            .lock()
            .expect("shutdown calls lock poisoned")
            .is_empty());
        assert_eq!(state.closed_sessions.load(Ordering::SeqCst), 0);
    }

    #[cfg(unix)]
    #[test]
    fn stale_supervisor_waits_for_exits_that_the_app_did_not_read() {
        // The supervisor shows no running session, but the app still waits
        // for the exit of one: its attach stream reconnects.
        let state = RestartTestState::default();
        let outcome = swap_stale_supervisor_with(
            restart_test_client(1, 0, state.clone()),
            true,
            1,
            || panic!("the supervisor still holds an unread exit"),
            || panic!("the supervisor still holds an unread exit"),
        );
        assert!(matches!(outcome, StaleSwapOutcome::Kept(_)));
        assert!(state
            .shutdown_calls
            .lock()
            .expect("shutdown calls lock poisoned")
            .is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn current_supervisor_is_never_swapped() {
        let state = RestartTestState::default();
        let outcome = swap_stale_supervisor_with(
            restart_test_client(0, 0, state.clone()),
            false,
            0,
            || panic!("a current supervisor must not stop"),
            || panic!("a current supervisor must not be replaced"),
        );
        assert!(matches!(outcome, StaleSwapOutcome::Kept(_)));
        assert!(state
            .shutdown_calls
            .lock()
            .expect("shutdown calls lock poisoned")
            .is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn refused_swap_keeps_the_old_supervisor() {
        // A session starts between the list and the shutdown request.
        struct RacingClient {
            shutdown_calls: Vec<bool>,
        }
        impl SupervisorRestartClient for RacingClient {
            fn session_counts(&mut self) -> TerminalResult<(usize, usize)> {
                Ok((1, 0))
            }

            fn shutdown(&mut self, kill_sessions: bool) -> TerminalResult<()> {
                self.shutdown_calls.push(kill_sessions);
                Err(SUPERVISOR_RUNNING_SESSIONS_ERROR.to_string())
            }
        }

        let outcome = swap_stale_supervisor_with(
            RacingClient {
                shutdown_calls: Vec::new(),
            },
            true,
            0,
            || panic!("a refused swap must not wait for the socket"),
            || panic!("a refused swap must not start a supervisor"),
        );
        match outcome {
            StaleSwapOutcome::Kept(client) => assert_eq!(client.shutdown_calls, [false]),
            _ => panic!("a refused swap must keep the old supervisor"),
        }
    }

    #[cfg(unix)]
    #[test]
    fn protocol_mismatch_is_reported_until_a_supervisor_answers() {
        let manager = TerminalManager::default();
        let status = |available| TerminalSupervisorStatus {
            available,
            socket_path: String::new(),
            pid: None,
            sessions: 0,
            running_sessions: 0,
            supervisor_version: None,
            supervisor_stale: false,
            app_version: "0.7.2".to_string(),
            message: None,
        };
        assert!(!manager.supervisor_report(status(false)).protocol_mismatch);

        *manager
            .supervisor_protocol_mismatch
            .lock()
            .expect("terminal supervisor mismatch lock poisoned") =
            Some("failed to connect to supervisor: protocol 1 does not match 2".to_string());
        let report = manager.supervisor_report(status(false));
        assert!(report.protocol_mismatch);
        let value = serde_json::to_value(&report).expect("report should encode");
        assert_eq!(value["protocolMismatch"], true);
        assert_eq!(value["available"], false);
        assert!(!manager.supervisor_report(status(true)).protocol_mismatch);
    }

    #[cfg(unix)]
    #[test]
    fn supervisor_fallback_uses_a_longer_protocol_mismatch_interval() {
        let manager = TerminalManager::default();
        assert!(manager.record_fallback("failed to connect to supervisor".to_string()));
        assert_eq!(
            manager.fallback_message().as_deref(),
            Some("failed to connect to supervisor")
        );
        assert!(!manager.record_fallback("failed again".to_string()));
        manager
            .supervisor_fallback
            .lock()
            .expect("terminal supervisor fallback lock poisoned")
            .as_mut()
            .expect("fallback should exist")
            .decided_at = Instant::now() - SUPERVISOR_FALLBACK_INTERVAL;
        assert_eq!(manager.fallback_message(), None);

        assert!(manager.record_fallback(
            "failed to connect to supervisor: protocol 0 does not match 1".to_string()
        ));
        manager
            .supervisor_fallback
            .lock()
            .expect("terminal supervisor fallback lock poisoned")
            .as_mut()
            .expect("fallback should exist")
            .decided_at = Instant::now() - SUPERVISOR_FALLBACK_INTERVAL;
        assert!(manager.fallback_message().is_some());
        assert!(!manager.record_fallback("failed again".to_string()));

        let root = PathBuf::from("/tmp").join(format!(
            "halv-supervisor-status-{}-{:016x}",
            std::process::id(),
            rand::random::<u128>()
        ));
        manager
            .set_app_data_dir(&root)
            .expect("test app data directory should initialize");
        let status = manager.supervisor_status();
        assert!(!status.available);
        assert_eq!(
            status.message.as_deref(),
            Some("failed to connect to supervisor: protocol 0 does not match 1")
        );
        assert_eq!(
            status.socket_path,
            crate::supervisor_link::socket_path(&root)
                .to_string_lossy()
                .into_owned()
        );
        manager
            .supervisor_fallback
            .lock()
            .expect("terminal supervisor fallback lock poisoned")
            .as_mut()
            .expect("fallback should exist")
            .decided_at = Instant::now() - SUPERVISOR_PROTOCOL_FALLBACK_INTERVAL;
        assert_eq!(manager.fallback_message(), None);
    }

    #[cfg(unix)]
    #[test]
    fn supervisor_reconnect_policy_caps_backoff_and_classifies_failures() {
        assert_eq!(supervisor_reconnect_delay(1), Duration::from_millis(250));
        assert_eq!(supervisor_reconnect_delay(2), Duration::from_millis(500));
        assert_eq!(supervisor_reconnect_delay(3), Duration::from_secs(1));
        assert_eq!(supervisor_reconnect_delay(4), Duration::from_secs(2));
        assert_eq!(supervisor_reconnect_delay(5), Duration::from_secs(5));
        assert_eq!(supervisor_reconnect_delay(20), Duration::from_secs(5));
        assert_eq!(
            reconnect_attempt_after_connection(
                4,
                SUPERVISOR_RECONNECT_STABLE_INTERVAL - Duration::from_millis(1),
            ),
            4
        );
        assert_eq!(
            reconnect_attempt_after_connection(4, SUPERVISOR_RECONNECT_STABLE_INTERVAL),
            0
        );
        assert_eq!(
            classify_supervisor_reconnect_error("connection closed"),
            SupervisorReconnectDecision::Retry
        );
        assert_eq!(
            classify_supervisor_reconnect_error("terminal session `terminal-ended` was not found"),
            SupervisorReconnectDecision::SessionEnded
        );
        assert_eq!(
            classify_supervisor_reconnect_error(
                "failed to connect to supervisor: protocol 0 does not match 1"
            ),
            SupervisorReconnectDecision::ProtocolMismatch
        );
    }

    #[cfg(unix)]
    #[test]
    fn supervisor_staleness_is_visible_and_not_retryable() {
        let stale = is_supervisor_stale(
            Some(halv_supervisor::SUPERVISOR_FINGERPRINT),
            "0.4.15",
            "0.4.15",
        );
        assert!(!stale);
        assert_eq!(
            supervisor_version_mismatch_message(stale, "0.4.15", "0.4.15"),
            None
        );

        let stale = is_supervisor_stale(Some("different-fingerprint"), "0.4.15", "0.4.15");
        assert!(stale);
        let message = supervisor_version_mismatch_message(stale, "0.4.15", "0.4.15")
            .expect("different fingerprints should produce a status message");
        assert_eq!(message, SUPERVISOR_BUILD_MISMATCH_MESSAGE);
        assert!(is_supervisor_version_mismatch(&message));
        assert_eq!(
            classify_supervisor_reconnect_error(&message),
            SupervisorReconnectDecision::VersionMismatch
        );

        let stale = is_supervisor_stale(None, "0.4.14", "0.4.15");
        assert!(stale);
        let message = supervisor_version_mismatch_message(stale, "0.4.14", "0.4.15")
            .expect("legacy version mismatch should produce a status message");
        assert_eq!(
            message,
            "Supervisor is outdated (0.4.14). Restart it to use Halv 0.4.15."
        );
        assert!(is_supervisor_version_mismatch(&message));
        assert_eq!(
            classify_supervisor_reconnect_error(&message),
            SupervisorReconnectDecision::VersionMismatch
        );
        assert!(!is_supervisor_stale(None, "0.4.15", "0.4.15"));
    }

    #[test]
    fn spawned_terminals_drop_inherited_claude_code_session_markers() {
        // Serialized test runner: set_var cannot race another test.
        unsafe { std::env::set_var("CLAUDE_CODE_CHILD_SESSION", "1") };
        let resolved = build_terminal_command(Path::new("/tmp"), None);
        let command = portable_pty_command(&resolved, Path::new("/tmp"));
        unsafe { std::env::remove_var("CLAUDE_CODE_CHILD_SESSION") };

        for name in CLAUDE_CODE_SESSION_MARKERS {
            assert_eq!(command.get_env(name), None, "{name} should be scrubbed");
        }
        assert!(command.get_env("HALV_TERMINAL").is_some());
    }

    #[test]
    fn spawned_terminals_drop_orca_variables_and_shells_drop_pane_variables() {
        // Serialized test runner: set_var cannot race another test.
        unsafe {
            std::env::set_var("ORCA_PANE_KEY", "tab:leaf");
            std::env::set_var("ORCA_TEST_ONLY_MARK", "1");
            std::env::set_var("HALV_AGENT_SESSION_ID", "outer-pane");
        }
        let shell = portable_pty_command(
            &build_terminal_command(Path::new("/tmp"), None),
            Path::new("/tmp"),
        );
        let agent = portable_pty_command(
            &build_terminal_command(
                Path::new("/tmp"),
                Some(TerminalProgram {
                    program: PathBuf::from("/bin/echo"),
                    args: Vec::new(),
                    env: vec![(
                        OsString::from("HALV_AGENT_SESSION_ID"),
                        OsString::from("agent-pane"),
                    )],
                    env_remove: Vec::new(),
                }),
            ),
            Path::new("/tmp"),
        );
        unsafe {
            std::env::remove_var("ORCA_PANE_KEY");
            std::env::remove_var("ORCA_TEST_ONLY_MARK");
            std::env::remove_var("HALV_AGENT_SESSION_ID");
        }

        for command in [&shell, &agent] {
            assert_eq!(command.get_env("ORCA_PANE_KEY"), None);
            assert_eq!(command.get_env("ORCA_TEST_ONLY_MARK"), None);
        }
        assert_eq!(shell.get_env("HALV_AGENT_SESSION_ID"), None);
        assert_eq!(
            agent.get_env("HALV_AGENT_SESSION_ID"),
            Some(std::ffi::OsStr::new("agent-pane"))
        );
    }

    #[test]
    fn terminal_size_is_bounded() {
        assert_eq!(terminal_size(1, 1).cols, 20);
        assert_eq!(terminal_size(1, 1).rows, 5);
        assert_eq!(terminal_size(u16::MAX, u16::MAX).cols, 500);
        assert_eq!(terminal_size(u16::MAX, u16::MAX).rows, 300);
    }

    #[test]
    fn terminal_session_metadata_round_trips_through_json() {
        let session = TerminalSession {
            id: "terminal-meta".to_string(),
            title: Some("Release checks".to_string()),
            project_id: Some(7),
            conversation_id: Some("conversation-1".to_string()),
            project_name: "Halv".to_string(),
            cwd: "/tmp/halv".to_string(),
            shell: "/bin/sh".to_string(),
            pid: 123,
            execution_profile: "full_access".to_string(),
            agent_kind: Some("claude".to_string()),
            status: "running".to_string(),
            cols: 100,
            rows: 30,
            started_at: "2026-08-24T12:00:00Z".to_string(),
            exited_at: None,
            exit_code: None,
            signal: None,
            continued_from: Some("agent-session".to_string()),
            shell_tab_id: None,
        };

        let value = serde_json::to_value(&session).expect("terminal metadata should encode");
        assert_eq!(value["continuedFrom"], "agent-session");
        let decoded = serde_json::from_value::<TerminalSession>(value)
            .expect("terminal metadata should decode");
        assert_eq!(decoded, session);
    }

    const SHELL_TAB: &str = "0f8fad5b-d9cb-469f-a165-70867728950e";

    fn shell_tab_record(id: &str) -> TerminalSession {
        TerminalSession {
            conversation_id: None,
            shell_tab_id: Some(SHELL_TAB.to_string()),
            ..test_terminal_record(id, None, "running", None)
        }
    }

    #[test]
    fn the_shell_tab_owner_round_trips_and_old_metadata_has_none() {
        let session = shell_tab_record("terminal-owned");
        let value = serde_json::to_value(&session).expect("terminal metadata should encode");
        assert_eq!(value["shellTabId"], SHELL_TAB);
        assert!(value.get("conversationId").is_none());
        let decoded = serde_json::from_value::<TerminalSession>(value)
            .expect("terminal metadata should decode");
        assert_eq!(decoded, session);

        // Metadata of an older build has no owner field.
        let mut value =
            serde_json::to_value(test_terminal_record("terminal-old", None, "running", None))
                .expect("terminal metadata should encode");
        assert!(value.get("shellTabId").is_none());
        value
            .as_object_mut()
            .expect("terminal metadata should be an object")
            .remove("shellTabId");
        let decoded = serde_json::from_value::<TerminalSession>(value)
            .expect("old terminal metadata should decode");
        assert_eq!(decoded.shell_tab_id, None);
    }

    #[test]
    fn terminal_open_rejects_two_owners_and_an_unknown_shell_tab() {
        let known = |key: &str| -> TerminalResult<Option<String>> {
            Ok((key == format!("shell:{SHELL_TAB}")).then(|| "primary-1".to_string()))
        };
        assert!(check_terminal_owner(None, None, None, known).is_ok());
        assert!(check_terminal_owner(Some("conversation-1"), None, None, known).is_ok());
        assert!(check_terminal_owner(None, Some(SHELL_TAB), None, known).is_ok());
        assert!(check_terminal_owner(None, Some(SHELL_TAB), Some("primary-1"), known).is_ok());
        assert!(
            check_terminal_owner(Some("conversation-1"), Some(SHELL_TAB), None, known)
                .unwrap_err()
                .contains("not both")
        );
        assert!(check_terminal_owner(None, Some("missing"), None, known)
            .unwrap_err()
            .ends_with("was not found"));
        assert!(
            check_terminal_owner(None, Some(SHELL_TAB), Some("worktree-1"), known)
                .unwrap_err()
                .contains("another workspace")
        );
        // A storage error stops the open.
        assert!(check_terminal_owner(None, Some(SHELL_TAB), None, |_| Err(
            "storage is locked".to_string()
        ))
        .is_err());
    }

    #[test]
    fn open_in_tab_moves_only_a_running_terminal_without_an_owner() {
        let known = |key: &str| -> TerminalResult<Option<String>> {
            Ok((key == format!("shell:{SHELL_TAB}")).then(|| "primary-1".to_string()))
        };
        let shell = TerminalSession {
            conversation_id: None,
            ..test_terminal_record("loose-shell", None, "running", None)
        };
        assert_eq!(
            check_shell_adoption(&shell, &format!(" {SHELL_TAB} "), known).as_deref(),
            Ok(SHELL_TAB)
        );
        // The same move again is accepted.
        let owned = TerminalSession {
            shell_tab_id: Some(SHELL_TAB.to_string()),
            ..shell.clone()
        };
        assert!(check_shell_adoption(&owned, SHELL_TAB, known).is_ok());
        assert!(check_shell_adoption(
            &TerminalSession {
                shell_tab_id: Some("other-tab".to_string()),
                ..shell.clone()
            },
            SHELL_TAB,
            known
        )
        .unwrap_err()
        .contains("another shell tab"));
        assert!(check_shell_adoption(
            &test_terminal_record("chat", None, "running", None),
            SHELL_TAB,
            known
        )
        .unwrap_err()
        .contains("conversation"));
        // A standalone agent terminal moves too (ADR 0020).
        let agent = TerminalSession {
            conversation_id: None,
            ..test_terminal_record("agent", Some("claude"), "running", None)
        };
        assert_eq!(
            check_shell_adoption(&agent, SHELL_TAB, known).as_deref(),
            Ok(SHELL_TAB)
        );
        let exited = TerminalSession {
            conversation_id: None,
            ..test_terminal_record("ended", None, "exited", Some(0))
        };
        assert!(check_shell_adoption(&exited, SHELL_TAB, known)
            .unwrap_err()
            .contains("running"));
        assert!(check_shell_adoption(&shell, "missing", known)
            .unwrap_err()
            .ends_with("was not found"));
        assert!(check_shell_adoption(&shell, "  ", known).is_err());
    }

    #[test]
    fn a_shell_tab_scope_names_its_owner() {
        assert_eq!(
            shell_tab_id_from_scope(&format!("halv.terminal-layout.v1:shell:{SHELL_TAB}")),
            Some(SHELL_TAB.to_string())
        );
        assert_eq!(
            shell_tab_id_from_scope(&format!("shell:{SHELL_TAB}")),
            Some(SHELL_TAB.to_string())
        );
        assert_eq!(
            shell_tab_id_from_scope("halv.terminal-layout.v1:conversation:c1"),
            None
        );
        assert_eq!(
            shell_tab_id_from_scope("halv.terminal-layout.v1:shell:"),
            None
        );
        assert_eq!(shell_tab_id_from_scope("panel:global"), None);
    }

    #[test]
    fn new_shells_copy_the_owner_of_their_source() {
        // A split and a continuation copy the owner of the live session.
        let owned = shell_tab_record("terminal-owned");
        let request = plain_shell_request(ShellSource::from_session(&owned), 100, 24);
        assert_eq!(request.shell_tab_id.as_deref(), Some(SHELL_TAB));
        assert_eq!(request.conversation_id, None);
        let conversation = test_terminal_record("agent", Some("claude"), "exited", Some(0));
        let request = plain_shell_request(ShellSource::from_session(&conversation), 100, 24);
        assert_eq!(request.shell_tab_id, None);

        // A saved pane takes the owner from the scope of its shell tab. A
        // conversation pane never gets a shell tab owner.
        let pane = |conversation_id: Option<&str>| crate::terminal_state::TerminalPaneState {
            pane_id: "pane".to_string(),
            session_id: "saved".to_string(),
            project_id: Some(1),
            conversation_id: conversation_id.map(ToString::to_string),
            project_name: "Halv".to_string(),
            cwd: "/tmp".to_string(),
            execution_profile: "full_access".to_string(),
            agent_kind: None,
            lane_kind: None,
            credential_profile_id: None,
            custom_title: None,
            default_title: None,
            generated_title: None,
            ai_vault_title: None,
            generation: None,
            cli_session_id: None,
            cols: 100,
            rows: 24,
            scrollback: String::new(),
            updated_at: "2026-10-15T10:00:00Z".to_string(),
        };
        let scope = format!("halv.terminal-layout.v1:shell:{SHELL_TAB}");
        assert_eq!(
            ShellSource::from_saved_pane(&pane(None), &scope)
                .shell_tab_id
                .as_deref(),
            Some(SHELL_TAB)
        );
        assert_eq!(
            ShellSource::from_saved_pane(&pane(Some("c1")), &scope).shell_tab_id,
            None
        );
        assert_eq!(
            ShellSource::from_saved_pane(&pane(None), "halv.terminal-layout.v1:panel:global")
                .shell_tab_id,
            None
        );
    }

    #[test]
    fn the_busy_probe_reads_the_foreground_group_of_each_shell() {
        let groups = parse_process_groups(
            "  501   501   777\n  502   502   502\n  503   503     0\n  504   504    -1\nnoise\n",
        );
        assert_eq!(groups.len(), 4);
        assert_eq!(
            foreground_state(groups.get(&501).copied()),
            ForegroundState::Busy
        );
        assert_eq!(
            foreground_state(groups.get(&502).copied()),
            ForegroundState::Idle
        );
        // No controlling terminal, or no row: the prompt counts it as busy.
        assert_eq!(
            foreground_state(groups.get(&503).copied()),
            ForegroundState::Unknown
        );
        assert_eq!(
            foreground_state(groups.get(&504).copied()),
            ForegroundState::Unknown
        );
        assert_eq!(foreground_state(None), ForegroundState::Unknown);
        assert_eq!(
            serde_json::to_value(TerminalForegroundBusy {
                session_id: "terminal-1".to_string(),
                state: ForegroundState::Busy,
            })
            .expect("the probe result should encode"),
            serde_json::json!({ "sessionId": "terminal-1", "state": "busy" })
        );
    }

    #[test]
    fn the_busy_probe_answers_for_every_requested_session() {
        let manager = TerminalManager::default();
        let states = foreground_busy(&manager, &["terminal-missing".to_string()]);
        assert_eq!(
            states,
            vec![TerminalForegroundBusy {
                session_id: "terminal-missing".to_string(),
                state: ForegroundState::Unknown,
            }]
        );
    }

    #[cfg(unix)]
    #[test]
    fn the_busy_probe_reads_a_live_process() {
        let pid = std::process::id();
        let groups = probe_process_groups(&[pid]);
        let (pgid, _) = groups
            .get(&pid)
            .copied()
            .expect("ps should list the test process");
        assert!(pgid > 0);
    }

    fn test_terminal_record(
        id: &str,
        agent_kind: Option<&str>,
        status: &str,
        exit_code: Option<i32>,
    ) -> TerminalSession {
        TerminalSession {
            id: id.to_string(),
            title: None,
            project_id: Some(7),
            conversation_id: Some("conversation-continue".to_string()),
            project_name: "Halv".to_string(),
            cwd: "/tmp".to_string(),
            shell: "/usr/local/bin/claude".to_string(),
            pid: 42,
            execution_profile: "full_access".to_string(),
            agent_kind: agent_kind.map(ToString::to_string),
            status: status.to_string(),
            cols: 120,
            rows: 40,
            started_at: "2026-10-14T12:00:00Z".to_string(),
            exited_at: None,
            exit_code,
            signal: None,
            continued_from: None,
            shell_tab_id: None,
        }
    }

    #[cfg(unix)]
    fn insert_test_terminal(
        manager: &TerminalManager,
        record: TerminalSession,
        scrollback: &str,
    ) -> Arc<ManagedTerminal> {
        let mut output = ScrollbackRing::new(MAX_SCROLLBACK_BYTES);
        output.append(scrollback.as_bytes());
        let session = Arc::new(ManagedTerminal {
            record: Mutex::new(record.clone()),
            output: Mutex::new(output),
            closing: AtomicBool::new(false),
            stop_requested: AtomicBool::new(false),
            prelude: None,
            continuation: Mutex::new(None),
            backend: Backend::Supervised,
        });
        manager
            .sessions
            .lock()
            .expect("terminal sessions lock poisoned")
            .insert(record.id, session.clone());
        session
    }

    #[test]
    fn terminal_session_metadata_without_a_continuation_still_parses() {
        // Supervisor metadata from an older build has no continuation field.
        let mut value =
            serde_json::to_value(test_terminal_record("terminal-old", None, "running", None))
                .expect("terminal metadata should encode");
        assert!(value.get("continuedFrom").is_none());
        value
            .as_object_mut()
            .expect("terminal metadata should be an object")
            .remove("continuedFrom");
        let decoded = serde_json::from_value::<TerminalSession>(value)
            .expect("old terminal metadata should decode");
        assert_eq!(decoded.continued_from, None);
    }

    #[test]
    fn clean_self_exit_requires_success_without_a_signal_or_a_halv_stop() {
        let ended = |status: &str, exit_code: Option<i32>, signal: Option<&str>| TerminalSession {
            signal: signal.map(ToString::to_string),
            ..test_terminal_record("agent", Some("claude"), status, exit_code)
        };
        assert!(clean_self_exit(&ended("exited", Some(0), None), false));
        assert!(!clean_self_exit(&ended("exited", Some(0), None), true));
        assert!(!clean_self_exit(&ended("exited", Some(1), None), false));
        assert!(!clean_self_exit(
            &ended("exited", Some(0), Some("Hangup")),
            false
        ));
        assert!(!clean_self_exit(
            &ended("exited", None, Some("Killed")),
            false
        ));
        assert!(!clean_self_exit(&ended("failed", Some(0), None), false));
        assert!(!clean_self_exit(&ended("exited", None, None), false));
        assert!(!clean_self_exit(&ended("running", None, None), false));
    }

    #[test]
    fn plain_shell_request_copies_the_source_and_drops_the_agent_environment() {
        let source = test_terminal_record("agent-source", Some("claude"), "exited", Some(0));
        let request = plain_shell_request(ShellSource::from_session(&source), 100, 24);
        assert_eq!(request.id, None);
        assert!(request.program.is_none());
        assert_eq!(request.agent_kind, None);
        assert_eq!(request.cwd, PathBuf::from(&source.cwd));
        assert_eq!(request.project_id, source.project_id);
        assert_eq!(request.conversation_id, source.conversation_id);
        assert_eq!(request.project_name, source.project_name);
        assert_eq!(request.execution_profile, "full_access");
        assert_eq!((request.cols, request.rows), (100, 24));

        // No program means the login shell with the plain-shell variables
        // only. Agent keys, session routes, and ledgers never cross.
        let command = build_terminal_command(&request.cwd, request.program);
        assert_eq!(command.program, terminal_shell());
        let names = command
            .env
            .iter()
            .map(|(name, _)| name.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        for agent_variable in [
            "HEADROOM_PROJECT",
            "ANTHROPIC_BASE_URL",
            "ANTHROPIC_API_KEY",
            "ANTHROPIC_AUTH_TOKEN",
            "ANTHROPIC_CUSTOM_HEADERS",
            "API_TIMEOUT_MS",
            "RTK_DB_PATH",
            "RTK_TELEMETRY_DISABLED",
            "CRUX_SAVINGS_LEDGER",
        ] {
            assert!(
                !names.iter().any(|name| name == agent_variable),
                "{agent_variable} must not reach a plain shell"
            );
        }
        assert!(names.iter().any(|name| name == "HALV_TERMINAL"));
        // A continuation shell is not an agent pane. The agent hook
        // variables of the app never reach it.
        for pane_variable in crate::agent_status::pane_variable_names() {
            assert!(command.env_remove.contains(&pane_variable));
            assert!(!command.env.iter().any(|(name, _)| name == &pane_variable));
        }
    }

    #[cfg(unix)]
    #[test]
    fn continue_with_shell_runs_once_with_the_last_agent_screen() {
        let manager = TerminalManager::default();
        let source = test_terminal_record("agent-ended", Some("claude"), "exited", Some(0));
        insert_test_terminal(&manager, source.clone(), "last agent screen");

        let (shell, created) = manager
            .continue_with_shell_with("agent-ended", |request, continuation| {
                assert!(request.program.is_none());
                assert_eq!(request.agent_kind, None);
                assert_eq!(request.cwd, PathBuf::from(&source.cwd));
                assert_eq!(request.conversation_id, source.conversation_id);
                assert_eq!((request.cols, request.rows), (source.cols, source.rows));
                assert_eq!(continuation.continued_from, "agent-ended");
                assert_eq!(
                    continuation.prelude,
                    TerminalPrelude {
                        scrollback: "last agent screen".to_string(),
                        agent_kind: Some("claude".to_string()),
                        exit_code: Some(0),
                    }
                );
                let shell = TerminalSession {
                    continued_from: Some(continuation.continued_from),
                    ..test_terminal_record("terminal-shell", None, "running", None)
                };
                insert_test_terminal(&manager, shell.clone(), "");
                Ok(shell)
            })
            .expect("an ended agent should continue in a shell");
        assert!(created);
        assert_eq!(shell.continued_from.as_deref(), Some("agent-ended"));

        let (again, created_again) = manager
            .continue_with_shell_with("agent-ended", |_, _| {
                panic!("a second request must reuse the first shell")
            })
            .expect("a second request should return the first shell");
        assert!(!created_again);
        assert_eq!(again.id, shell.id);
    }

    #[cfg(unix)]
    #[test]
    fn continue_with_shell_refuses_live_plain_and_stopping_sources() {
        let manager = TerminalManager::default();
        insert_test_terminal(
            &manager,
            test_terminal_record("agent-live", Some("codex"), "running", None),
            "",
        );
        insert_test_terminal(
            &manager,
            test_terminal_record("terminal-plain", None, "exited", Some(0)),
            "",
        );
        insert_test_terminal(
            &manager,
            test_terminal_record("agent-ended", Some("codex"), "exited", Some(0)),
            "",
        );
        let never =
            |_: TerminalSpawnRequest, _: TerminalContinuation| -> TerminalResult<TerminalSession> {
                panic!("a refused source must not start a shell")
            };

        assert_eq!(
            manager
                .continue_with_shell_with("agent-live", never)
                .unwrap_err(),
            "the terminal is still running"
        );
        assert_eq!(
            manager
                .continue_with_shell_with("terminal-plain", never)
                .unwrap_err(),
            "only agent terminals continue in a shell"
        );
        assert!(manager
            .continue_with_shell_with("terminal-missing", never)
            .unwrap_err()
            .ends_with("was not found"));
        manager.stopping.store(true, Ordering::SeqCst);
        assert_eq!(
            manager
                .continue_with_shell_with("agent-ended", never)
                .unwrap_err(),
            "terminals are stopping"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_quit_never_starts_a_supervisor() {
        let manager = TerminalManager::default();
        *manager
            .app_data_dir
            .lock()
            .expect("terminal app data lock poisoned") =
            Some(PathBuf::from("/nonexistent/halv-quit-test"));
        manager.stopping.store(true, Ordering::SeqCst);

        let result = manager.with_supervisor("quit-test", |_| -> TerminalResult<()> {
            panic!("a quit must not reach a supervisor")
        });
        assert_eq!(result.unwrap_err(), TERMINALS_STOPPING_ERROR);
        assert!(manager
            .supervisor
            .lock()
            .expect("terminal supervisor lock poisoned")
            .is_none());
    }

    #[cfg(unix)]
    #[test]
    fn bulk_kill_marks_running_sessions_as_stopped_by_halv() {
        let manager = TerminalManager::default();
        let live = insert_test_terminal(
            &manager,
            test_terminal_record("agent-live", Some("claude"), "running", None),
            "",
        );
        let ended = insert_test_terminal(
            &manager,
            test_terminal_record("agent-ended", Some("claude"), "exited", Some(0)),
            "",
        );
        assert!(!manager.halv_stopped("agent-live"));

        let marked = manager.mark_supervised_sessions_stopped();
        assert_eq!(marked.len(), 1);
        assert!(Arc::ptr_eq(&marked[0], &live));
        assert!(live.stop_requested.load(Ordering::SeqCst));
        // The mark leaves the attach thread running, so the exit still lands.
        assert!(!live.closing.load(Ordering::SeqCst));
        assert!(!ended.stop_requested.load(Ordering::SeqCst));
        assert!(manager.halv_stopped("agent-live"));
        assert!(!manager.halv_stopped("agent-ended"));

        ended.closing.store(true, Ordering::SeqCst);
        assert!(manager.halv_stopped("agent-ended"));
        assert!(!manager.halv_stopped("agent-missing"));
        manager.stopping.store(true, Ordering::SeqCst);
        assert!(manager.halv_stopped("agent-missing"));
    }

    #[cfg(unix)]
    #[test]
    fn failed_stop_all_leaves_no_quit_state_and_no_marks_without_a_kill() {
        // A manager without an app data folder cannot reach a supervisor, so
        // the stop fails before any kill.
        let manager = TerminalManager::default();
        let live = insert_test_terminal(
            &manager,
            test_terminal_record("agent-live", Some("claude"), "running", None),
            "",
        );

        assert!(manager.stop_all_supervised().is_err());
        assert!(!live.stop_requested.load(Ordering::SeqCst));
        assert!(!manager.halv_stopped("agent-live"));
        // The app stays open, so a later clean exit can continue in a shell.
        assert!(!manager.stopping.load(Ordering::SeqCst));
        assert!(!manager.halv_stopped("agent-other"));
    }

    #[cfg(unix)]
    #[test]
    fn failed_bulk_kill_clears_the_marks_of_sessions_that_still_run() {
        let manager = TerminalManager::default();
        let survivor = insert_test_terminal(
            &manager,
            test_terminal_record("agent-survivor", Some("claude"), "running", None),
            "",
        );
        let killed = insert_test_terminal(
            &manager,
            test_terminal_record("agent-killed", Some("claude"), "running", None),
            "",
        );
        let unlisted = insert_test_terminal(
            &manager,
            test_terminal_record("agent-unlisted", Some("codex"), "running", None),
            "",
        );
        let marked = manager.mark_supervised_sessions_stopped();
        assert_eq!(marked.len(), 3);
        let summary = |id: &str, status: SessionStatus| SessionSummary {
            id: id.to_string(),
            pid: 42,
            cwd: "/tmp".to_string(),
            program: "/bin/sh".to_string(),
            status,
            cols: 120,
            rows: 40,
            started_at: "2026-10-14T12:00:00Z".to_string(),
            exited_at: None,
            exit_code: None,
            signal: None,
            meta: serde_json::Value::Null,
        };

        clear_stop_marks_of_running_sessions(
            &marked,
            &[
                summary("agent-survivor", SessionStatus::Running),
                summary("agent-killed", SessionStatus::Exited),
            ],
        );
        // The kill did not reach the survivor, so its later clean exit
        // continues in a shell again.
        assert!(!survivor.stop_requested.load(Ordering::SeqCst));
        assert!(!manager.halv_stopped("agent-survivor"));
        assert!(killed.stop_requested.load(Ordering::SeqCst));
        assert!(unlisted.stop_requested.load(Ordering::SeqCst));
    }

    #[test]
    fn open_shell_source_reads_a_saved_pane_and_rejects_unknown_ids() {
        let manager = TerminalManager::default();
        manager
            .persistence
            .lock()
            .expect("terminal persistence lock poisoned")
            .save_panel(
                "conversation:saved".to_string(),
                serde_json::json!({ "version": 1, "tabs": [] }),
                vec![TerminalPaneStateInput {
                    pane_id: "pane-saved".to_string(),
                    session_id: "agent-saved".to_string(),
                    project_id: Some(3),
                    conversation_id: Some("conversation-saved".to_string()),
                    project_name: "Saved project".to_string(),
                    cwd: "/tmp/saved".to_string(),
                    execution_profile: "full_access".to_string(),
                    agent_kind: Some("codex".to_string()),
                    lane_kind: Some("codexChatGpt".to_string()),
                    credential_profile_id: None,
                    custom_title: None,
                    default_title: None,
                    generated_title: None,
                    ai_vault_title: None,
                    generation: Some(1),
                    cli_session_id: Some("codex-session".to_string()),
                    cols: 100,
                    rows: 24,
                }],
                &HashMap::new(),
            )
            .expect("saved panel should store");

        assert_eq!(
            shell_source_for(&manager, "agent-saved", Some("conversation:saved"))
                .expect("a saved pane should be a shell source"),
            ShellSource {
                project_id: Some(3),
                conversation_id: Some("conversation-saved".to_string()),
                project_name: "Saved project".to_string(),
                cwd: PathBuf::from("/tmp/saved"),
                shell_tab_id: None,
            }
        );
        assert!(
            shell_source_for(&manager, "agent-saved", Some("other-scope"))
                .unwrap_err()
                .ends_with("was not found")
        );
        assert!(shell_source_for(&manager, "agent-saved", None).is_err());
        assert!(
            shell_source_for(&manager, "terminal-missing", Some("conversation:saved")).is_err()
        );
    }

    #[cfg(unix)]
    #[test]
    fn open_shell_source_prefers_the_live_session() {
        let manager = TerminalManager::default();
        let live = test_terminal_record("agent-live", Some("claude"), "running", None);
        insert_test_terminal(&manager, live.clone(), "");
        assert_eq!(
            shell_source_for(&manager, "agent-live", None)
                .expect("a live agent should be a shell source"),
            ShellSource::from_session(&live)
        );
    }

    // Halv 0.7.1 stored these metadata objects in the supervisor. A newer app
    // adopts their sessions after an update, so each object must decode.
    #[test]
    fn terminal_session_metadata_from_0_7_1_still_decodes() {
        let opened = r#"{"id":"0f6b2f9e-7c1d-4a59-9a43-5d2c0b8e1f20","projectId":3,"conversationId":"conversation-1","projectName":"halv","cwd":"/Users/demo/programming/halv/app","shell":"/opt/homebrew/bin/claude","pid":0,"executionProfile":"full_access","agentKind":"claude","status":"running","cols":120,"rows":32,"startedAt":"2026-10-08T19:55:52.639123+00:00"}"#;
        let renamed = r#"{"id":"a5d3c9b1-2e4f-4c7a-8b6d-1f0e9c8b7a65","title":"Release checks","projectId":null,"projectName":"Home","cwd":"/Users/demo","shell":"/bin/zsh","pid":4242,"executionProfile":"full_access","status":"running","cols":100,"rows":30,"startedAt":"2026-10-08T20:01:02Z"}"#;
        let exited = r#"{"id":"c1d2e3f4-a5b6-4c7d-8e9f-0a1b2c3d4e5f","projectId":7,"projectName":"halv","cwd":"/tmp","shell":"/usr/local/bin/codex","pid":51515,"executionProfile":"full_access","agentKind":"codex","status":"exited","cols":80,"rows":24,"startedAt":"2026-10-08T20:01:02Z","exitedAt":"2026-10-08T21:00:00Z","exitCode":0,"signal":"SIGHUP"}"#;

        let opened = serde_json::from_str::<TerminalSession>(opened)
            .expect("0.7.1 open metadata should decode");
        assert_eq!(opened.agent_kind.as_deref(), Some("claude"));
        assert_eq!(opened.conversation_id.as_deref(), Some("conversation-1"));
        assert_eq!(opened.title, None);
        assert_eq!(opened.continued_from, None);
        assert_eq!(opened.shell_tab_id, None);

        let renamed = serde_json::from_str::<TerminalSession>(renamed)
            .expect("0.7.1 renamed metadata should decode");
        assert_eq!(renamed.title.as_deref(), Some("Release checks"));
        assert_eq!(renamed.project_id, None);
        assert_eq!(renamed.agent_kind, None);

        let exited = serde_json::from_str::<TerminalSession>(exited)
            .expect("0.7.1 exited metadata should decode");
        assert_eq!(exited.exit_code, Some(0));
        assert_eq!(exited.signal.as_deref(), Some("SIGHUP"));
    }

    #[test]
    fn command_builder_accepts_an_injected_terminal_program() {
        let cwd = PathBuf::from("/tmp");
        let resolved = build_terminal_command(
            &cwd,
            Some(TerminalProgram {
                program: PathBuf::from("/usr/bin/env"),
                args: vec![OsString::from("printf"), OsString::from("ok")],
                env: vec![(OsString::from("HALV_TEST_TOKEN"), OsString::from("value"))],
                env_remove: vec![OsString::from("HALV_OLD_TOKEN")],
            }),
        );
        assert_eq!(resolved.program, PathBuf::from("/usr/bin/env"));
        let command = portable_pty_command(&resolved, &cwd);
        assert_eq!(
            command.get_argv(),
            &vec![
                OsString::from("/usr/bin/env"),
                OsString::from("printf"),
                OsString::from("ok"),
            ]
        );
        assert_eq!(command.get_cwd(), Some(&cwd.into_os_string()));
        assert_eq!(
            command.get_env("HALV_TEST_TOKEN"),
            Some(std::ffi::OsStr::new("value"))
        );
        assert_eq!(
            command.get_env("TERM"),
            Some(std::ffi::OsStr::new("xterm-256color"))
        );
        let locale_is_unset = ["LC_ALL", "LC_CTYPE", "LANG"].iter().all(|name| {
            std::env::var_os(name)
                .map(|value| value.is_empty())
                .unwrap_or(true)
        });
        if locale_is_unset && !cfg!(target_os = "windows") {
            assert!(command
                .get_env("LANG")
                .expect("terminal LANG should exist")
                .to_string_lossy()
                .ends_with("UTF-8"));
        }
        if let Some(parent_lang) = std::env::var_os("LANG").filter(|value| !value.is_empty()) {
            assert_eq!(command.get_env("LANG"), Some(parent_lang.as_os_str()));
        }
    }

    #[cfg(unix)]
    mod supervisor_adoption_tests {
        use super::*;

        struct TestSupervisorEvents {
            inner: Box<dyn Iterator<Item = TerminalResult<SupervisorEvent>> + Send>,
        }

        impl TestSupervisorEvents {
            fn new(
                events: impl Iterator<Item = TerminalResult<SupervisorEvent>> + Send + 'static,
            ) -> Self {
                Self {
                    inner: Box::new(events),
                }
            }
        }

        impl Iterator for TestSupervisorEvents {
            type Item = TerminalResult<SupervisorEvent>;

            fn next(&mut self) -> Option<Self::Item> {
                self.inner.next()
            }
        }

        fn running_supervisor_summary(id: &str) -> SessionSummary {
            let record = TerminalSession {
                id: id.to_string(),
                title: None,
                project_id: Some(9),
                conversation_id: Some("conversation-adoption".to_string()),
                project_name: "Halv".to_string(),
                cwd: "/tmp".to_string(),
                shell: "/bin/sh".to_string(),
                pid: 0,
                execution_profile: "full_access".to_string(),
                agent_kind: Some("codex".to_string()),
                status: "running".to_string(),
                cols: 80,
                rows: 24,
                started_at: Utc::now().to_rfc3339(),
                exited_at: None,
                exit_code: None,
                signal: None,
                continued_from: None,
                shell_tab_id: None,
            };
            SessionSummary {
                id: id.to_string(),
                pid: 42,
                cwd: record.cwd.clone(),
                program: record.shell.clone(),
                status: SessionStatus::Running,
                cols: record.cols,
                rows: record.rows,
                started_at: record.started_at.clone(),
                exited_at: None,
                exit_code: None,
                signal: None,
                meta: serde_json::to_value(record).expect("terminal metadata should encode"),
            }
        }

        fn adopt_test_summary<Attach, Events, Remove>(
            manager: &TerminalManager,
            summary: &SessionSummary,
            attach: Attach,
            remove: Remove,
        ) where
            Attach: FnMut(&str, Option<u64>) -> TerminalResult<(halv_supervisor::Snapshot, Events)>
                + Send
                + 'static,
            Events: Iterator<Item = TerminalResult<SupervisorEvent>> + Send + 'static,
            Remove: FnMut(&str) -> TerminalResult<()>,
        {
            let mut record = serde_json::from_value::<TerminalSession>(summary.meta.clone())
                .expect("terminal metadata should decode");
            apply_supervisor_summary(&mut record, summary);
            manager.adopt_running_supervised_session_with(
                summary,
                record,
                SupervisorAttachSinks {
                    terminal: Arc::new(|_| {}),
                    connection: Arc::new(|_| {}),
                },
                SupervisorAttachHooks {
                    ensure: || Ok(()),
                    attach,
                    wait: |_| {},
                },
                remove,
            );
        }

        fn failed_test_attach(
            error: &'static str,
        ) -> impl FnMut(
            &str,
            Option<u64>,
        ) -> TerminalResult<(halv_supervisor::Snapshot, TestSupervisorEvents)>
               + Send {
            move |_, _| Err(error.to_string())
        }

        fn insert_supervised_test_session(
            manager: &TerminalManager,
            id: &str,
            status: &str,
        ) -> Arc<ManagedTerminal> {
            let summary = running_supervisor_summary(id);
            let mut record = serde_json::from_value::<TerminalSession>(summary.meta)
                .expect("terminal metadata should decode");
            record.status = status.to_string();
            let session = Arc::new(ManagedTerminal {
                record: Mutex::new(record),
                output: Mutex::new(ScrollbackRing::new(MAX_SCROLLBACK_BYTES)),
                closing: AtomicBool::new(false),
                stop_requested: AtomicBool::new(false),
                prelude: None,
                continuation: Mutex::new(None),
                backend: Backend::Supervised,
            });
            manager
                .sessions
                .lock()
                .expect("terminal sessions lock poisoned")
                .insert(id.to_string(), session.clone());
            session
        }

        #[test]
        fn post_update_counts_only_running_supervised_agents() {
            let manager = TerminalManager::default();
            for (id, agent_kind, status) in [
                ("shell-1", None, "running"),
                ("shell-2", None, "running"),
                ("claude-1", Some("claude"), "running"),
                ("codex-1", Some("codex"), "exited"),
            ] {
                let session = insert_supervised_test_session(&manager, id, status);
                session
                    .record
                    .lock()
                    .expect("terminal record lock poisoned")
                    .agent_kind = agent_kind.map(str::to_string);
            }
            assert_eq!(manager.running_supervised_session_count(), 3);
            assert_eq!(manager.running_supervised_agent_count(), 1);
        }

        #[test]
        fn terminal_forget_refuses_running_supervisor_session() {
            let manager = TerminalManager::default();
            let id = "terminal-live";
            insert_supervised_test_session(&manager, id, "running");

            let error = manager
                .forget_supervised_session_with(id, |session_id| {
                    Err(format!("terminal session `{session_id}` still running"))
                })
                .expect_err("a live terminal should remain");

            assert_eq!(error, TERMINAL_FORGET_RUNNING_ERROR);
            assert!(manager.get(id).is_ok());
        }

        #[test]
        fn terminal_forget_succeeds_for_failed_adopted_session() {
            let manager = TerminalManager::default();
            let id = "terminal-failed-adoption";
            insert_supervised_test_session(&manager, id, "running");

            manager
                .forget_supervised_session_with(id, |_| Ok(()))
                .expect("a failed adoption should be removable");

            assert!(manager.get(id).is_err());
        }

        #[test]
        fn terminal_forget_removes_failed_adoption_from_supervisor() {
            let manager = TerminalManager::default();
            let id = "terminal-failed-adoption";
            let mut removed = Vec::new();
            insert_supervised_test_session(&manager, id, "running");

            manager
                .forget_supervised_session_with(id, |session_id| {
                    removed.push(session_id.to_string());
                    Ok(())
                })
                .expect("a failed adoption should be removable");

            assert_eq!(removed, [id]);
        }

        #[test]
        fn terminal_forget_succeeds_for_exited_supervisor_session() {
            let manager = TerminalManager::default();
            let id = "terminal-exited";
            insert_supervised_test_session(&manager, id, "exited");

            manager
                .forget_supervised_session_with(id, |_| Ok(()))
                .expect("an exited terminal should be removable");

            assert!(manager.get(id).is_err());
        }

        #[test]
        fn terminal_forget_tolerates_missing_supervisor_session() {
            let manager = TerminalManager::default();
            let id = "terminal-missing";
            insert_supervised_test_session(&manager, id, "running");

            manager
                .forget_supervised_session_with(id, |session_id| {
                    Err(format!("terminal session `{session_id}` was not found"))
                })
                .expect("a missing supervisor session should be removable");

            assert!(manager.get(id).is_err());
        }

        #[test]
        fn adoption_attach_failure_leaves_no_session() {
            let manager = TerminalManager::default();
            let summary = running_supervisor_summary("terminal-stale");

            adopt_test_summary(
                &manager,
                &summary,
                failed_test_attach("terminal session `terminal-stale` was not found"),
                |_| Ok(()),
            );

            assert!(manager.list().is_empty());
        }

        #[test]
        fn adoption_attach_failure_removes_unattachable_session() {
            let manager = TerminalManager::default();
            let summary = running_supervisor_summary("terminal-stale");
            let mut removed = Vec::new();

            adopt_test_summary(
                &manager,
                &summary,
                failed_test_attach("terminal session `terminal-stale` was not found"),
                |id| {
                    removed.push(id.to_string());
                    Ok(())
                },
            );

            assert_eq!(removed, [summary.id]);
        }

        #[test]
        fn adoption_broken_connection_does_not_remove_session() {
            let manager = TerminalManager::default();
            let summary = running_supervisor_summary("terminal-live");
            let snapshot_summary = summary.clone();
            let (event_sender, event_receiver) =
                std::sync::mpsc::channel::<TerminalResult<SupervisorEvent>>();
            let mut event_receiver = Some(event_receiver);
            let mut attach_count = 0;
            let mut removed = Vec::new();

            adopt_test_summary(
                &manager,
                &summary,
                move |_, from_sequence| {
                    attach_count += 1;
                    if attach_count == 1 {
                        assert_eq!(from_sequence, None);
                        return Err(
                            "failed to read supervisor response: connection closed".to_string()
                        );
                    }
                    assert_eq!(from_sequence, Some(1));
                    Ok((
                        halv_supervisor::Snapshot {
                            session: snapshot_summary.clone(),
                            sequence: 0,
                            scrollback: String::new(),
                        },
                        TestSupervisorEvents::new(
                            event_receiver
                                .take()
                                .expect("reconnect event receiver should exist")
                                .into_iter(),
                        ),
                    ))
                },
                |id| {
                    removed.push(id.to_string());
                    Ok(())
                },
            );

            wait_until("adopted reconnect", Duration::from_secs(2), || {
                manager.get(&summary.id).is_ok()
            });
            assert!(removed.is_empty());
            manager
                .get(&summary.id)
                .expect("reconnected session should exist")
                .closing
                .store(true, Ordering::SeqCst);
            drop(event_sender);
        }

        #[test]
        fn successful_adoption_inserts_session_without_removal() {
            let manager = TerminalManager::default();
            let summary = running_supervisor_summary("terminal-live");
            let snapshot_summary = summary.clone();
            let (_event_sender, event_receiver) =
                std::sync::mpsc::channel::<TerminalResult<SupervisorEvent>>();
            let mut event_receiver = Some(event_receiver);
            let mut removed = Vec::new();

            adopt_test_summary(
                &manager,
                &summary,
                move |_, _| {
                    Ok((
                        halv_supervisor::Snapshot {
                            session: snapshot_summary.clone(),
                            sequence: 7,
                            scrollback: "saved output".to_string(),
                        },
                        TestSupervisorEvents::new(
                            event_receiver
                                .take()
                                .expect("initial event receiver should exist")
                                .into_iter(),
                        ),
                    ))
                },
                |id| {
                    removed.push(id.to_string());
                    Ok(())
                },
            );

            let attached = manager
                .get(&summary.id)
                .expect("adopted session should exist")
                .attach();
            assert_eq!(attached.session.status, "running");
            assert_eq!(attached.sequence, 7);
            assert_eq!(attached.scrollback, "saved output");
            assert!(removed.is_empty());
            manager
                .get(&summary.id)
                .expect("adopted session should exist")
                .closing
                .store(true, Ordering::SeqCst);
        }

        #[test]
        fn adoption_keeps_the_continuation_link_without_the_prelude() {
            let manager = TerminalManager::default();
            let mut summary = running_supervisor_summary("terminal-continued");
            summary.meta["agentKind"] = serde_json::Value::Null;
            summary.meta["continuedFrom"] = serde_json::json!("agent-ended");
            let snapshot_summary = summary.clone();
            let (_event_sender, event_receiver) =
                std::sync::mpsc::channel::<TerminalResult<SupervisorEvent>>();
            let mut event_receiver = Some(event_receiver);

            adopt_test_summary(
                &manager,
                &summary,
                move |_, _| {
                    Ok((
                        halv_supervisor::Snapshot {
                            session: snapshot_summary.clone(),
                            sequence: 3,
                            scrollback: "$ ".to_string(),
                        },
                        TestSupervisorEvents::new(
                            event_receiver
                                .take()
                                .expect("initial event receiver should exist")
                                .into_iter(),
                        ),
                    ))
                },
                |_| Ok(()),
            );

            let adopted = manager
                .get(&summary.id)
                .expect("adopted continuation should exist");
            let attached = adopted.attach_with_prelude();
            assert_eq!(
                attached.session.continued_from.as_deref(),
                Some("agent-ended")
            );
            assert_eq!(attached.session.agent_kind, None);
            // The prelude lives in memory only, so a relaunch drops it.
            assert_eq!(attached.prelude, None);
            adopted.closing.store(true, Ordering::SeqCst);
        }

        #[test]
        fn resumed_attach_snapshot_emits_only_new_output_once() {
            let manager = TerminalManager::default();
            let summary = running_supervisor_summary("terminal-delta");
            let session = insert_supervised_test_session(&manager, &summary.id, "running");
            let terminal_events = Arc::new(Mutex::new(Vec::new()));
            let recorded_terminal_events = terminal_events.clone();
            let sink: TerminalEventSink = Arc::new(move |event| {
                recorded_terminal_events
                    .lock()
                    .expect("terminal events lock poisoned")
                    .push(event);
            });

            apply_supervisor_attach_snapshot(
                &session,
                &manager,
                &sink,
                halv_supervisor::Snapshot {
                    session: summary.clone(),
                    sequence: 1,
                    scrollback: "first".to_string(),
                },
                None,
            );
            assert!(terminal_events
                .lock()
                .expect("terminal events lock poisoned")
                .is_empty());
            let resumed_snapshot = halv_supervisor::Snapshot {
                session: summary,
                sequence: 3,
                scrollback: "secondthird".to_string(),
            };
            apply_supervisor_attach_snapshot(
                &session,
                &manager,
                &sink,
                resumed_snapshot.clone(),
                Some(2),
            );
            apply_supervisor_attach_snapshot(&session, &manager, &sink, resumed_snapshot, Some(2));

            let attached = session.attach();
            assert_eq!(attached.sequence, 3);
            assert_eq!(attached.scrollback, "firstsecondthird");
            let events = terminal_events
                .lock()
                .expect("terminal events lock poisoned");
            assert_eq!(events.len(), 1);
            assert_eq!(events[0].kind, "data");
            assert_eq!(events[0].sequence, 3);
            assert_eq!(events[0].data.as_deref(), Some("secondthird"));
            assert_eq!(events[0].replay, Some(true));
        }

        #[test]
        fn duplicate_supervisor_sequence_changes_neither_scrollback_nor_events() {
            let manager = TerminalManager::default();
            let summary = running_supervisor_summary("terminal-idempotent");
            let session = insert_supervised_test_session(&manager, &summary.id, "running");
            let terminal_events = Arc::new(Mutex::new(Vec::new()));
            let recorded_terminal_events = terminal_events.clone();
            let sink: TerminalEventSink = Arc::new(move |event| {
                recorded_terminal_events
                    .lock()
                    .expect("terminal events lock poisoned")
                    .push(event);
            });

            session.emit_supervised_data(&sink, &manager, 8, "once", false);
            session.emit_supervised_data(&sink, &manager, 8, "duplicate", false);

            let attached = session.attach();
            assert_eq!(attached.sequence, 8);
            assert_eq!(attached.scrollback, "once");
            let events = terminal_events
                .lock()
                .expect("terminal events lock poisoned");
            assert_eq!(events.len(), 1);
            assert_eq!(events[0].sequence, 8);
            assert_eq!(events[0].data.as_deref(), Some("once"));
            assert_eq!(events[0].replay, None);
        }

        #[test]
        fn adopted_session_reconnects_and_resumes_without_output_loss() {
            let manager = TerminalManager::default();
            let summary = running_supervisor_summary("terminal-live");
            let snapshot_summary = summary.clone();
            let (live_sender, live_receiver) =
                std::sync::mpsc::channel::<TerminalResult<SupervisorEvent>>();
            let mut live_receiver = Some(live_receiver);
            let attach_sequences = Arc::new(Mutex::new(Vec::new()));
            let recorded_attach_sequences = attach_sequences.clone();
            let connection_events = Arc::new(Mutex::new(Vec::new()));
            let recorded_connection_events = connection_events.clone();
            let waits = Arc::new(Mutex::new(Vec::new()));
            let recorded_waits = waits.clone();
            let terminal_events = Arc::new(Mutex::new(Vec::new()));
            let recorded_terminal_events = terminal_events.clone();
            let session = insert_supervised_test_session(&manager, &summary.id, "running");
            let mut attach_count = 0;

            spawn_supervisor_attach_with(
                manager.clone(),
                session.clone(),
                SupervisorAttachSinks {
                    terminal: Arc::new(move |event| {
                        recorded_terminal_events
                            .lock()
                            .expect("terminal events lock poisoned")
                            .push(event);
                    }),
                    connection: Arc::new(move |event| {
                        recorded_connection_events
                            .lock()
                            .expect("connection events lock poisoned")
                            .push(event);
                    }),
                },
                false,
                SupervisorAttachHooks {
                    ensure: || Ok(()),
                    attach: move |_id: &str, from_sequence: Option<u64>| {
                        recorded_attach_sequences
                            .lock()
                            .expect("attach sequences lock poisoned")
                            .push(from_sequence);
                        attach_count += 1;
                        if attach_count == 1 {
                            return Ok((
                                halv_supervisor::Snapshot {
                                    session: snapshot_summary.clone(),
                                    sequence: 1,
                                    scrollback: "first".to_string(),
                                },
                                TestSupervisorEvents::new(
                                    vec![Err(
                                        "failed to read supervisor response: connection closed"
                                            .to_string(),
                                    )]
                                    .into_iter(),
                                ),
                            ));
                        }
                        assert_eq!(from_sequence, Some(2));
                        Ok((
                            halv_supervisor::Snapshot {
                                session: snapshot_summary.clone(),
                                sequence: 3,
                                scrollback: "secondthird".to_string(),
                            },
                            TestSupervisorEvents::new(
                                live_receiver
                                    .take()
                                    .expect("live event receiver should exist")
                                    .into_iter(),
                            ),
                        ))
                    },
                    wait: move |delay| {
                        recorded_waits
                            .lock()
                            .expect("waits lock poisoned")
                            .push(delay);
                    },
                },
            )
            .expect("initial attach should succeed");

            wait_until("adopted reconnect", Duration::from_secs(2), || {
                connection_events
                    .lock()
                    .expect("connection events lock poisoned")
                    .iter()
                    .any(|event| matches!(event, SupervisorConnectionEvent::Reconnected(_)))
            });
            let attached = session.attach();
            assert_eq!(attached.session.status, "running");
            assert_eq!(attached.sequence, 3);
            assert_eq!(attached.scrollback, "firstsecondthird");
            assert_eq!(
                *attach_sequences
                    .lock()
                    .expect("attach sequences lock poisoned"),
                [None, Some(2)]
            );
            assert_eq!(
                *waits.lock().expect("waits lock poisoned"),
                [Duration::from_millis(250)]
            );
            assert!(terminal_events
                .lock()
                .expect("terminal events lock poisoned")
                .iter()
                .any(|event| {
                    event.kind == "data"
                        && event.sequence == 3
                        && event.data.as_deref() == Some("secondthird")
                        && event.replay == Some(true)
                }));
            session.closing.store(true, Ordering::SeqCst);
            drop(live_sender);
        }

        fn assert_classified_reconnect_error_stops(
            id: &'static str,
            reconnect_error: &'static str,
            expected_message: &'static str,
        ) {
            let manager = TerminalManager::default();
            let summary = running_supervisor_summary(id);
            let snapshot_summary = summary.clone();
            let session = insert_supervised_test_session(&manager, &summary.id, "running");
            let terminal_events = Arc::new(Mutex::new(Vec::new()));
            let recorded_terminal_events = terminal_events.clone();
            let attach_calls = Arc::new(Mutex::new(0_u32));
            let recorded_attach_calls = attach_calls.clone();
            let waits = Arc::new(Mutex::new(Vec::new()));
            let recorded_waits = waits.clone();

            spawn_supervisor_attach_with(
                manager,
                session.clone(),
                SupervisorAttachSinks {
                    terminal: Arc::new(move |event| {
                        recorded_terminal_events
                            .lock()
                            .expect("terminal events lock poisoned")
                            .push(event);
                    }),
                    connection: Arc::new(|_| {}),
                },
                false,
                SupervisorAttachHooks {
                    ensure: || Ok(()),
                    attach: move |_id: &str, _from_sequence: Option<u64>| {
                        let mut calls = recorded_attach_calls
                            .lock()
                            .expect("attach calls lock poisoned");
                        *calls += 1;
                        if *calls == 1 {
                            return Ok((
                                halv_supervisor::Snapshot {
                                    session: snapshot_summary.clone(),
                                    sequence: 0,
                                    scrollback: String::new(),
                                },
                                TestSupervisorEvents::new(std::iter::empty()),
                            ));
                        }
                        Err(reconnect_error.to_string())
                    },
                    wait: move |delay| {
                        recorded_waits
                            .lock()
                            .expect("waits lock poisoned")
                            .push(delay);
                    },
                },
            )
            .expect("initial attach should succeed");

            wait_until(
                "classified reconnect failure",
                Duration::from_secs(2),
                || {
                    session.snapshot().status == "failed"
                        && terminal_events
                            .lock()
                            .expect("terminal events lock poisoned")
                            .iter()
                            .any(|event| {
                                event.kind == "error"
                                    && event.message.as_deref() == Some(expected_message)
                            })
                },
            );
            assert_eq!(*attach_calls.lock().expect("attach calls lock poisoned"), 2);
            assert_eq!(
                *waits.lock().expect("waits lock poisoned"),
                [Duration::from_millis(250)]
            );
            assert!(terminal_events
                .lock()
                .expect("terminal events lock poisoned")
                .iter()
                .any(|event| {
                    event.kind == "error" && event.message.as_deref() == Some(expected_message)
                }));
        }

        #[test]
        fn classified_reconnect_errors_stop_immediately() {
            assert_classified_reconnect_error_stops(
                "terminal-ended-immediately",
                "terminal session `terminal-ended-immediately` was not found",
                "terminal session ended while the supervisor was unavailable",
            );
            assert_classified_reconnect_error_stops(
                "terminal-protocol-mismatch",
                "failed connect supervisor: protocol 0 does not match 1",
                "failed connect supervisor: protocol 0 does not match 1",
            );
        }

        #[test]
        fn reconnect_not_found_finishes_the_session_as_failed() {
            let manager = TerminalManager::default();
            let summary = running_supervisor_summary("terminal-ended");
            let snapshot_summary = summary.clone();
            let session = insert_supervised_test_session(&manager, &summary.id, "running");
            let terminal_events = Arc::new(Mutex::new(Vec::new()));
            let recorded_terminal_events = terminal_events.clone();
            let mut attach_count = 0;

            spawn_supervisor_attach_with(
                manager.clone(),
                session.clone(),
                SupervisorAttachSinks {
                    terminal: Arc::new(move |event| {
                        recorded_terminal_events
                            .lock()
                            .expect("terminal events lock poisoned")
                            .push(event);
                    }),
                    connection: Arc::new(|_| {}),
                },
                false,
                SupervisorAttachHooks {
                    ensure: || Ok(()),
                    attach: move |_id: &str, _from_sequence: Option<u64>| {
                        attach_count += 1;
                        if attach_count == 1 {
                            return Ok((
                                halv_supervisor::Snapshot {
                                    session: snapshot_summary.clone(),
                                    sequence: 0,
                                    scrollback: String::new(),
                                },
                                TestSupervisorEvents::new(
                                    vec![Err("connection closed".to_string())].into_iter(),
                                ),
                            ));
                        }
                        Err("terminal session `terminal-ended` was not found".to_string())
                    },
                    wait: |_| {},
                },
            )
            .expect("initial attach should succeed");

            wait_until("not-found reconnect", Duration::from_secs(2), || {
                session.snapshot().status == "failed"
                    && terminal_events
                        .lock()
                        .expect("terminal events lock poisoned")
                        .iter()
                        .any(|event| {
                            event.kind == "error"
                                && event.message.as_deref()
                                    == Some(
                                        "terminal session ended while the supervisor was unavailable",
                                    )
                        })
            });
            let events = terminal_events
                .lock()
                .expect("terminal events lock poisoned");
            assert!(events.iter().any(|event| {
                event.kind == "error"
                    && event.message.as_deref()
                        == Some("terminal session ended while the supervisor was unavailable")
            }));
        }

        #[test]
        fn reconnect_stops_silently_when_the_session_closes() {
            let manager = TerminalManager::default();
            let summary = running_supervisor_summary("terminal-closing");
            let snapshot_summary = summary.clone();
            let session = insert_supervised_test_session(&manager, &summary.id, "running");
            let terminal_events = Arc::new(Mutex::new(Vec::new()));
            let recorded_terminal_events = terminal_events.clone();
            let wait_reached = Arc::new(AtomicBool::new(false));
            let recorded_wait_reached = wait_reached.clone();
            let closing_session = session.clone();
            let ensure_calls = Arc::new(Mutex::new(0_u32));
            let recorded_ensure_calls = ensure_calls.clone();

            spawn_supervisor_attach_with(
                manager,
                session.clone(),
                SupervisorAttachSinks {
                    terminal: Arc::new(move |event| {
                        recorded_terminal_events
                            .lock()
                            .expect("terminal events lock poisoned")
                            .push(event);
                    }),
                    connection: Arc::new(|_| {}),
                },
                false,
                SupervisorAttachHooks {
                    ensure: move || {
                        *recorded_ensure_calls
                            .lock()
                            .expect("ensure calls lock poisoned") += 1;
                        Ok(())
                    },
                    attach: move |_id: &str, _from_sequence: Option<u64>| {
                        Ok((
                            halv_supervisor::Snapshot {
                                session: snapshot_summary.clone(),
                                sequence: 0,
                                scrollback: String::new(),
                            },
                            TestSupervisorEvents::new(
                                vec![Err("connection closed".to_string())].into_iter(),
                            ),
                        ))
                    },
                    wait: move |_| {
                        closing_session.closing.store(true, Ordering::SeqCst);
                        recorded_wait_reached.store(true, Ordering::SeqCst);
                    },
                },
            )
            .expect("initial attach should succeed");

            wait_until("closing reconnect", Duration::from_secs(2), || {
                wait_reached.load(Ordering::SeqCst)
            });
            assert_eq!(session.snapshot().status, "running");
            assert!(terminal_events
                .lock()
                .expect("terminal events lock poisoned")
                .is_empty());
            assert_eq!(*ensure_calls.lock().expect("ensure calls lock poisoned"), 0);
        }

        #[test]
        fn transient_attach_closures_never_fail_a_live_session() {
            let manager = TerminalManager::default();
            let summary = running_supervisor_summary("terminal-immediate-close");
            let snapshot_summary = summary.clone();
            let session = insert_supervised_test_session(&manager, &summary.id, "running");
            let terminal_events = Arc::new(Mutex::new(Vec::new()));
            let recorded_terminal_events = terminal_events.clone();
            let connection_events = Arc::new(Mutex::new(Vec::new()));
            let recorded_connection_events = connection_events.clone();
            let waits = Arc::new(Mutex::new(Vec::new()));
            let recorded_waits = waits.clone();
            let attach_calls = Arc::new(Mutex::new(0_u32));
            let recorded_attach_calls = attach_calls.clone();
            let closing_session = session.clone();

            spawn_supervisor_attach_with(
                manager,
                session.clone(),
                SupervisorAttachSinks {
                    terminal: Arc::new(move |event| {
                        recorded_terminal_events
                            .lock()
                            .expect("terminal events lock poisoned")
                            .push(event);
                    }),
                    connection: Arc::new(move |event| {
                        let should_close = matches!(
                            event,
                            SupervisorConnectionEvent::Reconnected(
                                TerminalSupervisorReconnectedEvent { attempt: 12, .. }
                            )
                        );
                        recorded_connection_events
                            .lock()
                            .expect("connection events lock poisoned")
                            .push(event);
                        if should_close {
                            closing_session.closing.store(true, Ordering::SeqCst);
                        }
                    }),
                },
                false,
                SupervisorAttachHooks {
                    ensure: || Ok(()),
                    attach: move |_id: &str, _from_sequence: Option<u64>| {
                        *recorded_attach_calls
                            .lock()
                            .expect("attach calls lock poisoned") += 1;
                        Ok((
                            halv_supervisor::Snapshot {
                                session: snapshot_summary.clone(),
                                sequence: 0,
                                scrollback: String::new(),
                            },
                            TestSupervisorEvents::new(std::iter::empty()),
                        ))
                    },
                    wait: move |delay| {
                        recorded_waits
                            .lock()
                            .expect("waits lock poisoned")
                            .push(delay);
                    },
                },
            )
            .expect("initial attach should succeed");

            wait_until(
                "continued transient reconnects",
                Duration::from_secs(2),
                || {
                    connection_events
                        .lock()
                        .expect("connection events lock poisoned")
                        .iter()
                        .any(|event| {
                            matches!(
                                event,
                                SupervisorConnectionEvent::Reconnected(
                                    TerminalSupervisorReconnectedEvent { attempt: 12, .. }
                                )
                            )
                        })
                },
            );
            assert_eq!(session.snapshot().status, "running");
            assert_eq!(
                *attach_calls.lock().expect("attach calls lock poisoned"),
                13
            );
            let reconnect_attempts = connection_events
                .lock()
                .expect("connection events lock poisoned")
                .iter()
                .filter_map(|event| match event {
                    SupervisorConnectionEvent::Reconnecting(payload) => Some(payload.attempt),
                    SupervisorConnectionEvent::Reconnected(_) => None,
                })
                .collect::<Vec<_>>();
            assert_eq!(reconnect_attempts, (1..=12).collect::<Vec<_>>());
            assert_eq!(waits.lock().expect("waits lock poisoned").len(), 12);
            assert!(terminal_events
                .lock()
                .expect("terminal events lock poisoned")
                .is_empty());
        }

        #[test]
        fn brief_streamed_data_keeps_the_reconnect_attempt_counter() {
            let manager = TerminalManager::default();
            let summary = running_supervisor_summary("terminal-reset-attempt");
            let snapshot_summary = summary.clone();
            let session = insert_supervised_test_session(&manager, &summary.id, "running");
            let connection_events = Arc::new(Mutex::new(Vec::new()));
            let recorded_connection_events = connection_events.clone();
            let waits = Arc::new(Mutex::new(Vec::new()));
            let recorded_waits = waits.clone();
            let terminal_events = Arc::new(Mutex::new(Vec::new()));
            let recorded_terminal_events = terminal_events.clone();
            let mut attach_count = 0_u32;

            spawn_supervisor_attach_with(
                manager,
                session.clone(),
                SupervisorAttachSinks {
                    terminal: Arc::new(move |event| {
                        recorded_terminal_events
                            .lock()
                            .expect("terminal events lock poisoned")
                            .push(event);
                    }),
                    connection: Arc::new(move |event| {
                        recorded_connection_events
                            .lock()
                            .expect("connection events lock poisoned")
                            .push(event);
                    }),
                },
                false,
                SupervisorAttachHooks {
                    ensure: || Ok(()),
                    attach: move |_id: &str, _from_sequence: Option<u64>| {
                        attach_count += 1;
                        let snapshot = halv_supervisor::Snapshot {
                            session: snapshot_summary.clone(),
                            sequence: u64::from(attach_count >= 3),
                            scrollback: String::new(),
                        };
                        let events = match attach_count {
                            1 => vec![Err("connection closed".to_string())],
                            2 => vec![
                                Ok(SupervisorEvent::Data {
                                    sequence: 1,
                                    data: "streamed".to_string(),
                                }),
                                Err("connection closed".to_string()),
                            ],
                            3 => vec![Ok(SupervisorEvent::Exit {
                                sequence: 2,
                                exit_code: Some(0),
                                signal: None,
                            })],
                            _ => panic!("unexpected attach call {attach_count}"),
                        };
                        Ok((snapshot, TestSupervisorEvents::new(events.into_iter())))
                    },
                    wait: move |delay| {
                        recorded_waits
                            .lock()
                            .expect("waits lock poisoned")
                            .push(delay);
                    },
                },
            )
            .expect("initial attach should succeed");

            wait_until("brief reconnect with data", Duration::from_secs(2), || {
                session.snapshot().status == "exited"
            });
            let reconnect_attempts = connection_events
                .lock()
                .expect("connection events lock poisoned")
                .iter()
                .filter_map(|event| match event {
                    SupervisorConnectionEvent::Reconnecting(payload) => Some(payload.attempt),
                    SupervisorConnectionEvent::Reconnected(_) => None,
                })
                .collect::<Vec<_>>();
            assert_eq!(reconnect_attempts, [1, 2]);
            assert_eq!(
                *waits.lock().expect("waits lock poisoned"),
                [Duration::from_millis(250), Duration::from_millis(500)]
            );
            assert!(terminal_events
                .lock()
                .expect("terminal events lock poisoned")
                .iter()
                .any(|event| {
                    event.kind == "data"
                        && event.sequence == 1
                        && event.data.as_deref() == Some("streamed")
                        && event.replay.is_none()
                }));
        }

        #[test]
        fn repeated_drops_increment_attempts_and_grow_backoff() {
            let manager = TerminalManager::default();
            let summary = running_supervisor_summary("terminal-repeated-drop");
            let snapshot_summary = summary.clone();
            let session = insert_supervised_test_session(&manager, &summary.id, "running");
            let connection_events = Arc::new(Mutex::new(Vec::new()));
            let recorded_connection_events = connection_events.clone();
            let waits = Arc::new(Mutex::new(Vec::new()));
            let recorded_waits = waits.clone();
            let closing_session = session.clone();

            spawn_supervisor_attach_with(
                manager,
                session,
                SupervisorAttachSinks {
                    terminal: Arc::new(|_| {}),
                    connection: Arc::new(move |event| {
                        let should_close = matches!(
                            event,
                            SupervisorConnectionEvent::Reconnected(
                                TerminalSupervisorReconnectedEvent { attempt: 2, .. }
                            )
                        );
                        recorded_connection_events
                            .lock()
                            .expect("connection events lock poisoned")
                            .push(event);
                        if should_close {
                            closing_session.closing.store(true, Ordering::SeqCst);
                        }
                    }),
                },
                false,
                SupervisorAttachHooks {
                    ensure: || Ok(()),
                    attach: move |_id: &str, _from_sequence: Option<u64>| {
                        Ok((
                            halv_supervisor::Snapshot {
                                session: snapshot_summary.clone(),
                                sequence: 0,
                                scrollback: String::new(),
                            },
                            TestSupervisorEvents::new(
                                vec![Err("connection closed".to_string())].into_iter(),
                            ),
                        ))
                    },
                    wait: move |delay| {
                        recorded_waits
                            .lock()
                            .expect("waits lock poisoned")
                            .push(delay);
                    },
                },
            )
            .expect("initial attach should succeed");

            wait_until("repeated reconnect", Duration::from_secs(2), || {
                connection_events
                    .lock()
                    .expect("connection events lock poisoned")
                    .len()
                    >= 4
            });
            let reconnect_attempts = connection_events
                .lock()
                .expect("connection events lock poisoned")
                .iter()
                .filter_map(|event| match event {
                    SupervisorConnectionEvent::Reconnecting(payload) => Some(payload.attempt),
                    SupervisorConnectionEvent::Reconnected(_) => None,
                })
                .collect::<Vec<_>>();
            assert_eq!(reconnect_attempts, [1, 2]);
            assert_eq!(
                *waits.lock().expect("waits lock poisoned"),
                [Duration::from_millis(250), Duration::from_millis(500)]
            );
        }
    }

    #[cfg(unix)]
    #[test]
    #[ignore = "requires the standalone supervisor binary to be built"]
    fn supervised_session_is_adopted_by_a_second_manager() {
        let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
        let binary = [
            manifest_dir.join("supervisor/target/debug/halv-supervisor"),
            manifest_dir.join("supervisor/target/release/halv-supervisor"),
        ]
        .into_iter()
        .find(|candidate| candidate.is_file());
        let Some(binary) = binary else {
            eprintln!("skipping supervised adoption test: build src-tauri/supervisor first");
            return;
        };
        let root = PathBuf::from("/tmp").join(format!(
            "halv-adopt-{}-{:016x}",
            std::process::id(),
            rand::random::<u128>()
        ));
        std::fs::create_dir_all(&root).expect("test directory should exist");
        let probe = root.join("probe.sock");
        match std::os::unix::net::UnixListener::bind(&probe) {
            Ok(listener) => {
                drop(listener);
                std::fs::remove_file(&probe).expect("probe socket should be removable");
            }
            Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
                eprintln!("skipping supervised adoption test: Unix sockets are unavailable");
                let _ = std::fs::remove_dir_all(&root);
                return;
            }
            Err(error) => panic!("failed to probe Unix socket support: {error}"),
        }
        let socket = crate::supervisor_link::socket_path(&root);
        let log = crate::supervisor_link::log_path(&root);
        let child = std::process::Command::new(binary)
            .arg("--socket")
            .arg(&socket)
            .arg("--log")
            .arg(&log)
            .arg("--idle-timeout-secs")
            .arg("60")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("supervisor should start");
        let mut server = TestSupervisorProcess {
            child: Some(child),
            root: root.clone(),
        };
        wait_until("supervisor startup", Duration::from_secs(5), || {
            SupervisorClient::connect(&socket, "integration-test").is_ok()
        });

        let first = TerminalManager::default();
        first
            .set_app_data_dir(&root)
            .expect("first manager should initialize");
        // A shell tab owns this session, so it has no conversation.
        let session = TerminalSession {
            id: "terminal-adoption".to_string(),
            title: None,
            project_id: Some(9),
            conversation_id: None,
            project_name: "Halv".to_string(),
            cwd: "/tmp".to_string(),
            shell: "/bin/sh".to_string(),
            pid: 0,
            execution_profile: "full_access".to_string(),
            agent_kind: Some("codex".to_string()),
            status: "running".to_string(),
            cols: 80,
            rows: 24,
            started_at: Utc::now().to_rfc3339(),
            exited_at: None,
            exit_code: None,
            signal: None,
            continued_from: Some("terminal-agent-source".to_string()),
            shell_tab_id: Some("0f8fad5b-d9cb-469f-a165-70867728950e".to_string()),
        };
        let opened = spawn_supervised_terminal(
            &first,
            session,
            Path::new("/tmp"),
            build_terminal_command(Path::new("/tmp"), None),
            "integration-test".to_string(),
            Arc::new(|_| {}),
            Arc::new(|_| {}),
            None,
        )
        .expect("supervised terminal should open");
        let status = first.supervisor_status();
        assert!(status.available);
        assert_eq!(
            status.pid,
            server.child.as_ref().map(std::process::Child::id)
        );
        assert_eq!(status.sessions, 1);
        first
            .with_supervisor("integration-test", |client| {
                client.write(&opened.id, b"printf 'BEFORE_ADOPTION\\n'\n")
            })
            .expect("terminal input should write");
        wait_until("first output", Duration::from_secs(5), || {
            first
                .get(&opened.id)
                .map(|session| session.attach().scrollback.contains("BEFORE_ADOPTION"))
                .unwrap_or(false)
        });
        first
            .supervisor
            .lock()
            .expect("terminal supervisor lock poisoned")
            .take();

        let second = TerminalManager::default();
        second
            .set_app_data_dir(&root)
            .expect("second manager should initialize");
        second
            .adopt_supervised_sessions_with_sink(
                "integration-test",
                Arc::new(|_| {}),
                Arc::new(|_| {}),
            )
            .expect("running terminal should be adopted");
        wait_until("adopted output", Duration::from_secs(5), || {
            second
                .get(&opened.id)
                .map(|session| {
                    let attached = session.attach();
                    attached.session.status == "running"
                        && attached.session.pid == opened.pid
                        && attached.scrollback.contains("BEFORE_ADOPTION")
                })
                .unwrap_or(false)
        });
        assert_eq!(second.list()[0].id, opened.id);
        // The continuation link rides in the supervisor metadata.
        assert_eq!(
            second.list()[0].continued_from.as_deref(),
            Some("terminal-agent-source")
        );
        // The shell tab owner rides in the supervisor metadata too.
        assert_eq!(
            second.list()[0].shell_tab_id.as_deref(),
            Some("0f8fad5b-d9cb-469f-a165-70867728950e")
        );

        second
            .close_session(&opened.id)
            .expect("adopted terminal should close");
        second
            .stop_all_supervised()
            .expect("supervisor should stop");
        server.wait_for_exit();
    }

    #[cfg(unix)]
    struct TestSupervisorProcess {
        child: Option<std::process::Child>,
        root: PathBuf,
    }

    #[cfg(unix)]
    impl TestSupervisorProcess {
        fn wait_for_exit(&mut self) {
            let deadline = Instant::now() + Duration::from_secs(5);
            while Instant::now() < deadline {
                if self
                    .child
                    .as_mut()
                    .and_then(|child| child.try_wait().ok())
                    .flatten()
                    .is_some()
                {
                    self.child.take();
                    return;
                }
                thread::sleep(Duration::from_millis(25));
            }
            panic!("supervisor did not exit");
        }
    }

    #[cfg(unix)]
    impl Drop for TestSupervisorProcess {
        fn drop(&mut self) {
            if let Some(child) = self.child.as_mut() {
                if let Ok(Some(mut client)) =
                    crate::supervisor_link::connect_existing(&self.root, "integration-test")
                {
                    let _ = client.shutdown(true);
                }
                let deadline = Instant::now() + Duration::from_secs(2);
                while Instant::now() < deadline {
                    if child.try_wait().ok().flatten().is_some() {
                        break;
                    }
                    thread::sleep(Duration::from_millis(25));
                }
                if child.try_wait().ok().flatten().is_none() {
                    let _ = child.kill();
                    let _ = child.wait();
                }
            }
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    #[cfg(unix)]
    fn wait_until(label: &str, timeout: Duration, condition: impl Fn() -> bool) {
        let deadline = Instant::now() + timeout;
        while !condition() {
            assert!(Instant::now() < deadline, "{label} timed out");
            thread::sleep(Duration::from_millis(25));
        }
    }

    #[cfg(unix)]
    #[test]
    fn native_pty_accepts_input_and_preserves_shell_state() {
        let pair = native_pty_system()
            .openpty(terminal_size(80, 24))
            .expect("native PTY should open");
        let mut command = CommandBuilder::new("/bin/sh");
        command.env("PS1", "");
        let mut child = pair
            .slave
            .spawn_command(command)
            .expect("shell should spawn in PTY");
        let mut reader = pair
            .master
            .try_clone_reader()
            .expect("PTY output should attach");
        let mut writer = pair.master.take_writer().expect("PTY input should attach");
        drop(pair.slave);

        let (output_tx, output_rx) = std::sync::mpsc::channel();
        thread::spawn(move || {
            let mut output = Vec::new();
            let _ = reader.read_to_end(&mut output);
            let _ = output_tx.send(output);
        });

        if cfg!(target_os = "macos") {
            thread::sleep(std::time::Duration::from_millis(20));
        }

        writer
            .write_all(b"printf 'HALV_PTY_OK\\n'; cd /; pwd; exit\r\n")
            .expect("PTY input should write");
        writer.flush().expect("PTY input should flush");
        if cfg!(target_os = "macos") {
            thread::sleep(std::time::Duration::from_millis(20));
        }
        drop(writer);
        let mut status = None;
        for _ in 0..500 {
            status = child.try_wait().expect("shell status should poll");
            if status.is_some() {
                break;
            }
            thread::sleep(std::time::Duration::from_millis(10));
        }
        let status = match status {
            Some(status) => status,
            None => {
                let _ = child.kill();
                panic!("shell did not exit after terminal input");
            }
        };
        assert!(status.success());

        drop(pair.master);
        let output = output_rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("PTY output reader should finish");
        let output = String::from_utf8_lossy(&output);
        assert!(output.contains("HALV_PTY_OK"), "output was: {output}");
        assert!(
            output
                .lines()
                .any(|line| line.trim_end_matches('\r') == "/"),
            "output was: {output}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn ended_agent_continues_in_a_native_shell_in_the_same_folder() {
        let folder = std::env::temp_dir().join(format!(
            "halv-continue-{}-{:016x}",
            std::process::id(),
            rand::random::<u64>()
        ));
        std::fs::create_dir_all(&folder).expect("test folder should exist");
        let folder = folder.canonicalize().expect("test folder should resolve");
        let manager = TerminalManager::default();
        let shell_command = |arguments: &[&str]| ResolvedCommand {
            program: PathBuf::from("/bin/sh"),
            args: arguments.iter().map(OsString::from).collect(),
            env: vec![(OsString::from("PS1"), OsString::new())],
            env_remove: Vec::new(),
        };
        let source = TerminalSession {
            cwd: folder.to_string_lossy().into_owned(),
            ..test_terminal_record("agent-native", Some("claude"), "running", None)
        };
        spawn_in_process_terminal(
            &manager,
            source,
            folder.clone(),
            shell_command(&["-c", "printf HALV_AGENT_DONE; exit 0"]),
            Arc::new(|_| {}),
            None,
        )
        .expect("the agent stand-in should start");
        wait_until("agent exit", Duration::from_secs(5), || {
            manager
                .session("agent-native")
                .is_ok_and(|session| session.status == "exited")
        });
        let ended = manager
            .session("agent-native")
            .expect("the ended agent should stay listed");
        assert!(clean_self_exit(
            &ended,
            manager.halv_stopped("agent-native")
        ));

        let (shell, created) = manager
            .continue_with_shell_with("agent-native", |request, continuation| {
                let session = TerminalSession {
                    cwd: request.cwd.to_string_lossy().into_owned(),
                    continued_from: Some(continuation.continued_from),
                    ..test_terminal_record("terminal-native-shell", None, "running", None)
                };
                spawn_in_process_terminal(
                    &manager,
                    session,
                    request.cwd,
                    shell_command(&[]),
                    Arc::new(|_| {}),
                    Some(continuation.prelude),
                )
            })
            .expect("the continuation shell should start");
        assert!(created);
        let continued = manager
            .get(&shell.id)
            .expect("the continuation shell should be listed");
        let prelude = continued
            .attach_with_prelude()
            .prelude
            .expect("the continuation shell should carry a prelude");
        assert!(prelude.scrollback.contains("HALV_AGENT_DONE"));
        assert_eq!(prelude.exit_code, Some(0));

        if let Backend::InProcess { writer, .. } = &continued.backend {
            let mut writer = writer.lock().expect("terminal writer lock poisoned");
            let writer = writer.as_mut().expect("shell input should be open");
            writer
                .write_all(b"pwd; exit\n")
                .and_then(|()| writer.flush())
                .expect("shell input should write");
        } else {
            panic!("the continuation shell should run in process");
        }
        let expected = folder.to_string_lossy().into_owned();
        wait_until("shell pwd", Duration::from_secs(5), || {
            continued
                .attach()
                .scrollback
                .lines()
                .any(|line| line.trim_end_matches('\r') == expected)
        });
        wait_until("shell exit", Duration::from_secs(5), || {
            continued.snapshot().status != "running"
        });
        let _ = std::fs::remove_dir_all(&folder);
    }

    #[cfg(unix)]
    mod input_tests {
        use super::*;
        use halv_supervisor::server::InProcessSupervisor;
        use std::sync::{mpsc, Barrier};

        const TEST_TIMEOUT: Duration = Duration::from_secs(10);
        const TEST_APP_VERSION: &str = "input-test";

        fn session_record(id: &str, status: &str) -> TerminalSession {
            TerminalSession {
                id: id.to_string(),
                title: None,
                project_id: None,
                conversation_id: None,
                project_name: "Halv".to_string(),
                cwd: "/tmp".to_string(),
                shell: "/bin/cat".to_string(),
                pid: 0,
                execution_profile: "full_access".to_string(),
                agent_kind: None,
                status: status.to_string(),
                cols: 80,
                rows: 24,
                started_at: Utc::now().to_rfc3339(),
                exited_at: None,
                exit_code: None,
                signal: None,
                continued_from: None,
                shell_tab_id: None,
            }
        }

        fn insert_supervised_session(
            manager: &TerminalManager,
            id: &str,
            status: &str,
        ) -> Arc<ManagedTerminal> {
            insert_test_terminal(manager, session_record(id, status), "")
        }

        fn open_cat_session(supervisor: &InProcessSupervisor, id: &str) {
            supervisor
                .connect(TEST_APP_VERSION)
                .expect("the in-process supervisor should connect")
                .open(OpenRequest {
                    id: id.to_string(),
                    program: "/bin/cat".to_string(),
                    args: Vec::new(),
                    cwd: "/tmp".to_string(),
                    env: Vec::new(),
                    env_remove: Vec::new(),
                    cols: 80,
                    rows: 24,
                    meta: serde_json::Value::Null,
                })
                .expect("the cat session should open");
        }

        /// The input lines in the order that the PTY first showed them. The
        /// PTY echoes each line before `cat` prints it again.
        fn first_seen_lines(scrollback: &str) -> Vec<String> {
            let mut seen: Vec<String> = Vec::new();
            for token in scrollback.split(|character: char| !character.is_ascii_alphanumeric()) {
                if token.len() == 5
                    && token.starts_with('L')
                    && !seen.iter().any(|line| line == token)
                {
                    seen.push(token.to_string());
                }
            }
            seen
        }

        #[test]
        fn input_does_not_wait_for_the_shared_supervisor_connection() {
            const LINES: u64 = 200;
            const PRODUCERS: u64 = 4;
            let supervisor = Arc::new(InProcessSupervisor::new());
            let id = "input-order";
            open_cat_session(&supervisor, id);
            let manager = TerminalManager::default();
            let session = insert_supervised_session(&manager, id, "running");

            // A close or a supervisor start holds the shared connection.
            let shared_connection = manager
                .supervisor
                .lock()
                .expect("terminal supervisor lock poisoned");

            let (finished, finished_rx) = mpsc::channel();
            {
                let manager = manager.clone();
                let supervisor = supervisor.clone();
                thread::spawn(move || {
                    let start = Arc::new(Barrier::new(PRODUCERS as usize));
                    let producers = (0..PRODUCERS)
                        .map(|producer| {
                            let manager = manager.clone();
                            let supervisor = supervisor.clone();
                            let session = session.clone();
                            let start = start.clone();
                            thread::spawn(move || {
                                start.wait();
                                (1..=LINES)
                                    .filter(|line| line % PRODUCERS == producer)
                                    .map(|line| {
                                        let supervisor = supervisor.clone();
                                        let session = session.clone();
                                        manager
                                            .queue_input_with(
                                                id,
                                                format!("L{line:04}\n"),
                                                Some(InputOrder {
                                                    writer: "pane".to_string(),
                                                    sequence: line,
                                                }),
                                                move || {
                                                    Ok(supervised_input_sink(
                                                        session,
                                                        id.to_string(),
                                                        move || {
                                                            supervisor.connect(TEST_APP_VERSION)
                                                        },
                                                    ))
                                                },
                                            )
                                            .expect("input should queue")
                                    })
                                    .collect::<Vec<_>>()
                            })
                        })
                        .collect::<Vec<_>>();
                    let results = producers
                        .into_iter()
                        .flat_map(|producer| producer.join().expect("producer should finish"))
                        .map(|written| written.blocking_recv().expect("the writer should answer"))
                        .collect::<Vec<_>>();
                    let _ = finished.send(results);
                });
            }
            let results = finished_rx
                .recv_timeout(TEST_TIMEOUT)
                .expect("input must not wait for the shared supervisor connection");
            drop(shared_connection);
            assert!(results.iter().all(Result::is_ok), "results: {results:?}");

            let expected = (1..=LINES)
                .map(|line| format!("L{line:04}"))
                .collect::<Vec<_>>();
            let deadline = Instant::now() + TEST_TIMEOUT;
            let mut lines = Vec::new();
            while Instant::now() < deadline {
                let (snapshot, _events) = supervisor
                    .attach(TEST_APP_VERSION, id)
                    .expect("the cat session should attach");
                lines = first_seen_lines(&snapshot.scrollback);
                if lines.len() == expected.len() {
                    break;
                }
                thread::sleep(Duration::from_millis(20));
            }
            manager.stop_input(id);
            let _ = supervisor
                .connect(TEST_APP_VERSION)
                .and_then(|mut client| client.shutdown(true));
            assert_eq!(lines, expected);
        }

        #[test]
        fn input_for_a_stopped_or_missing_session_fails_at_once() {
            let manager = TerminalManager::default();
            insert_supervised_session(&manager, "input-exited", "exited");

            assert_eq!(
                manager
                    .queue_input("input-exited", "a".to_string(), None, TEST_APP_VERSION)
                    .err(),
                Some("terminal session is not running".to_string())
            );
            assert_eq!(
                manager
                    .queue_input("input-missing", "a".to_string(), None, TEST_APP_VERSION)
                    .err(),
                Some("terminal session `input-missing` was not found".to_string())
            );
        }

        #[test]
        fn a_stopped_input_queue_is_replaced() {
            let manager = TerminalManager::default();
            manager
                .inputs
                .lock()
                .expect("terminal input lock poisoned")
                .insert("input-restart".to_string(), InputQueue::stopped());
            let written = Arc::new(Mutex::new(Vec::new()));
            let sink_written = written.clone();

            let answer = manager
                .queue_input_with("input-restart", "b".to_string(), None, move || {
                    Ok(Box::new(move |data: &str| {
                        sink_written.lock().unwrap().push(data.to_string());
                        Ok(())
                    }))
                })
                .expect("input should queue on a new writer thread");
            assert_eq!(answer.blocking_recv().unwrap(), Ok(()));
            assert_eq!(*written.lock().unwrap(), ["b"]);
        }
    }
}
