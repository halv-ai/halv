//! The status of each agent terminal pane: working, done, needs input,
//! needs approval, idle, exited, failed, or interrupted.
//!
//! One store holds the status of each pane, keyed by the terminal session
//! id. A background thread polls the terminal list every 300 ms. For each
//! Claude-family pane (Claude, GLM, Grok) it reads the hook journal of the
//! pane and then the Claude session presence file of the pane process.
//! Codex panes get their turns from the rollout watcher and their waits
//! from the terminal notifications of Codex. The terminal title of each pane
//! is the fallback signal: it drives a pane while no other signal is fresh,
//! for example a Claude pane that an older Halv started without hooks. The
//! PTY exit ends each pane.
//!
//! Each change goes to the webview as the `agent-status` event and to Rust
//! subscribers through a broadcast channel. The webview keeps its byte
//! volume heuristic only for panes that have no signal here.

mod hooks;
mod launch;
mod machine;
pub(crate) mod notify;
mod osc;
mod presence;
mod title;

pub(crate) use hooks::{claude_status_hooks_for, ClaudeStatusHooks};
pub(crate) use launch::orca_variables;
pub use machine::{AgentStatus, AgentStatusEntry, PromptDetail, RolloutTurn, StatusSource};

use halv_supervisor::agent_hook::{SESSION_ID_ENV, STATUS_DIR_ENV};
use hooks::{HookRecord, JournalTail};
use launch::ConfigDirRegistry;
use machine::{AnswerKey, PaneInput, PaneStatusMachine, TitleStatus};
use osc::{OscEvent, OscScanner};
use presence::{DiskFiles, PresenceFiles, PresenceProbe, PresenceRead};
use std::{
    collections::{HashMap, HashSet},
    ffi::OsString,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard, OnceLock},
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::broadcast;

pub const AGENT_STATUS_EVENT: &str = "agent-status";
const POLL_INTERVAL: Duration = Duration::from_millis(300);
const BROADCAST_CAPACITY: usize = 256;
const STATE_DIR: &str = "agent-status";
/// The journals, the wake socket, and the helper copy. Debug builds keep
/// their own folder, as they keep their own supervisor socket.
const HOOKS_DIR: &str = if cfg!(debug_assertions) {
    "hooks-dev"
} else {
    "hooks"
};
const ESCAPE_KEY: &str = "\u{1b}";
const PASTE_END: &str = "\u{1b}[201~";
/// A pane younger than this can show the start animation of its agent in
/// its title. Codex shows a spinner for some seconds before its first
/// prompt.
const PANE_START_MS: u64 = 10_000;

static HOOKS_PATH: OnceLock<PathBuf> = OnceLock::new();
static HELPER_PATH: OnceLock<PathBuf> = OnceLock::new();

/// The agents that this module tracks: each agent that Halv launches. A
/// pane with an unknown agent keeps the webview heuristic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PaneAgent {
    /// Claude, GLM, and Grok lanes all run the Claude Code CLI.
    ClaudeFamily,
    Codex,
    Gemini,
    Antigravity,
    Kimi,
}

impl PaneAgent {
    fn from_agent_kind(agent_kind: Option<&str>) -> Option<Self> {
        match agent_kind? {
            "claude" | "zai" | "grok" => Some(Self::ClaudeFamily),
            "codex" => Some(Self::Codex),
            "gemini" => Some(Self::Gemini),
            "antigravity" => Some(Self::Antigravity),
            "kimi" => Some(Self::Kimi),
            _ => None,
        }
    }
}

/// One terminal pane on one poll.
#[derive(Debug, Clone, PartialEq, Eq)]
struct PaneObservation {
    id: String,
    agent_kind: Option<String>,
    pid: u32,
    running: bool,
    failed: bool,
    /// Unix milliseconds of the PTY start.
    started_at: u64,
    /// The last title in the scrollback of a pane that is not tracked yet.
    /// It seeds the title state of an adopted pane, which can rest with no
    /// new title for a long time.
    title_seed: Option<String>,
}

/// Gives the folders to try for a pane with no recorded config folder. The
/// service calls it only for such a pane, because the wallet lookup can read
/// a file.
type FallbackDirs<'a> = &'a dyn Fn() -> Vec<PathBuf>;

struct PaneTracker {
    agent: PaneAgent,
    machine: PaneStatusMachine,
    pid: u32,
    started_at: u64,
    probe: Option<PresenceProbe>,
    /// Folders to try for a pane with no recorded config folder.
    candidates: Option<Vec<PresenceProbe>>,
    /// The hook journal of a Claude-family pane.
    journal: Option<JournalTail>,
    /// Journal lines from before this time came from an earlier app run.
    created_at: u64,
}

impl PaneTracker {
    fn new(agent: PaneAgent, now: u64, hooks_dir: Option<&Path>, session_id: &str) -> Self {
        Self {
            agent,
            machine: PaneStatusMachine::new(now),
            pid: 0,
            started_at: 0,
            probe: None,
            candidates: None,
            journal: hooks_dir
                .filter(|_| agent == PaneAgent::ClaudeFamily)
                .and_then(|dir| JournalTail::new(dir, session_id)),
            created_at: now,
        }
    }

    /// Applies the new journal lines. Lines from an earlier app run rebuild
    /// the status, and their finished turns count as read: the unread state
    /// lives in memory only. The tail reads the whole journal, so the replay
    /// ends before the presence file of the pane applies.
    fn read_journal(&mut self, now: u64, focused: bool) -> bool {
        let Some(journal) = &mut self.journal else {
            return false;
        };
        let records = journal.read_new();
        let live_from = records
            .iter()
            .position(|record| record.at >= self.created_at)
            .unwrap_or(records.len());
        let mut records = records.into_iter();
        let mut changed = false;
        if live_from > 0 {
            let mut last_at = 0;
            for record in records.by_ref().take(live_from) {
                last_at = record.at;
                changed |= self.machine.apply(hook_input(record), last_at, false);
            }
            // A Stop at the end of the history has no later event to wait for.
            let settled_at = last_at.saturating_add(machine::REST_GRACE_MS).min(now);
            changed |= self.machine.apply(PaneInput::Tick, settled_at, false);
            changed |= self.machine.mark_viewed();
        }
        for record in records {
            changed |= self.machine.apply(hook_input(record), now, focused);
        }
        changed
    }

    /// Applies the last title from the scrollback of a new pane. The title
    /// is the current state, not news: a turn that it shows as finished
    /// counts as read, as a replayed journal does. Work in the title of an
    /// older pane is a turn in progress. Work in the title of a pane that
    /// just started can be the start animation of the agent.
    fn seed_title(&mut self, title: Option<&str>, started_long_ago: bool, now: u64) -> bool {
        let Some(status) = title.and_then(|title| title::classify_title(self.agent, title)) else {
            return false;
        };
        let mut changed = self.machine.apply(PaneInput::Title(status), now, false);
        if started_long_ago {
            changed |= self.machine.claim_title_work(now);
        }
        if changed {
            self.machine.mark_viewed();
        }
        changed
    }

    fn poll_presence(
        &mut self,
        session_id: &str,
        registry: &mut ConfigDirRegistry,
        files: &dyn PresenceFiles,
        fallback_dirs: FallbackDirs<'_>,
        now: u64,
    ) -> Option<PaneInput> {
        if self.probe.is_none() {
            if let Some(dir) = registry.get(session_id) {
                self.probe = Some(PresenceProbe::new(dir, self.pid, self.started_at));
            }
        }
        if let Some(probe) = &mut self.probe {
            return match probe.poll(files) {
                PresenceRead::State(state) => Some(PaneInput::Presence(state)),
                PresenceRead::Missing => Some(PaneInput::PresenceLost),
                PresenceRead::Unchanged => None,
            };
        }
        // An adopted pane from an earlier app run can lack a record. Try
        // the likely folders until one holds the file of this process.
        let (pid, started_at) = (self.pid, self.started_at);
        let candidates = self.candidates.get_or_insert_with(|| {
            fallback_dirs()
                .iter()
                .map(|dir| PresenceProbe::new(dir, pid, started_at))
                .collect()
        });
        let (index, state) = candidates
            .iter_mut()
            .enumerate()
            .find_map(|(index, probe)| match probe.poll(files) {
                PresenceRead::State(state) => Some((index, state)),
                _ => None,
            })?;
        let probe = candidates.swap_remove(index);
        self.candidates = None;
        registry.insert(session_id, probe.config_dir().to_path_buf(), now);
        self.probe = Some(probe);
        Some(PaneInput::Presence(state))
    }
}

