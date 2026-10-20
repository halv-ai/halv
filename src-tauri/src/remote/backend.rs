//! The terminal side of the gateway: a [`HostBackend`] over `TerminalManager`.
//!
//! - The pane list comes from the live sessions of `TerminalManager` (section 8.6).
//! - The output comes from the terminal tap, with the sequence numbers of the ring.
//! - Snapshots and replays come from the app-side ring (section 8.8).
//! - Input goes through the same backend as the `terminal_write` command.
//!   Input with a line break also updates the conversation time, like `terminal_write`.
//! - Claude Code and Codex CLI panes always use bracketed paste (section 8.8).
//!
//! The engine calls `snapshot`, `replay_since`, `write`, `bracketed_paste`, and `panes`
//! in `spawn_blocking`.
//! The tap runs on terminal reader threads, so it only sends to channels and never waits.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

use halv_remote::host::{Chunk, HostBackend, OutputEvent, Replay, Snapshot, WriteError};
use halv_remote::rpc::{AgentStatus, HostInfo, Pane, PaneKind, PaneState};
use tokio::sync::broadcast;

use crate::terminal::{
    RemoteRing, TerminalManager, TerminalSession, TerminalTap, TerminalTapEvent,
};

/// The capacity of the output channel. A slow reader gets a new snapshot when it falls behind.
const OUTPUT_CHANNEL_CAPACITY: usize = 1024;
/// The capacity of the pane change channel. The engine merges queued changes.
const PANE_CHANGE_CAPACITY: usize = 16;
/// Output alone changes the pane list (`lastOutputAt`) at most this often for one pane.
const OUTPUT_NOTICE_INTERVAL: Duration = Duration::from_secs(10);
/// Remote input updates one conversation at most this often.
/// It follows `CONVERSATION_ACTIVITY_THROTTLE` of `terminal_write`.
const CONVERSATION_ACTIVITY_THROTTLE: Duration = Duration::from_secs(5);

/// The terminal sessions of the app. `TerminalManager` is the real source. Tests use a fake.
pub(crate) trait TerminalSource: Send + Sync + 'static {
    /// Returns the live sessions.
    fn sessions(&self) -> Vec<TerminalSession>;
    /// Returns a live session, or `None` when it does not exist.
    fn find_session(&self, session_id: &str) -> Option<TerminalSession>;
    /// Returns the full ring of a session.
    fn ring_snapshot(&self, session_id: &str) -> Option<RemoteRing>;
    /// Returns the ring output after `after`, or `None` when the ring lost part of it.
    fn ring_since(&self, session_id: &str, after: u64) -> Option<RemoteRing>;
    /// Writes input to a session.
    fn write(&self, session_id: &str, data: &[u8]) -> Result<(), String>;
    /// Sets or removes the output tap.
    fn install_tap(&self, tap: Option<TerminalTap>);
}

impl TerminalSource for TerminalManager {
    fn sessions(&self) -> Vec<TerminalSession> {
        self.list()
    }

    fn find_session(&self, session_id: &str) -> Option<TerminalSession> {
        self.session(session_id).ok()
    }

    fn ring_snapshot(&self, session_id: &str) -> Option<RemoteRing> {
        self.remote_ring_snapshot(session_id)
    }

    fn ring_since(&self, session_id: &str, after: u64) -> Option<RemoteRing> {
        self.remote_ring_since(session_id, after)
    }

    fn write(&self, session_id: &str, data: &[u8]) -> Result<(), String> {
        self.remote_write(session_id, data)
    }

    fn install_tap(&self, tap: Option<TerminalTap>) {
        crate::terminal::set_terminal_tap(tap);
    }
}

/// Records the activity of a conversation after remote input.
///
/// The app updates the conversation time and sends `conversation-activity`,
/// like the `terminal_write` command. The backend limits the calls for each conversation.
pub(crate) trait ConversationActivity: Send + Sync + 'static {
    /// Records new activity of the conversation.
    fn touch(&self, conversation_id: &str);
}