fn hook_input(record: HookRecord) -> PaneInput {
    PaneInput::Hook(Box::new(record))
}

/// Reads the live PTY output of one tracked pane.
#[derive(Debug)]
struct PaneScanner {
    agent: PaneAgent,
    osc: OscScanner,
    /// The state of the last title that told one. A spinner frame changes
    /// the title often, so only a change of the state goes on.
    title: Option<TitleStatus>,
}

impl PaneScanner {
    fn new(agent: PaneAgent) -> Self {
        Self {
            agent,
            osc: OscScanner::default(),
            title: None,
        }
    }

    /// The signals in one chunk of live output, in order.
    fn scan(&mut self, data: &str) -> Vec<PaneInput> {
        let mut inputs = Vec::new();
        for event in self.osc.feed(data) {
            match event {
                OscEvent::Notification(text) if self.agent == PaneAgent::Codex => {
                    if let Some(notice) = osc::classify_codex_notification(&text) {
                        inputs.push(PaneInput::Terminal(notice));
                    }
                }
                OscEvent::Notification(_) => {}
                OscEvent::Title(text) => {
                    let Some(status) = title::classify_title(self.agent, &text) else {
                        continue;
                    };
                    if self.title != Some(status) {
                        self.title = Some(status);
                        inputs.push(PaneInput::Title(status));
                    }
                }
            }
        }
        inputs
    }
}

#[derive(Default)]
struct Inner {
    panes: HashMap<String, PaneTracker>,
    /// The pane that the user looks at in a focused window.
    focused: Option<String>,
    registry: ConfigDirRegistry,
    badge_count: Option<usize>,
    /// The folder of the hook journals, when the app could create it.
    hooks_dir: Option<PathBuf>,
}

/// The status of every tracked agent pane. Tauri manages one instance.
#[derive(Clone)]
pub struct AgentStatusStore {
    inner: Arc<Mutex<Inner>>,
    /// The output scanners of the running tracked panes. A separate lock
    /// keeps the PTY output path off the lock of the poll.
    scanners: Arc<Mutex<HashMap<String, PaneScanner>>>,
    sender: broadcast::Sender<AgentStatusEntry>,
}

impl Default for AgentStatusStore {
    fn default() -> Self {
        Self::with_registry(ConfigDirRegistry::default(), None)
    }
}

impl AgentStatusStore {
    fn with_registry(registry: ConfigDirRegistry, hooks_dir: Option<PathBuf>) -> Self {
        let (sender, _) = broadcast::channel(BROADCAST_CAPACITY);
        Self {
            inner: Arc::new(Mutex::new(Inner {
                registry,
                hooks_dir,
                ..Inner::default()
            })),
            scanners: Arc::default(),
            sender,
        }
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().expect("agent status lock poisoned")
    }

    fn lock_scanners(&self) -> MutexGuard<'_, HashMap<String, PaneScanner>> {
        self.scanners
            .lock()
            .expect("agent status scanner lock poisoned")
    }

    /// Every tracked pane, ordered by session id.
    pub fn snapshot(&self) -> Vec<AgentStatusEntry> {
        let inner = self.lock();
        let mut entries = inner
            .panes
            .iter()
            .map(|(id, tracker)| tracker.machine.entry(id))
            .collect::<Vec<_>>();
        entries.sort_by(|left, right| left.session_id.cmp(&right.session_id));
        entries
    }

    /// A receiver for each later change. It carries the `agent-status`
    /// event payload.
    pub fn subscribe(&self) -> broadcast::Receiver<AgentStatusEntry> {
        self.sender.subscribe()
    }

    /// True when the store tracks the pane already.
    fn tracks(&self, session_id: &str) -> bool {
        self.lock().panes.contains_key(session_id)
    }

    fn observe(
        &self,
        panes: &[PaneObservation],
        now: u64,
        files: &dyn PresenceFiles,
        fallback_dirs: FallbackDirs<'_>,
    ) -> Vec<AgentStatusEntry> {
        let mut inner = self.lock();
        let Inner {
            panes: trackers,
            focused,
            registry,
            hooks_dir,
            ..
        } = &mut *inner;
        let mut changed = Vec::new();
        let mut seen = HashSet::new();
        let mut scanned = HashMap::new();
        for pane in panes {
            let Some(agent) = PaneAgent::from_agent_kind(pane.agent_kind.as_deref()) else {
                continue;
            };
            seen.insert(pane.id.clone());
            if pane.running {
                scanned.insert(pane.id.clone(), agent);
            }
            let focused_here = focused.as_deref() == Some(pane.id.as_str());
            let mut created = false;
            let tracker = trackers.entry(pane.id.clone()).or_insert_with(|| {
                created = true;
                PaneTracker::new(agent, now, hooks_dir.as_deref(), &pane.id)
            });
            let mut updated = created;
            if tracker.pid != pane.pid || tracker.started_at != pane.started_at {
                // An adoption can report the process again. Start over with
                // the presence file of the current process.
                tracker.pid = pane.pid;
                tracker.started_at = pane.started_at;
                tracker.probe = None;
                tracker.candidates = None;
            }
            if !pane.running {
                let input = PaneInput::ProcessExited { error: pane.failed };
                updated |= tracker.machine.apply(input, now, focused_here);
            } else if tracker.agent == PaneAgent::ClaudeFamily && pane.pid != 0 {
                // The helper writes a hook line before Claude changes its
                // presence file, so the journal goes first.
                updated |= tracker.read_journal(now, focused_here);
                if let Some(input) =
                    tracker.poll_presence(&pane.id, registry, files, fallback_dirs, now)
                {
                    updated |= tracker.machine.apply(input, now, focused_here);
                }
                updated |= tracker.machine.apply(PaneInput::Tick, now, focused_here);
            }
            if created && pane.running {
                let started_long_ago = now.saturating_sub(pane.started_at) >= PANE_START_MS;
                updated |= tracker.seed_title(pane.title_seed.as_deref(), started_long_ago, now);
            }
            if updated {
                changed.push(tracker.machine.entry(&pane.id));
            }
        }
        let gone = trackers
            .keys()
            .filter(|id| !seen.contains(*id))
            .cloned()
            .collect::<Vec<_>>();
        for id in gone {
            trackers.remove(&id);
            registry.remove(&id);
            if let Some(dir) = hooks_dir.as_deref() {
                hooks::remove_journals(dir, &id);
            }
            if focused.as_deref() == Some(id.as_str()) {
                *focused = None;
            }
        }
        drop(inner);
        let mut scanners = self.lock_scanners();
        scanners.retain(|id, _| scanned.contains_key(id));
        for (id, agent) in scanned {
            scanners
                .entry(id)
                .or_insert_with(|| PaneScanner::new(agent));
        }
        changed
    }

    /// Reads the journal of one pane after a wake datagram.
    fn wake_pane(&self, session_id: &str, now: u64) -> Option<AgentStatusEntry> {
        let mut inner = self.lock();
        let focused = inner.focused.as_deref() == Some(session_id);
        let tracker = inner.panes.get_mut(session_id)?;
        tracker
            .read_journal(now, focused)
            .then(|| tracker.machine.entry(session_id))
    }

    /// Applies a rollout observation. `recorded_at` is the rollout time of a
    /// turn event from the first read of a rollout. An event from before
    /// the pane was tracked comes from an earlier app run: it rebuilds the
    /// status, and its result counts as read.
    fn note_rollout(
        &self,
        session_id: &str,
        input: PaneInput,
        now: u64,
        recorded_at: Option<u64>,
    ) -> Option<AgentStatusEntry> {
        let mut inner = self.lock();
        let focused = inner.focused.as_deref() == Some(session_id);
        let hooks_dir = inner.hooks_dir.clone();
        let tracker = inner
            .panes
            .entry(session_id.to_string())
            .or_insert_with(|| {
                PaneTracker::new(PaneAgent::Codex, now, hooks_dir.as_deref(), session_id)
            });
        let changed = if recorded_at.is_some_and(|at| at < tracker.created_at) {
            tracker.machine.apply(input, now, false) | tracker.machine.mark_viewed()
        } else {
            tracker.machine.apply(input, now, focused)
        };
        changed.then(|| tracker.machine.entry(session_id))
    }

    /// The Codex notifications and the title changes in a chunk of PTY
    /// output.
    fn scan_output(&self, session_id: &str, data: &str) -> Vec<PaneInput> {
        let mut scanners = self.lock_scanners();
        let Some(scanner) = scanners.get_mut(session_id) else {
            return Vec::new();
        };
        scanner.scan(data)
    }

    fn note_terminal_signals(
        &self,
        session_id: &str,
        inputs: Vec<PaneInput>,
        now: u64,
    ) -> Option<AgentStatusEntry> {
        let mut inner = self.lock();
        let focused = inner.focused.as_deref() == Some(session_id);
        let tracker = inner.panes.get_mut(session_id)?;
        let mut changed = false;
        for input in inputs {
            changed |= tracker.machine.apply(input, now, focused);
        }
        changed.then(|| tracker.machine.entry(session_id))
    }

    fn note_input(
        &self,
        session_id: &str,
        signal: InputSignal,
        now: u64,
    ) -> Option<AgentStatusEntry> {
        let mut inner = self.lock();
        let tracker = inner.panes.get_mut(session_id)?;
        if signal.escape {
            tracker.machine.apply(PaneInput::Escape, now, false);
        }
        let prompted = signal.prompt && tracker.machine.apply(PaneInput::Prompt, now, false);
        let answered = signal
            .answer
            .is_some_and(|key| tracker.machine.apply(PaneInput::Answer(key), now, false));
        (prompted || answered).then(|| tracker.machine.entry(session_id))
    }

    fn mark_viewed(&self, session_id: &str) -> Option<AgentStatusEntry> {
        let mut inner = self.lock();
        let tracker = inner.panes.get_mut(session_id)?;
        tracker
            .machine
            .mark_viewed()
            .then(|| tracker.machine.entry(session_id))
    }

    fn set_focus(&self, session_id: Option<String>) -> Option<AgentStatusEntry> {
        self.lock().focused.clone_from(&session_id);
        self.mark_viewed(session_id.as_deref()?)
    }

    fn register_config_dir(&self, session_id: &str, dir: PathBuf, now: u64) {
        self.lock().registry.insert(session_id, dir, now);
    }

    /// True while a Codex pane runs a turn or waits for the user.
    fn codex_active(&self) -> bool {
        self.lock()
            .panes
            .values()
            .any(|tracker| tracker.agent == PaneAgent::Codex && tracker.machine.active())
    }

    /// The number of panes that wait for the user, when it changed since
    /// the last call.
    fn take_badge_change(&self) -> Option<usize> {
        let mut inner = self.lock();
        let count = inner
            .panes
            .values()
            .filter(|tracker| tracker.machine.needs_user())
            .count();
        (inner.badge_count != Some(count)).then(|| {
            inner.badge_count = Some(count);
            count
        })
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or_default()
}

fn rfc3339_ms(value: &str) -> u64 {
    chrono::DateTime::parse_from_rfc3339(value)
        .map(|time| time.timestamp_millis().max(0) as u64)
        .unwrap_or_default()
}

fn observations(app: &AppHandle, store: &AgentStatusStore) -> Vec<PaneObservation> {
    let Some(terminals) = app.try_state::<crate::terminal::TerminalManager>() else {
        return Vec::new();
    };
    terminals
        .list()
        .into_iter()
        .map(|session| {
            let running = session.status == "running";
            // Only a pane that the store does not track yet needs its seed.
            // The read copies the scrollback once, outside the store lock.
            let title_seed = (running
                && PaneAgent::from_agent_kind(session.agent_kind.as_deref()).is_some()
                && !store.tracks(&session.id))
            .then(|| terminals.remote_ring_snapshot(&session.id))
            .flatten()
            .and_then(|ring| osc::last_title(&ring.data));
            PaneObservation {
                running,
                failed: session.status == "failed",
                started_at: rfc3339_ms(&session.started_at),
                id: session.id,
                agent_kind: session.agent_kind,
                pid: session.pid,
                title_seed,
            }
        })
        .collect()
}

/// The folders where an adopted pane can keep its presence file.
fn fallback_config_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    for dir in [
        crate::wallet::claude_config_dir(),
        launch::system_config_dir(),
    ]
    .into_iter()
    .flatten()
    {
        if !dirs.contains(&dir) {
            dirs.push(dir);
        }
    }
    dirs
}

fn publish(app: &AppHandle, store: &AgentStatusStore, entries: Vec<AgentStatusEntry>) {
    for entry in entries {
        if let Err(error) = app.emit(AGENT_STATUS_EVENT, &entry) {
            eprintln!("agent status: could not emit a status: {error}");
        }
        // A send fails only when no subscriber exists.
        let _ = store.sender.send(entry);
    }
    if let Some(count) = store.take_badge_change() {
        update_badge(app, count);
    }
}

/// Shows the number of panes that wait for the user on the dock icon.
fn update_badge(app: &AppHandle, count: usize) {
    #[cfg(desktop)]
    if let Some(window) = app.get_webview_window("main") {
        // Some platforms have no badge. The count is a convenience there.
        let _ = window.set_badge_count((count > 0).then_some(count as i64));
    }
    #[cfg(not(desktop))]
    let _ = (app, count);
}

/// Prepares the hook folder: a private folder, a stable helper copy, and no
/// old journals. Returns the folder, or `None` when hooks cannot work.
fn prepare_hooks(state_dir: &Path) -> Option<PathBuf> {
    let hooks_dir = state_dir.join(HOOKS_DIR);
    if let Err(error) = hooks::create_private_dir(&hooks_dir) {
        eprintln!("agent status: no Claude hooks: {error}");
        return None;
    }
    hooks::remove_old_journals(&hooks_dir, SystemTime::now());
    #[cfg(unix)]
    match hooks::install_helper(&hooks_dir) {
        Ok(helper) => {
            let _ = HELPER_PATH.set(helper);
        }
        Err(error) => eprintln!("agent status: no Claude hooks: {error}"),
    }
    let _ = HOOKS_PATH.set(hooks_dir.clone());
    Some(hooks_dir)
}

/// Reads a pane journal as soon as the helper sends a wake datagram. The
/// poll still reads every journal, so a lost datagram costs one poll.
#[cfg(unix)]
fn spawn_wake_listener(app: AppHandle, store: AgentStatusStore, hooks_dir: &Path) {
    let socket = match hooks::bind_wake_socket(hooks_dir) {
        Ok(socket) => socket,
        Err(error) => {
            eprintln!("agent status: no wake socket: {error}");
            return;
        }
    };
    let spawned = thread::Builder::new()
        .name("halv-agent-hooks".to_string())
        .spawn(move || {
            let mut buffer = [0; 256];
            loop {
                let Ok(count) = socket.recv(&mut buffer) else {
                    thread::sleep(POLL_INTERVAL);
                    continue;
                };
                let Ok(session_id) = std::str::from_utf8(&buffer[..count]) else {
                    continue;
                };
                let changed = store.wake_pane(session_id, now_ms());
                publish(&app, &store, changed.into_iter().collect());
            }
        });
    if let Err(error) = spawned {
        eprintln!("agent status: could not start the wake thread: {error}");
    }
}