/// The agent status of the panes (section 8.6, capability `agent-status.v1`).
///
/// `agent_bridge::DesktopAgentStatus` connects the desktop `agent_status` module here.
/// It keys statuses by the terminal session id.
pub(crate) trait AgentStatusSource: Send + Sync + 'static {
    /// Returns true when the source reports statuses. The host then adds `agent-status.v1`.
    fn available(&self) -> bool;
    /// Returns the status of a pane.
    fn status(&self, session_id: &str) -> Option<AgentStatus>;
    /// Returns a receiver that gets a value after each status change.
    fn subscribe(&self) -> Option<broadcast::Receiver<()>>;
}

/// An agent status source that reports nothing. Tests use it.
#[cfg(test)]
pub(crate) struct NoAgentStatus;

#[cfg(test)]
impl AgentStatusSource for NoAgentStatus {
    fn available(&self) -> bool {
        false
    }

    fn status(&self, _session_id: &str) -> Option<AgentStatus> {
        None
    }

    fn subscribe(&self) -> Option<broadcast::Receiver<()>> {
        None
    }
}

/// Maps the `agent_kind` of a terminal session to the pane kind of the contract.
///
/// A session without an agent is a shell. Halv calls the GLM lane `zai`.
/// A kind that the contract does not name keeps its own text.
pub(crate) fn pane_kind(agent_kind: Option<&str>) -> PaneKind {
    match agent_kind.map(str::trim).filter(|kind| !kind.is_empty()) {
        None => PaneKind::Shell,
        Some("zai") => PaneKind::Glm,
        Some(kind) => PaneKind::from(kind),
    }
}

/// Returns true for an agent kind whose TUI always turns on bracketed paste:
/// Claude Code and Codex CLI.
pub(crate) fn always_bracketed_paste(agent_kind: Option<&str>) -> bool {
    matches!(pane_kind(agent_kind), PaneKind::Claude | PaneKind::Codex)
}

/// Returns true when terminal input submits a line, as in `terminal_write`.
fn input_marks_activity(data: &[u8]) -> bool {
    data.iter().any(|byte| matches!(byte, b'\r' | b'\n'))
}

/// The product name of an agent kind, for a pane without a title.
/// It follows `AgentKind::display_name` in `agent_sessions`.
fn agent_display_name(agent_kind: &str) -> Option<&'static str> {
    Some(match agent_kind {
        "antigravity" => "Antigravity CLI",
        "claude" => "Claude Code",
        "codex" => "Codex CLI",
        "gemini" => "Gemini CLI",
        "grok" => "Grok",
        "kimi" => "Kimi Code",
        "zai" => "GLM Coding Plan",
        _ => return None,
    })
}

/// Returns the pane title: the user title, else the agent name, else the shell name.
pub(crate) fn pane_title(session: &TerminalSession) -> String {
    if let Some(title) = session.title.as_deref().map(str::trim) {
        if !title.is_empty() {
            return title.to_string();
        }
    }
    if let Some(name) = session.agent_kind.as_deref().and_then(agent_display_name) {
        return name.to_string();
    }
    std::path::Path::new(&session.shell)
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or("Terminal")
        .to_string()
}

fn non_empty(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_string())
}

/// Maps a terminal session to a pane of the contract.
pub(crate) fn pane_from_session(
    session: &TerminalSession,
    last_output_at: Option<u64>,
    status: Option<AgentStatus>,
) -> Pane {
    let running = session.status == "running";
    Pane {
        session: session.id.clone(),
        title: pane_title(session),
        kind: pane_kind(session.agent_kind.as_deref()),
        project: non_empty(&session.project_name),
        cwd: non_empty(&session.cwd),
        cols: session.cols,
        rows: session.rows,
        state: if running {
            PaneState::Running
        } else {
            PaneState::Exited
        },
        exit_code: if running { None } else { session.exit_code },
        last_output_at,
        status,
    }
}