/// Starts the store and its poll thread. Call it after the terminal
/// manager adopts the supervised panes.
pub(crate) fn start(app: &AppHandle, app_data_dir: &Path) {
    let state_dir = app_data_dir.join(STATE_DIR);
    let hooks_dir = prepare_hooks(&state_dir);
    let store = AgentStatusStore::with_registry(
        ConfigDirRegistry::open(&state_dir, now_ms()),
        hooks_dir.clone(),
    );
    app.manage(store.clone());
    #[cfg(unix)]
    if let Some(hooks_dir) = &hooks_dir {
        spawn_wake_listener(app.clone(), store.clone(), hooks_dir);
    }
    let app = app.clone();
    let spawned = thread::Builder::new()
        .name("halv-agent-status".to_string())
        .spawn(move || loop {
            thread::sleep(POLL_INTERVAL);
            let panes = observations(&app, &store);
            let changed = store.observe(&panes, now_ms(), &DiskFiles, &fallback_config_dirs);
            publish(&app, &store, changed);
        });
    if let Err(error) = spawned {
        eprintln!("agent status: could not start the poll thread: {error}");
    }
}

/// The stable helper copy that the hook entries run.
fn helper_path() -> Option<&'static Path> {
    HELPER_PATH.get().map(PathBuf::as_path)
}

/// The variables that tie an agent PTY to its pane: the hook helper needs
/// them. Empty before `start` or when hooks cannot work.
pub(crate) fn pane_environment(session_id: &str) -> Vec<(OsString, OsString)> {
    let Some(dir) = HOOKS_PATH.get() else {
        return Vec::new();
    };
    vec![
        (OsString::from(SESSION_ID_ENV), OsString::from(session_id)),
        (
            OsString::from(STATUS_DIR_ENV),
            dir.as_os_str().to_os_string(),
        ),
    ]
}

/// The pane variables that a plain shell must not inherit from the app.
pub(crate) fn pane_variable_names() -> [OsString; 2] {
    [
        OsString::from(SESSION_ID_ENV),
        OsString::from(STATUS_DIR_ENV),
    ]
}

/// The current entries. Empty before `start`.
pub fn snapshot(app: &AppHandle) -> Vec<AgentStatusEntry> {
    app.try_state::<AgentStatusStore>()
        .map(|store| store.snapshot())
        .unwrap_or_default()
}

/// A receiver for each later change. `None` before `start`.
pub fn subscribe(app: &AppHandle) -> Option<broadcast::Receiver<AgentStatusEntry>> {
    app.try_state::<AgentStatusStore>()
        .map(|store| store.subscribe())
}

/// Records the Claude config folder of a new agent pane from its launch
/// environment.
pub(crate) fn register_launch(
    app: &AppHandle,
    session_id: &str,
    agent_kind: &str,
    env: &[(OsString, OsString)],
    env_remove: &[OsString],
) {
    if PaneAgent::from_agent_kind(Some(agent_kind)) != Some(PaneAgent::ClaudeFamily) {
        return;
    }
    let (Some(store), Some(dir)) = (
        app.try_state::<AgentStatusStore>(),
        launch::claude_config_dir_for_launch(env, env_remove),
    ) else {
        return;
    };
    store.register_config_dir(session_id, dir, now_ms());
}

/// Feeds a Codex rollout turn event to the pane status. `recorded_at` is the
/// rollout time (Unix ms) of an event from the first read of a rollout.
pub(crate) fn note_rollout_turn(
    app: &AppHandle,
    session_id: &str,
    turn: RolloutTurn,
    recorded_at: Option<u64>,
) {
    let Some(store) = app.try_state::<AgentStatusStore>() else {
        return;
    };
    let changed = store.note_rollout(session_id, PaneInput::Rollout(turn), now_ms(), recorded_at);
    publish(app, &store, changed.into_iter().collect());
}

/// Records a Codex tool result that the rollout wrote at `at` (Unix ms).
pub(crate) fn note_rollout_activity(app: &AppHandle, session_id: &str, at: u64) {
    let Some(store) = app.try_state::<AgentStatusStore>() else {
        return;
    };
    let changed = store.note_rollout(
        session_id,
        PaneInput::RolloutActivity { at },
        now_ms(),
        None,
    );
    publish(app, &store, changed.into_iter().collect());
}

/// True while a Codex pane runs a turn or waits. The rollout watcher polls
/// faster then.
pub(crate) fn codex_active(app: &AppHandle) -> bool {
    app.try_state::<AgentStatusStore>()
        .is_some_and(|store| store.codex_active())
}

/// Scans the live PTY output of a tracked pane for its title and, in a Codex
/// pane, for its notifications. Replayed output is history and must not
/// come here.
pub(crate) fn note_terminal_output(app: &AppHandle, session_id: &str, data: &str) {
    let Some(store) = app.try_state::<AgentStatusStore>() else {
        return;
    };
    let inputs = store.scan_output(session_id, data);
    if inputs.is_empty() {
        return;
    }
    let changed = store.note_terminal_signals(session_id, inputs, now_ms());
    publish(app, &store, changed.into_iter().collect());
}

/// The key in `data` that can answer a Codex prompt. Enter and Esc close
/// each prompt. The approval keys y, a, n, and the digits pick an option.
/// Focus reports, mouse reports, and arrow keys are longer escape sequences.
fn answer_key(data: &str) -> Option<AnswerKey> {
    if data == ESCAPE_KEY || data.contains(['\r', '\n']) {
        return Some(AnswerKey::Submit);
    }
    let [key] = data.as_bytes() else {
        return None;
    };
    matches!(key.to_ascii_lowercase(), b'y' | b'a' | b'n' | b'1'..=b'9')
        .then_some(AnswerKey::Choice)
}

/// True when `data` holds Enter outside a bracketed paste, so it can send a
/// prompt. A line break in a paste, and Alt or Esc with Enter, do not send
/// the prompt. A single letter does not count either: the user can type it
/// while the agent starts.
fn sends_prompt(data: &str) -> bool {
    let typed = data
        .rfind(PASTE_END)
        .map_or(data, |end| &data[end + PASTE_END.len()..]);
    !typed.starts_with(ESCAPE_KEY) && typed.contains(['\r', '\n'])
}

/// The parts of one user input that agent status reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct InputSignal {
    escape: bool,
    answer: Option<AnswerKey>,
    /// The input can send a prompt. Work in the title after it is a turn.
    prompt: bool,
}

/// The signal in `data`, or `None` when the input does not matter to agent
/// status. The terminal reads it before it queues the input.
pub(crate) fn input_signal(data: &str) -> Option<InputSignal> {
    let escape = data == ESCAPE_KEY;
    let answer = answer_key(data);
    let prompt = sends_prompt(data);
    (escape || answer.is_some()).then_some(InputSignal {
        escape,
        answer,
        prompt,
    })
}

/// Records user input that reached the PTY. Esc shortly before a turn rests
/// means the user stopped the turn. An answer key ends a Codex wait. A
/// prompt key lets the title start a turn.
pub(crate) fn note_terminal_input(app: &AppHandle, session_id: &str, signal: InputSignal) {
    let Some(store) = app.try_state::<AgentStatusStore>() else {
        return;
    };
    let changed = store.note_input(session_id, signal, now_ms());
    publish(app, &store, changed.into_iter().collect());
}

#[tauri::command]
pub fn agent_status_snapshot(store: tauri::State<'_, AgentStatusStore>) -> Vec<AgentStatusEntry> {
    store.snapshot()
}

/// Marks the rested turns of a pane as seen.
#[tauri::command]
pub fn agent_status_mark_viewed(
    app: AppHandle,
    store: tauri::State<'_, AgentStatusStore>,
    session_id: String,
) {
    let changed = store.mark_viewed(&session_id);
    publish(&app, &store, changed.into_iter().collect());
}

/// Names the pane that the user looks at in a focused window, or none. A
/// turn that rests in that pane is not unread.
#[tauri::command]
pub fn agent_status_set_focus(
    app: AppHandle,
    store: tauri::State<'_, AgentStatusStore>,
    session_id: Option<String>,
) {
    let changed = store.set_focus(session_id);
    publish(&app, &store, changed.into_iter().collect());
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    struct Scratch(PathBuf);

    impl Scratch {
        fn new(label: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "halv-agent-status-{label}-{}-{}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            fs::create_dir_all(path.join("sessions")).unwrap();
            Self(path)
        }

        fn write(&self, pid: u32, started_at: u64, status: &str, waiting_for: Option<&str>) {
            let mut value = serde_json::json!({
                "pid": pid,
                "startedAt": started_at,
                "status": status,
            });
            if let Some(waiting_for) = waiting_for {
                value["waitingFor"] = serde_json::json!(waiting_for);
            }
            let path = presence::presence_path(&self.0, pid);
            // Each write grows the file, so the stat check sees a change.
            let body = value.to_string();
            let previous = fs::metadata(&path).map_or(0, |metadata| metadata.len() as usize);
            let padding = " ".repeat((previous + 1).saturating_sub(body.len()));
            fs::write(path, format!("{body}{padding}")).unwrap();
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn pane(id: &str, agent_kind: &str, pid: u32) -> PaneObservation {
        PaneObservation {
            id: id.to_string(),
            agent_kind: Some(agent_kind.to_string()),
            pid,
            running: true,
            failed: false,
            started_at: 1_000_000,
            title_seed: None,
        }
    }

    fn statuses(entries: &[AgentStatusEntry]) -> Vec<(String, AgentStatus)> {
        entries
            .iter()
            .map(|entry| (entry.session_id.clone(), entry.status))
            .collect()
    }

    #[test]
    fn every_agent_that_halv_launches_is_tracked() {
        let store = AgentStatusStore::default();
        let mut shell = pane("shell", "claude", 1);
        shell.agent_kind = None;
        let changed = store.observe(
            &[
                pane("claude", "claude", 11),
                pane("glm", "zai", 12),
                pane("grok", "grok", 13),
                pane("codex", "codex", 14),
                pane("kimi", "kimi", 15),
                pane("gemini", "gemini", 16),
                pane("agy", "antigravity", 17),
                pane("aider", "aider", 18),
                shell,
            ],
            5,
            &DiskFiles,
            &Vec::new,
        );
        assert_eq!(
            statuses(&changed),
            [
                ("claude".to_string(), AgentStatus::Idle),
                ("glm".to_string(), AgentStatus::Idle),
                ("grok".to_string(), AgentStatus::Idle),
                ("codex".to_string(), AgentStatus::Idle),
                ("kimi".to_string(), AgentStatus::Idle),
                ("gemini".to_string(), AgentStatus::Idle),
                ("agy".to_string(), AgentStatus::Idle),
            ]
        );
        assert!(changed
            .iter()
            .all(|entry| entry.source == StatusSource::None));
        assert_eq!(store.snapshot().len(), 7);
        // Each running tracked pane gets an output scanner.
        assert_eq!(store.lock_scanners().len(), 7);
        assert!(store.tracks("kimi"));
        assert!(!store.tracks("aider"));
    }

    #[test]
    fn the_recorded_folder_drives_a_claude_pane() {
        let scratch = Scratch::new("recorded");
        let store = AgentStatusStore::default();
        store.register_config_dir("pane", scratch.0.clone(), 1);
        let panes = [pane("pane", "claude", 4311)];
        store.observe(&panes, 1, &DiskFiles, &Vec::new);

        scratch.write(4311, 1_000_100, "busy", None);
        let changed = store.observe(&panes, 2, &DiskFiles, &Vec::new);
        assert_eq!(
            statuses(&changed),
            [("pane".to_string(), AgentStatus::Working)]
        );
        assert_eq!(changed[0].source, StatusSource::Presence);
        assert!(store.observe(&panes, 3, &DiskFiles, &Vec::new).is_empty());

        scratch.write(4311, 1_000_100, "waiting", Some("permission prompt"));
        let changed = store.observe(&panes, 4, &DiskFiles, &Vec::new);
        assert_eq!(changed[0].status, AgentStatus::NeedsApproval);
        assert_eq!(store.take_badge_change(), Some(1));
        assert_eq!(store.take_badge_change(), None);

        scratch.write(4311, 1_000_100, "idle", None);
        let changed = store.observe(&panes, 5, &DiskFiles, &Vec::new);
        assert_eq!(changed[0].status, AgentStatus::Done);
        assert!(changed[0].unread);
        assert_eq!(store.take_badge_change(), Some(0));

        let viewed = store.mark_viewed("pane").expect("unread changes");
        assert!(!viewed.unread);
        assert!(store.mark_viewed("pane").is_none());
    }

    #[test]
    fn an_adopted_pane_finds_its_folder_among_the_fallbacks() {
        let empty = Scratch::new("fallback-empty");
        let account = Scratch::new("fallback-account");
        let store = AgentStatusStore::default();
        let panes = [pane("adopted", "claude", 52)];
        account.write(52, 1_000_000, "busy", None);
        let fallbacks = || vec![empty.0.clone(), account.0.clone()];
        let changed = store.observe(&panes, 1, &DiskFiles, &fallbacks);
        assert_eq!(
            statuses(&changed),
            [("adopted".to_string(), AgentStatus::Working)]
        );
        assert_eq!(
            store.lock().registry.get("adopted"),
            Some(account.0.as_path())
        );
        // A file for this PID in another folder no longer matters.
        empty.write(52, 1_000_000, "idle", None);
        assert!(store.observe(&panes, 2, &DiskFiles, &fallbacks).is_empty());
    }

    #[test]
    fn the_fallback_folders_load_once_and_only_for_unrecorded_panes() {
        let scratch = Scratch::new("fallback-once");
        let store = AgentStatusStore::default();
        store.register_config_dir("recorded", scratch.0.clone(), 1);
        let calls = std::cell::Cell::new(0);
        let fallbacks = || {
            calls.set(calls.get() + 1);
            Vec::new()
        };
        let panes = [pane("recorded", "claude", 61), pane("adopted", "grok", 62)];
        for now in 1..=4 {
            store.observe(&panes, now, &DiskFiles, &fallbacks);
        }
        assert_eq!(calls.get(), 1);
        // A new process in the pane starts the search again.
        let restarted = [pane("adopted", "grok", 63)];
        store.observe(&restarted, 5, &DiskFiles, &fallbacks);
        assert_eq!(calls.get(), 2);
    }

    #[test]
    fn the_focused_pane_rests_as_read() {
        let scratch = Scratch::new("focused");
        let store = AgentStatusStore::default();
        store.register_config_dir("pane", scratch.0.clone(), 1);
        let panes = [pane("pane", "claude", 77)];
        scratch.write(77, 1_000_000, "busy", None);
        store.observe(&panes, 1, &DiskFiles, &Vec::new);
        assert!(store.set_focus(Some("pane".to_string())).is_none());
        scratch.write(77, 1_000_000, "idle", None);
        let changed = store.observe(&panes, 2, &DiskFiles, &Vec::new);
        assert_eq!(changed[0].status, AgentStatus::Done);
        assert!(!changed[0].unread);

        store.set_focus(None);
        scratch.write(77, 1_000_000, "busy", None);
        store.observe(&panes, 3, &DiskFiles, &Vec::new);
        scratch.write(77, 1_000_000, "idle", None);
        assert!(store.observe(&panes, 4, &DiskFiles, &Vec::new)[0].unread);
        // Focus on an unread pane reads it.
        let read = store.set_focus(Some("pane".to_string())).expect("read");
        assert!(!read.unread);
    }

    #[test]
    fn exits_and_removals_follow_the_terminal_list() {
        let store = AgentStatusStore::default();
        store.register_config_dir("pane", PathBuf::from("/nowhere"), 1);
        let mut exited = pane("pane", "codex", 9);
        store.observe(std::slice::from_ref(&exited), 1, &DiskFiles, &Vec::new);
        exited.running = false;
        exited.failed = true;
        let changed = store.observe(std::slice::from_ref(&exited), 2, &DiskFiles, &Vec::new);
        assert_eq!(changed[0].status, AgentStatus::Exited);
        assert_eq!(changed[0].reason.as_deref(), Some("error"));
        assert!(store
            .observe(std::slice::from_ref(&exited), 3, &DiskFiles, &Vec::new)
            .is_empty());

        store.set_focus(Some("pane".to_string()));
        store.observe(&[], 4, &DiskFiles, &Vec::new);
        assert!(store.snapshot().is_empty());
        assert!(store.lock().focused.is_none());
        assert!(store.lock().registry.get("pane").is_none());
    }

    #[test]
    fn codex_rollout_turns_and_escape_update_the_pane() {
        let store = AgentStatusStore::default();
        let started = store
            .note_rollout("codex", PaneInput::Rollout(RolloutTurn::Started), 10, None)
            .expect("working");
        assert_eq!(started.status, AgentStatus::Working);
        assert_eq!(started.source, StatusSource::Rollout);
        let finished = store
            .note_rollout(
                "codex",
                PaneInput::Rollout(RolloutTurn::Finished { aborted: false }),
                20,
                None,
            )
            .expect("done");
        assert_eq!(finished.status, AgentStatus::Done);
        assert!(finished.unread);
        assert!(store
            .note_rollout(
                "codex",
                PaneInput::Rollout(RolloutTurn::Finished { aborted: false }),
                30,
                None,
            )
            .is_none());
        // The poll keeps the rollout status of a running Codex pane.
        assert!(store
            .observe(&[pane("codex", "codex", 3)], 40, &DiskFiles, &Vec::new)
            .is_empty());
        assert!(store.note_input("missing", key("\u{1b}"), 50).is_none());
    }

    #[test]
    fn subscribers_receive_the_published_payload() {
        let store = AgentStatusStore::default();
        let mut receiver = store.subscribe();
        let entry = store
            .note_rollout("codex", PaneInput::Rollout(RolloutTurn::Started), 10, None)
            .expect("working");
        store.sender.send(entry.clone()).unwrap();
        assert_eq!(receiver.try_recv().unwrap(), entry);
    }

    #[test]
    fn terminal_start_times_parse_to_milliseconds() {
        assert_eq!(rfc3339_ms("1970-01-01T00:00:01.500Z"), 1_500);
        assert_eq!(rfc3339_ms("not a time"), 0);
    }

    const PANE_ID: &str = "7a6b5c4d-3e2f-4a1b-8c9d-0e1f2a3b4c5d";

    fn journal_line(dir: &Path, event: &str, at: u64, extra: impl FnOnce(&mut HookRecord)) {
        let mut record = HookRecord {
            v: 1,
            at,
            pane: PANE_ID.to_string(),
            agent: "claude".to_string(),
            event: event.to_string(),
            ..HookRecord::default()
        };
        extra(&mut record);
        let line = serde_json::to_string(&record).unwrap();
        halv_supervisor::agent_hook::append_journal(dir, PANE_ID, &line).unwrap();
    }

    fn hooked_store(scratch: &Scratch) -> AgentStatusStore {
        let hooks_dir = scratch.0.join("hooks");
        fs::create_dir_all(&hooks_dir).unwrap();
        AgentStatusStore::with_registry(ConfigDirRegistry::default(), Some(hooks_dir))
    }

    #[test]
    fn hook_journals_drive_a_pane_and_old_lines_count_as_read() {
        let scratch = Scratch::new("journal");
        let store = hooked_store(&scratch);
        let hooks_dir = scratch.0.join("hooks");
        // An earlier app run saw a finished turn.
        journal_line(&hooks_dir, "UserPromptSubmit", 100, |_| {});
        journal_line(&hooks_dir, "Stop", 200, |_| {});
        let panes = [pane(PANE_ID, "claude", 41)];
        let changed = store.observe(&panes, 5_000, &DiskFiles, &Vec::new);
        assert_eq!(changed.len(), 1);
        assert_eq!(changed[0].status, AgentStatus::Done);
        assert_eq!(changed[0].source, StatusSource::Hook);
        assert!(!changed[0].unread);
        assert_eq!(changed[0].updated_at, 200 + machine::REST_GRACE_MS);

        // A new turn in this run is news.
        journal_line(&hooks_dir, "UserPromptSubmit", 5_100, |_| {});
        journal_line(&hooks_dir, "PreToolUse", 5_150, |record| {
            record.tool = Some("Bash".to_string());
            record.tool_use_id = Some("t1".to_string());
        });
        journal_line(&hooks_dir, "PermissionRequest", 5_200, |record| {
            record.tool = Some("Bash".to_string());
            record.command = Some("make".to_string());
        });
        let entry = store.wake_pane(PANE_ID, 5_300).expect("the wait changes");
        assert_eq!(entry.status, AgentStatus::NeedsApproval);
        assert_eq!(
            entry.prompt,
            Some(PromptDetail::Approval {
                tool_name: "Bash".to_string(),
                command: Some("make".to_string()),
                file_path: None,
                message: None,
            })
        );
        assert_eq!(store.take_badge_change(), Some(1));
        journal_line(&hooks_dir, "PostToolUse", 5_400, |record| {
            record.tool = Some("Bash".to_string());
            record.tool_use_id = Some("t1".to_string());
        });
        journal_line(&hooks_dir, "Stop", 5_500, |_| {});
        let changed = store.observe(&panes, 5_600, &DiskFiles, &Vec::new);
        // Without a presence file, the Stop waits for the grace time.
        assert_eq!(changed[0].status, AgentStatus::Working);
        let done_at = 5_600 + machine::REST_GRACE_MS;
        let changed = store.observe(&panes, done_at, &DiskFiles, &Vec::new);
        assert_eq!(changed[0].status, AgentStatus::Done);
        assert!(changed[0].unread);
        assert!(store.wake_pane(PANE_ID, done_at + 100).is_none());
        assert!(store.wake_pane("unknown", done_at + 100).is_none());

        // A removed pane takes its journal along.
        store.observe(&[], done_at + 200, &DiskFiles, &Vec::new);
        assert!(!hooks_dir.join(format!("{PANE_ID}.jsonl")).exists());
    }

    #[test]
    fn a_long_journal_replays_whole_before_the_presence_file() {
        let scratch = Scratch::new("long-replay");
        let store = hooked_store(&scratch);
        let hooks_dir = scratch.0.join("hooks");
        store.register_config_dir(PANE_ID, scratch.0.clone(), 1);
        // An earlier app run: many turns, more than one read of the tail.
        let mut at = 100;
        let message = "x".repeat(150);
        while fs::metadata(hooks_dir.join(format!("{PANE_ID}.jsonl")))
            .map_or(0, |metadata| metadata.len())
            < 700 * 1024
        {
            journal_line(&hooks_dir, "UserPromptSubmit", at, |_| {});
            for index in 0..20 {
                let id = format!("t{at}-{index}");
                journal_line(&hooks_dir, "PreToolUse", at + 1, |record| {
                    record.tool = Some("Bash".to_string());
                    record.tool_use_id = Some(id.clone());
                    record.command = Some(message.clone());
                });
                journal_line(&hooks_dir, "PostToolUse", at + 2, |record| {
                    record.tool = Some("Bash".to_string());
                    record.tool_use_id = Some(id.clone());
                });
            }
            journal_line(&hooks_dir, "Stop", at + 3, |_| {});
            at += 10;
        }
        // Claude is idle now.
        scratch.write(43, 1_000_000, "idle", None);
        let panes = [pane(PANE_ID, "claude", 43)];
        let changed = store.observe(&panes, 1_000_000, &DiskFiles, &Vec::new);
        assert_eq!(changed[0].status, AgentStatus::Done);
        assert!(!changed[0].unread);
        assert!(store
            .observe(&panes, 1_060_000, &DiskFiles, &Vec::new)
            .is_empty());
    }

    #[test]
    fn a_hooked_pane_with_a_presence_file_waits_for_a_late_stop() {
        let scratch = Scratch::new("late-stop");
        let store = hooked_store(&scratch);
        let hooks_dir = scratch.0.join("hooks");
        store.register_config_dir(PANE_ID, scratch.0.clone(), 1);
        let panes = [pane(PANE_ID, "claude", 42)];
        journal_line(&hooks_dir, "SessionStart", 2_000, |_| {});
        scratch.write(42, 1_000_000, "busy", None);
        store.observe(&panes, 2_100, &DiskFiles, &Vec::new);
        scratch.write(42, 1_000_000, "idle", None);
        let changed = store.observe(&panes, 2_400, &DiskFiles, &Vec::new);
        assert!(changed.is_empty(), "the rest waits for a Stop event");
        let changed = store.observe(
            &panes,
            2_400 + machine::REST_GRACE_MS,
            &DiskFiles,
            &Vec::new,
        );
        assert_eq!(changed[0].status, AgentStatus::Interrupted);
    }

    #[test]
    fn codex_notifications_and_answer_keys_drive_codex_waits() {
        let store = AgentStatusStore::default();
        let panes = [pane("codex", "codex", 7)];
        store.observe(&panes, 1, &DiskFiles, &Vec::new);
        assert!(store
            .scan_output("other", "\u{1b}]9;Question: x\u{7}")
            .is_empty());
        let notices = store.scan_output("codex", "\u{1b}]9;Approval requested: rm -rf target\u{7}");
        assert_eq!(notices.len(), 1);
        let entry = store
            .note_terminal_signals("codex", notices, 2)
            .expect("the wait changes");
        assert_eq!(entry.status, AgentStatus::NeedsApproval);
        assert_eq!(entry.source, StatusSource::Terminal);
        assert!(store.codex_active());
        // An arrow key does not answer; Enter does.
        assert!(store.note_input("codex", key("\u{1b}[B"), 3).is_none());
        let entry = store
            .note_input("codex", key("\r"), 4)
            .expect("the answer changes");
        assert_eq!(entry.status, AgentStatus::Working);

        // A closed Codex pane loses its scanner.
        store.observe(&[], 5, &DiskFiles, &Vec::new);
        assert!(store.lock_scanners().is_empty());
    }

    #[test]
    fn answer_keys_are_enter_esc_and_approval_choices() {
        for key in ["\r", "\n", "\u{1b}", "paste\r"] {
            assert_eq!(answer_key(key), Some(AnswerKey::Submit), "{key:?}");
        }
        for key in ["y", "A", "n", "1", "9"] {
            assert_eq!(answer_key(key), Some(AnswerKey::Choice), "{key:?}");
        }
        for key in [
            "\u{1b}[A",
            "\u{1b}[I",
            "\u{1b}[O",
            "\u{1b}[<0;1;1M",
            "",
            " ",
            "ab",
            "m",
            "0",
        ] {
            assert_eq!(answer_key(key), None, "{key:?}");
        }
    }

    #[test]
    fn only_esc_and_answer_keys_give_an_input_signal() {
        assert_eq!(
            input_signal("\u{1b}"),
            Some(InputSignal {
                escape: true,
                answer: Some(AnswerKey::Submit),
                prompt: false,
            })
        );
        assert_eq!(
            input_signal("y"),
            Some(InputSignal {
                escape: false,
                answer: Some(AnswerKey::Choice),
                prompt: false,
            })
        );
        assert_eq!(
            input_signal("\r"),
            Some(InputSignal {
                escape: false,
                answer: Some(AnswerKey::Submit),
                prompt: true,
            })
        );
        for data in ["\u{1b}[A", "ls", "m", ""] {
            assert_eq!(input_signal(data), None, "{data:?}");
        }
    }

    #[test]
    fn only_enter_outside_a_paste_sends_a_prompt() {
        for data in [
            "\r",
            "\n",
            "fix the parser\r",
            "\u{1b}[200~fix the parser\u{1b}[201~\r",
        ] {
            assert!(sends_prompt(data), "{data:?}");
        }
        for data in [
            "\u{1b}",
            "\u{1b}\r",
            "\u{1b}[200~one\ntwo\u{1b}[201~",
            "\u{1b}[200~one\ntwo",
            "y",
            "1",
            "ls",
        ] {
            assert!(!sends_prompt(data), "{data:?}");
        }
    }

    #[test]
    fn a_rollout_turn_from_before_the_pane_was_tracked_is_read() {
        let store = AgentStatusStore::default();
        store.observe(&[pane("codex", "codex", 3)], 10_000, &DiskFiles, &Vec::new);
        // The first read after an app restart finds the last finished turn.
        store.note_rollout(
            "codex",
            PaneInput::Rollout(RolloutTurn::Started),
            10_100,
            Some(4_000),
        );
        let entry = store
            .note_rollout(
                "codex",
                PaneInput::Rollout(RolloutTurn::Finished { aborted: false }),
                10_100,
                Some(5_000),
            )
            .expect("the history rebuilds the status");
        assert_eq!(entry.status, AgentStatus::Done);
        assert!(!entry.unread);

        // A turn that finished after the pane was tracked is news.
        store.note_rollout(
            "codex",
            PaneInput::Rollout(RolloutTurn::Started),
            11_000,
            Some(10_500),
        );
        let entry = store
            .note_rollout(
                "codex",
                PaneInput::Rollout(RolloutTurn::Finished { aborted: false }),
                11_000,
                Some(10_900),
            )
            .expect("done");
        assert!(entry.unread);
    }

    /// The input signal of `data`, also for input that gives none.
    fn key(data: &str) -> InputSignal {
        input_signal(data).unwrap_or(InputSignal {
            escape: false,
            answer: None,
            prompt: false,
        })
    }

    /// Feeds live output to a pane, as the PTY reader does.
    fn output(
        store: &AgentStatusStore,
        id: &str,
        data: &str,
        now: u64,
    ) -> Option<AgentStatusEntry> {
        let inputs = store.scan_output(id, data);
        store.note_terminal_signals(id, inputs, now)
    }

    fn osc_title(text: &str) -> String {
        format!("\u{1b}]0;{text}\u{7}")
    }

    #[test]
    fn the_title_drives_a_claude_pane_without_hooks_or_presence() {
        // A pane from an older Halv: no hooks, and no presence file in the
        // folders that the store tries.
        let store = AgentStatusStore::default();
        let panes = [pane("old", "claude", 91)];
        store.observe(&panes, 1_000, &DiskFiles, &Vec::new);

        // The user sends a prompt. The key reaches the store after the PTY
        // write, so the title of the work can come first.
        output(&store, "old", &osc_title("\u{25d0} Fix the parser"), 2_000);
        let entry = store
            .note_input("old", key("\r"), 2_010)
            .expect("Enter makes the work a turn");
        assert_eq!(entry.status, AgentStatus::Working);
        assert_eq!(entry.source, StatusSource::Title);
        assert_eq!(entry.turn_seq, 1);
        // The next spinner frame is the same state. The chunk can split the
        // title anywhere.
        assert!(output(&store, "old", "\u{1b}]0;\u{25d1} Fix the", 2_900).is_none());
        assert!(output(&store, "old", " parser\u{7}", 2_901).is_none());
        // A poll does not end the title state.
        assert!(store
            .observe(&panes, 3_000, &DiskFiles, &Vec::new)
            .is_empty());

        let entry = output(&store, "old", &osc_title("\u{2733} Fix the parser"), 4_000)
            .expect("the rest ends the turn");
        assert_eq!(entry.status, AgentStatus::Done);
        assert!(entry.unread);
        assert_eq!(entry.source, StatusSource::Title);
    }

    #[test]
    fn a_title_in_a_codex_pane_shows_its_approval_wait() {
        let store = AgentStatusStore::default();
        store.observe(&[pane("codex", "codex", 92)], 1_000, &DiskFiles, &Vec::new);
        store.note_input("codex", key("\r"), 1_050);
        output(&store, "codex", &osc_title("\u{280b} scratch"), 1_100);
        let entry = output(
            &store,
            "codex",
            "\u{1b}]2;[ . ] Action Required | \u{280b} | scratch\u{1b}\\",
            1_200,
        )
        .expect("the wait starts");
        assert_eq!(entry.status, AgentStatus::NeedsApproval);
        assert_eq!(entry.source, StatusSource::Title);
        assert_eq!(entry.wait_seq, 1);
        // The blinking marker is the same wait.
        assert!(output(
            &store,
            "codex",
            &osc_title("[ ! ] Action Required | Create approved.txt | scratch"),
            1_300,
        )
        .is_none());
        let entry = output(&store, "codex", &osc_title("scratch"), 1_400).expect("the rest");
        assert_eq!(entry.status, AgentStatus::Done);
    }

    #[test]
    fn a_fresh_hook_journal_hides_the_title() {
        let scratch = Scratch::new("title-hooks");
        let store = hooked_store(&scratch);
        let hooks_dir = scratch.0.join("hooks");
        let panes = [pane(PANE_ID, "claude", 93)];
        store.observe(&panes, 5_000, &DiskFiles, &Vec::new);
        journal_line(&hooks_dir, "UserPromptSubmit", 5_100, |_| {});
        store
            .wake_pane(PANE_ID, 5_100)
            .expect("the hook starts a turn");
        // Claude rests its title at a permission prompt, too. The fresh
        // hooks own the pane.
        assert!(output(&store, PANE_ID, &osc_title("\u{2733} Fix it"), 5_200).is_none());
        let entry = &store.snapshot()[0];
        assert_eq!(entry.status, AgentStatus::Working);
        assert_eq!(entry.source, StatusSource::Hook);
    }

    #[test]
    fn the_scrollback_title_seeds_an_adopted_pane_as_read() {
        let store = AgentStatusStore::default();
        let mut resting = pane("resting", "claude", 94);
        resting.title_seed = Some("\u{2733} Fix the parser".to_string());
        let mut working = pane("working", "zai", 95);
        working.title_seed = Some("\u{25d0} Fix the parser".to_string());
        let mut plain = pane("plain", "claude", 96);
        plain.title_seed = Some("zsh".to_string());
        // The panes started long before the store tracks them. The spinner
        // shows a turn in progress.
        let now = 1_000_000 + PANE_START_MS;
        let changed = store.observe(&[resting, working, plain], now, &DiskFiles, &Vec::new);
        let by_id = |id: &str| changed.iter().find(|entry| entry.session_id == id).unwrap();
        assert_eq!(by_id("resting").status, AgentStatus::Idle);
        assert_eq!(by_id("resting").source, StatusSource::Title);
        assert_eq!(by_id("working").status, AgentStatus::Working);
        assert_eq!(by_id("working").source, StatusSource::Title);
        assert_eq!(by_id("working").turn_seq, 1);
        assert!(!by_id("working").unread);
        // A title that says nothing about the agent leaves the pane unreported.
        assert_eq!(by_id("plain").source, StatusSource::None);

        // A seed applies only once, when the store starts to track the pane.
        let mut later = pane("resting", "claude", 94);
        later.title_seed = Some("\u{25d0} Fix the parser".to_string());
        assert!(store
            .observe(&[later], now + 1_000, &DiskFiles, &Vec::new)
            .is_empty());
    }

    /// The titles of Codex 0.160.1 at start with no input, from the Orca
    /// capture `codex-fullscreen-startup`.
    const CODEX_START_TITLES: [&str; 10] = [
        "workspace-xxxxxxxxxxxxxx",
        "\u{2839} workspace-xxxxxxxxxxxxxx",
        "\u{2838} workspace-xxxxxxxxxxxxxx",
        "\u{283c} workspace-xxxxxxxxxxxxxx",
        "\u{2834} workspace-xxxxxxxxxxxxxx",
        "\u{2826} workspace-xxxxxxxxxxxxxx",
        "\u{2827} workspace-xxxxxxxxxxxxxx",
        "\u{2807} workspace-xxxxxxxxxxxxxx",
        "\u{280f} workspace-xxxxxxxxxxxxxx",
        "workspace-xxxxxxxxxxxxxx",
    ];

    #[test]
    fn the_codex_start_animation_is_not_a_turn() {
        let store = AgentStatusStore::default();
        store.observe(&[pane("codex", "codex", 98)], 1_000, &DiskFiles, &Vec::new);
        let mut seen = Vec::new();
        for (index, text) in CODEX_START_TITLES.iter().enumerate() {
            let now = 2_000 + 80 * index as u64;
            seen.extend(output(&store, "codex", &osc_title(text), now));
        }
        // The pane shows the start work, then rests. Nothing finished.
        assert!(seen
            .iter()
            .all(|entry| matches!(entry.status, AgentStatus::Idle | AgentStatus::Working)));
        let entry = &store.snapshot()[0];
        assert_eq!(entry.status, AgentStatus::Idle);
        assert_eq!(entry.source, StatusSource::Title);
        assert_eq!((entry.turn_seq, entry.wait_seq), (0, 0));
        assert!(!entry.unread);
        assert!(!store.codex_active());

        // The first prompt is a turn.
        store.note_input("codex", key("\r"), 5_000);
        output(
            &store,
            "codex",
            &osc_title("\u{280b} workspace-xxxxxxxxxxxxxx"),
            5_020,
        );
        let entry = output(
            &store,
            "codex",
            &osc_title("workspace-xxxxxxxxxxxxxx"),
            9_000,
        )
        .expect("the turn ends");
        assert_eq!(entry.status, AgentStatus::Done);
        assert_eq!(entry.turn_seq, 1);
        assert!(entry.unread);
    }

    #[test]
    fn a_new_pane_does_not_seed_a_turn_from_its_start_animation() {
        let store = AgentStatusStore::default();
        let mut young = pane("young", "codex", 99);
        young.started_at = 1_000;
        young.title_seed = Some(CODEX_START_TITLES[1].to_string());
        let changed = store.observe(&[young], 1_000 + PANE_START_MS - 1, &DiskFiles, &Vec::new);
        assert_eq!(changed[0].status, AgentStatus::Working);
        assert_eq!(changed[0].turn_seq, 0);
        let entry = output(&store, "young", &osc_title(CODEX_START_TITLES[9]), 20_000)
            .expect("the start ends");
        assert_eq!(entry.status, AgentStatus::Idle);
        assert!(!entry.unread);
    }

    #[test]
    fn the_presence_file_wins_over_the_title() {
        let scratch = Scratch::new("title-presence");
        let store = AgentStatusStore::default();
        store.register_config_dir("pane", scratch.0.clone(), 1);
        let panes = [pane("pane", "claude", 97)];
        scratch.write(97, 1_000_000, "waiting", Some("permission prompt"));
        store.observe(&panes, 1, &DiskFiles, &Vec::new);
        assert!(output(&store, "pane", &osc_title("\u{2733} Fix it"), 2).is_none());
        assert_eq!(store.snapshot()[0].status, AgentStatus::NeedsApproval);
        assert_eq!(store.snapshot()[0].source, StatusSource::Presence);
    }
}