/// The output time of a pane, and the last time that output alone changed the pane list.
struct Activity {
    last_output_at_ms: u64,
    last_notice: Instant,
}

/// The [`HostBackend`] of the gateway.
pub(crate) struct GatewayBackend {
    terminals: Arc<dyn TerminalSource>,
    agent_status: Arc<dyn AgentStatusSource>,
    conversations: Arc<dyn ConversationActivity>,
    host_info: HostInfo,
    output: broadcast::Sender<OutputEvent>,
    pane_changes: broadcast::Sender<()>,
    activity: Mutex<HashMap<String, Activity>>,
    /// The last time that remote input updated each conversation.
    conversation_touches: Mutex<HashMap<String, Instant>>,
}

impl GatewayBackend {
    pub(crate) fn new(
        terminals: Arc<dyn TerminalSource>,
        agent_status: Arc<dyn AgentStatusSource>,
        conversations: Arc<dyn ConversationActivity>,
        host_info: HostInfo,
    ) -> Arc<GatewayBackend> {
        let (output, _) = broadcast::channel(OUTPUT_CHANNEL_CAPACITY);
        let (pane_changes, _) = broadcast::channel(PANE_CHANGE_CAPACITY);
        let backend = Arc::new(GatewayBackend {
            terminals,
            agent_status,
            conversations,
            host_info,
            output,
            pane_changes,
            activity: Mutex::new(HashMap::new()),
            conversation_touches: Mutex::new(HashMap::new()),
        });
        if let Some(changes) = backend.agent_status.subscribe() {
            forward_agent_status_changes(Arc::downgrade(&backend), changes);
        }
        backend
    }

    /// Returns a tap that forwards terminal events to this backend.
    /// The tap holds a weak reference, so it never keeps a stopped gateway alive.
    pub(crate) fn tap(self: &Arc<Self>) -> TerminalTap {
        let backend = Arc::downgrade(self);
        Arc::new(move |event| {
            if let Some(backend) = backend.upgrade() {
                backend.on_terminal_event(event);
            }
        })
    }

    /// Asks the engine to read the pane list again.
    pub(crate) fn notify_panes(&self) {
        let _ = self.pane_changes.send(());
    }

    fn activity(&self) -> std::sync::MutexGuard<'_, HashMap<String, Activity>> {
        self.activity
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Converts a tap event to an engine event. It never waits.
    pub(crate) fn on_terminal_event(&self, event: TerminalTapEvent<'_>) {
        match event {
            TerminalTapEvent::Data {
                session_id,
                sequence,
                data,
            } => {
                let _ = self.output.send(OutputEvent::Data {
                    session: session_id.to_string(),
                    seq: sequence,
                    data: Arc::from(data.as_bytes()),
                });
                if self.record_output(session_id) {
                    self.notify_panes();
                }
            }
            TerminalTapEvent::Exit {
                session_id,
                exit_code,
                signal,
            } => {
                let _ = self.output.send(OutputEvent::Exit {
                    session: session_id.to_string(),
                    code: exit_code,
                    signal: signal.map(str::to_string),
                });
                self.notify_panes();
            }
            TerminalTapEvent::Resized {
                session_id,
                cols,
                rows,
            } => {
                let _ = self.output.send(OutputEvent::Resized {
                    session: session_id.to_string(),
                    cols,
                    rows,
                });
                self.notify_panes();
            }
            TerminalTapEvent::Removed { session_id } => {
                self.activity().remove(session_id);
                let _ = self.output.send(OutputEvent::Closed {
                    session: session_id.to_string(),
                });
                self.notify_panes();
            }
            TerminalTapEvent::Changed => self.notify_panes(),
        }
    }

    /// Records the output time. Returns true when the pane list must change now:
    /// for the first output of a pane, and then at most once per interval.
    fn record_output(&self, session_id: &str) -> bool {
        let now_ms = halv_remote::unix_time_ms();
        let now = Instant::now();
        let mut activity = self.activity();
        match activity.get_mut(session_id) {
            Some(entry) => {
                entry.last_output_at_ms = now_ms;
                if now.duration_since(entry.last_notice) >= OUTPUT_NOTICE_INTERVAL {
                    entry.last_notice = now;
                    true
                } else {
                    false
                }
            }
            None => {
                activity.insert(
                    session_id.to_string(),
                    Activity {
                        last_output_at_ms: now_ms,
                        last_notice: now,
                    },
                );
                true
            }
        }
    }

    /// Returns true when remote input must update the conversation now:
    /// for the first input, and then at most once per interval.
    fn mark_conversation_activity(&self, conversation_id: &str) -> bool {
        let now = Instant::now();
        let mut touches = self
            .conversation_touches
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if touches
            .get(conversation_id)
            .is_some_and(|last| now.duration_since(*last) < CONVERSATION_ACTIVITY_THROTTLE)
        {
            return false;
        }
        touches.insert(conversation_id.to_string(), now);
        true
    }
}

fn forward_agent_status_changes(
    backend: Weak<GatewayBackend>,
    mut changes: broadcast::Receiver<()>,
) {
    tokio::spawn(async move {
        loop {
            match changes.recv().await {
                Ok(()) | Err(broadcast::error::RecvError::Lagged(_)) => match backend.upgrade() {
                    Some(backend) => backend.notify_panes(),
                    None => return,
                },
                Err(broadcast::error::RecvError::Closed) => return,
            }
        }
    });
}

impl HostBackend for GatewayBackend {
    fn host_info(&self) -> HostInfo {
        self.host_info.clone()
    }

    fn agent_status(&self) -> bool {
        self.agent_status.available()
    }

    fn panes(&self) -> Vec<Pane> {
        let sessions = self.terminals.sessions();
        let with_status = self.agent_status.available();
        let output_times: HashMap<String, u64> = self
            .activity()
            .iter()
            .map(|(session, entry)| (session.clone(), entry.last_output_at_ms))
            .collect();
        sessions
            .iter()
            .map(|session| {
                let last_output_at = output_times.get(&session.id).copied();
                let status = if with_status {
                    self.agent_status.status(&session.id)
                } else {
                    None
                };
                pane_from_session(session, last_output_at, status)
            })
            .collect()
    }

    fn pane_changes(&self) -> broadcast::Receiver<()> {
        self.pane_changes.subscribe()
    }

    fn snapshot(&self, session: &str) -> Option<Snapshot> {
        let ring = self.terminals.ring_snapshot(session)?;
        Some(Snapshot {
            seq: ring.sequence,
            cols: ring.cols,
            rows: ring.rows,
            data: ring.data.into_bytes(),
        })
    }

    fn replay_since(&self, session: &str, after_seq: u64) -> Option<Replay> {
        let ring = self.terminals.ring_since(session, after_seq)?;
        // The ring joins the chunks after `after_seq`. The joined output keeps the
        // sequence number of the newest chunk, so the device acknowledges all of it.
        let chunks = if ring.data.is_empty() || ring.sequence <= after_seq {
            Vec::new()
        } else {
            vec![Chunk {
                seq: ring.sequence,
                data: ring.data.into_bytes(),
            }]
        };
        Some(Replay {
            cols: ring.cols,
            rows: ring.rows,
            chunks,
        })
    }

    fn output(&self) -> broadcast::Receiver<OutputEvent> {
        self.output.subscribe()
    }

    fn write(&self, session: &str, data: &[u8]) -> Result<(), WriteError> {
        let Some(record) = self.terminals.find_session(session) else {
            return Err(WriteError::NotFound);
        };
        self.terminals
            .write(session, data)
            .map_err(WriteError::Failed)?;
        if input_marks_activity(data) {
            if let Some(conversation_id) = record.conversation_id.as_deref() {
                if self.mark_conversation_activity(conversation_id) {
                    self.conversations.touch(conversation_id);
                }
            }
        }
        Ok(())
    }

    /// Claude Code and Codex CLI always turn on bracketed paste. Without this value, the
    /// engine reads the mode from the output, and a mode switch that left the ring is lost.
    /// Other panes return `None`, so the engine follows the output (section 8.8).
    fn bracketed_paste(&self, session: &str) -> Option<bool> {
        let record = self.terminals.find_session(session)?;
        always_bracketed_paste(record.agent_kind.as_deref()).then_some(true)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use halv_supervisor::ScrollbackRing;

    /// A fake `TerminalManager`: sessions with a real scrollback ring.
    #[derive(Default)]
    pub(crate) struct FakeTerminals {
        sessions: Mutex<Vec<(TerminalSession, ScrollbackRing)>>,
        pub(crate) writes: Mutex<Vec<(String, Vec<u8>)>>,
        tap: Mutex<Option<TerminalTap>>,
    }

    pub(crate) fn session(id: &str, agent_kind: Option<&str>) -> TerminalSession {
        TerminalSession {
            id: id.to_string(),
            title: None,
            project_id: Some(1),
            conversation_id: None,
            project_name: "halv".to_string(),
            cwd: "/Users/demo/halv".to_string(),
            shell: "/bin/zsh".to_string(),
            pid: 42,
            execution_profile: "full_access".to_string(),
            agent_kind: agent_kind.map(str::to_string),
            status: "running".to_string(),
            cols: 100,
            rows: 30,
            started_at: "2026-10-14T00:00:00Z".to_string(),
            exited_at: None,
            exit_code: None,
            signal: None,
            continued_from: None,
            shell_tab_id: None,
        }
    }

    impl FakeTerminals {
        pub(crate) fn add(&self, session: TerminalSession) {
            self.sessions
                .lock()
                .unwrap()
                .push((session, ScrollbackRing::new(256 * 1024)));
        }

        /// Appends output to the ring, then sends it through the tap, like `TerminalManager`.
        pub(crate) fn push_output(&self, session_id: &str, data: &str) -> u64 {
            let sequence = {
                let mut sessions = self.sessions.lock().unwrap();
                let (_, ring) = sessions
                    .iter_mut()
                    .find(|(session, _)| session.id == session_id)
                    .expect("a known session");
                ring.append(data.as_bytes()).expect("complete UTF-8").0
            };
            self.send_tap(TerminalTapEvent::Data {
                session_id,
                sequence,
                data,
            });
            sequence
        }

        pub(crate) fn send_tap(&self, event: TerminalTapEvent<'_>) {
            let tap = self.tap.lock().unwrap().clone();
            if let Some(tap) = tap {
                tap(event);
            }
        }

        pub(crate) fn has_tap(&self) -> bool {
            self.tap.lock().unwrap().is_some()
        }
    }

    impl TerminalSource for FakeTerminals {
        fn sessions(&self) -> Vec<TerminalSession> {
            self.sessions
                .lock()
                .unwrap()
                .iter()
                .map(|(session, _)| session.clone())
                .collect()
        }

        fn find_session(&self, session_id: &str) -> Option<TerminalSession> {
            self.sessions
                .lock()
                .unwrap()
                .iter()
                .find(|(session, _)| session.id == session_id)
                .map(|(session, _)| session.clone())
        }

        fn ring_snapshot(&self, session_id: &str) -> Option<RemoteRing> {
            let sessions = self.sessions.lock().unwrap();
            let (session, ring) = sessions
                .iter()
                .find(|(session, _)| session.id == session_id)?;
            let (sequence, data) = ring.snapshot(None);
            Some(RemoteRing {
                sequence,
                cols: session.cols,
                rows: session.rows,
                data,
            })
        }

        fn ring_since(&self, session_id: &str, after: u64) -> Option<RemoteRing> {
            let sessions = self.sessions.lock().unwrap();
            let (session, ring) = sessions
                .iter()
                .find(|(session, _)| session.id == session_id)?;
            let (sequence, data) = crate::terminal::ring_since(ring, after)?;
            Some(RemoteRing {
                sequence,
                cols: session.cols,
                rows: session.rows,
                data,
            })
        }

        fn write(&self, session_id: &str, data: &[u8]) -> Result<(), String> {
            self.writes
                .lock()
                .unwrap()
                .push((session_id.to_string(), data.to_vec()));
            Ok(())
        }

        fn install_tap(&self, tap: Option<TerminalTap>) {
            *self.tap.lock().unwrap() = tap;
        }
    }

    /// Records the conversations that remote input touched.
    #[derive(Default)]
    pub(crate) struct RecordingActivity {
        pub(crate) touched: Mutex<Vec<String>>,
    }

    impl ConversationActivity for RecordingActivity {
        fn touch(&self, conversation_id: &str) {
            self.touched
                .lock()
                .unwrap()
                .push(conversation_id.to_string());
        }
    }

    fn backend_for(
        terminals: &Arc<FakeTerminals>,
    ) -> (Arc<GatewayBackend>, Arc<RecordingActivity>) {
        let activity = Arc::new(RecordingActivity::default());
        let backend = GatewayBackend::new(
            terminals.clone(),
            Arc::new(NoAgentStatus),
            activity.clone(),
            host_info(),
        );
        (backend, activity)
    }

    fn host_info() -> HostInfo {
        HostInfo {
            name: "Studio Mac".to_string(),
            app: "halv-desktop/0.7.1".to_string(),
            os: "macos".to_string(),
        }
    }

    #[test]
    fn agent_kinds_map_to_the_contract_strings() {
        let cases = [
            (None, "shell"),
            (Some(""), "shell"),
            (Some("claude"), "claude"),
            (Some("codex"), "codex"),
            (Some("kimi"), "kimi"),
            (Some("zai"), "glm"),
            (Some("grok"), "grok"),
            (Some("gemini"), "gemini"),
            (Some("antigravity"), "antigravity"),
        ];
        for (agent_kind, expected) in cases {
            assert_eq!(pane_kind(agent_kind).as_str(), expected, "{agent_kind:?}");
        }
        assert_eq!(pane_kind(Some("claude")), PaneKind::Claude);
        assert_eq!(pane_kind(Some("zai")), PaneKind::Glm);
        assert_eq!(pane_kind(None), PaneKind::Shell);
    }

    #[test]
    fn panes_use_the_session_metadata() {
        let mut shell = session("terminal-1", None);
        shell.title = Some("  Build  ".to_string());
        let pane = pane_from_session(&shell, Some(7), None);
        assert_eq!(pane.session, "terminal-1");
        assert_eq!(pane.title, "Build");
        assert_eq!(pane.kind, PaneKind::Shell);
        assert_eq!(pane.project.as_deref(), Some("halv"));
        assert_eq!(pane.cwd.as_deref(), Some("/Users/demo/halv"));
        assert_eq!((pane.cols, pane.rows), (100, 30));
        assert_eq!(pane.state, PaneState::Running);
        assert_eq!(pane.exit_code, None);
        assert_eq!(pane.last_output_at, Some(7));

        let mut agent = session("agent-1", Some("zai"));
        agent.status = "exited".to_string();
        agent.exit_code = Some(3);
        let pane = pane_from_session(&agent, None, None);
        assert_eq!(pane.title, "GLM Coding Plan");
        assert_eq!(pane.state, PaneState::Exited);
        assert_eq!(pane.exit_code, Some(3));

        let mut failed = session("terminal-2", None);
        failed.status = "failed".to_string();
        failed.project_name = " ".to_string();
        let pane = pane_from_session(&failed, None, None);
        assert_eq!(pane.title, "zsh");
        assert_eq!(pane.state, PaneState::Exited);
        assert_eq!(pane.project, None);
    }

    #[test]
    fn the_ring_replays_only_complete_output() {
        let mut ring = ScrollbackRing::new(16);
        let first = ring.append(b"0123456789").unwrap().0;
        let second = ring.append(b"abc").unwrap().0;
        // The device has everything.
        assert_eq!(
            crate::terminal::ring_since(&ring, second),
            Some((2, String::new()))
        );
        // The ring still holds the first chunk, so it holds every chunk after it.
        assert_eq!(
            crate::terminal::ring_since(&ring, first),
            Some((2, "abc".to_string()))
        );
        // A sequence after the ring is not known.
        assert_eq!(crate::terminal::ring_since(&ring, 9), None);
        // A trim removes part of the first chunk: a resume from 0 needs a snapshot.
        let third = ring.append(b"defg").unwrap().0;
        assert_eq!(crate::terminal::ring_since(&ring, 0), None);
        assert_eq!(
            crate::terminal::ring_since(&ring, first),
            Some((third, "abcdefg".to_string()))
        );
        // A snapshot from the supervisor is one chunk. The device needs a snapshot for older sequences.
        let mut adopted = ScrollbackRing::new(64);
        adopted.replace_with_snapshot(40, "screen");
        assert_eq!(crate::terminal::ring_since(&adopted, 39), None);
        assert_eq!(
            crate::terminal::ring_since(&adopted, 40),
            Some((40, String::new()))
        );
        // An empty ring with a newer sequence cannot prove that no output was lost.
        let mut empty = ScrollbackRing::new(64);
        empty.set_sequence(5);
        assert_eq!(crate::terminal::ring_since(&empty, 3), None);
        assert_eq!(
            crate::terminal::ring_since(&empty, 5),
            Some((5, String::new()))
        );
    }

    #[tokio::test]
    async fn the_tap_feeds_output_events_and_pane_changes() {
        let terminals = Arc::new(FakeTerminals::default());
        terminals.add(session("terminal-1", Some("claude")));
        let (backend, _) = backend_for(&terminals);
        terminals.install_tap(Some(backend.tap()));
        let mut output = backend.output();
        let mut changes = backend.pane_changes();

        let seq = terminals.push_output("terminal-1", "hello");
        match output.try_recv().unwrap() {
            OutputEvent::Data {
                session,
                seq: got,
                data,
            } => {
                assert_eq!(session, "terminal-1");
                assert_eq!(got, seq);
                assert_eq!(&data[..], b"hello");
            }
            other => panic!("unexpected event {other:?}"),
        }
        assert!(
            changes.try_recv().is_ok(),
            "the first output changes the panes"
        );
        terminals.push_output("terminal-1", "more");
        let _ = output.try_recv().unwrap();
        assert!(
            changes.try_recv().is_err(),
            "later output waits for the interval"
        );
        let pane = &backend.panes()[0];
        assert!(pane.last_output_at.is_some());
        assert_eq!(pane.kind, PaneKind::Claude);

        terminals.send_tap(TerminalTapEvent::Resized {
            session_id: "terminal-1",
            cols: 120,
            rows: 40,
        });
        assert_eq!(
            output.try_recv().unwrap(),
            OutputEvent::Resized {
                session: "terminal-1".to_string(),
                cols: 120,
                rows: 40
            }
        );
        terminals.send_tap(TerminalTapEvent::Exit {
            session_id: "terminal-1",
            exit_code: Some(0),
            signal: None,
        });
        assert_eq!(
            output.try_recv().unwrap(),
            OutputEvent::Exit {
                session: "terminal-1".to_string(),
                code: Some(0),
                signal: None
            }
        );
        terminals.send_tap(TerminalTapEvent::Removed {
            session_id: "terminal-1",
        });
        assert_eq!(
            output.try_recv().unwrap(),
            OutputEvent::Closed {
                session: "terminal-1".to_string()
            }
        );
        terminals.send_tap(TerminalTapEvent::Changed);
        assert!(changes.try_recv().is_ok());

        // The tap holds a weak reference: a dropped backend gets no events.
        drop(output);
        drop(changes);
        drop(backend);
        terminals.push_output("terminal-1", "after");
    }

    #[test]
    fn snapshots_replays_and_writes_use_the_terminal_source() {
        let terminals = Arc::new(FakeTerminals::default());
        terminals.add(session("terminal-1", None));
        let (backend, _) = backend_for(&terminals);
        let first = terminals.push_output("terminal-1", "one ");
        let second = terminals.push_output("terminal-1", "two");

        let snapshot = backend.snapshot("terminal-1").unwrap();
        assert_eq!(snapshot.seq, second);
        assert_eq!(snapshot.data, b"one two");
        assert_eq!((snapshot.cols, snapshot.rows), (100, 30));
        assert!(backend.snapshot("missing").is_none());

        let replay = backend.replay_since("terminal-1", first).unwrap();
        assert_eq!(
            replay.chunks,
            vec![Chunk {
                seq: second,
                data: b"two".to_vec()
            }]
        );
        assert!(backend
            .replay_since("terminal-1", second)
            .unwrap()
            .chunks
            .is_empty());
        assert!(backend.replay_since("terminal-1", second + 5).is_none());

        backend.write("terminal-1", b"ls\r").unwrap();
        assert_eq!(
            terminals.writes.lock().unwrap().as_slice(),
            [("terminal-1".to_string(), b"ls\r".to_vec())]
        );
        assert_eq!(backend.write("missing", b"x"), Err(WriteError::NotFound));
        assert!(!backend.agent_status());
        assert_eq!(backend.host_info().name, "Studio Mac");
    }

    #[test]
    fn claude_and_codex_panes_always_use_bracketed_paste() {
        let terminals = Arc::new(FakeTerminals::default());
        for (id, kind) in [
            ("claude-1", Some("claude")),
            ("codex-1", Some("codex")),
            ("kimi-1", Some("kimi")),
            ("glm-1", Some("zai")),
            ("shell-1", None),
        ] {
            terminals.add(session(id, kind));
        }
        let (backend, _) = backend_for(&terminals);
        assert_eq!(backend.bracketed_paste("claude-1"), Some(true));
        assert_eq!(backend.bracketed_paste("codex-1"), Some(true));
        // Other panes and unknown panes leave the mode to the output scanner.
        assert_eq!(backend.bracketed_paste("kimi-1"), None);
        assert_eq!(backend.bracketed_paste("glm-1"), None);
        assert_eq!(backend.bracketed_paste("shell-1"), None);
        assert_eq!(backend.bracketed_paste("missing"), None);
        assert!(always_bracketed_paste(Some(" claude ")));
        assert!(!always_bracketed_paste(None));
    }

    #[test]
    fn remote_input_with_a_line_break_updates_the_conversation() {
        let terminals = Arc::new(FakeTerminals::default());
        let mut linked = session("agent-1", Some("claude"));
        linked.conversation_id = Some("conversation-1".to_string());
        terminals.add(linked);
        terminals.add(session("terminal-1", None));
        let (backend, activity) = backend_for(&terminals);

        // Typing without a line break is not activity, like `terminal_write`.
        backend.write("agent-1", b"draft").unwrap();
        assert!(activity.touched.lock().unwrap().is_empty());
        backend.write("agent-1", b"\r").unwrap();
        assert_eq!(
            activity.touched.lock().unwrap().as_slice(),
            ["conversation-1".to_string()]
        );
        // The throttle merges input that follows soon after.
        backend.write("agent-1", b"more\n").unwrap();
        assert_eq!(activity.touched.lock().unwrap().len(), 1);
        // A pane without a conversation and a failed write change nothing.
        backend.write("terminal-1", b"ls\r").unwrap();
        assert_eq!(backend.write("missing", b"\r"), Err(WriteError::NotFound));
        assert_eq!(activity.touched.lock().unwrap().len(), 1);
        assert_eq!(terminals.writes.lock().unwrap().len(), 4);
    }
}
