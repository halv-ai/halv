//! The status machine of one agent pane.
//!
//! The machine has no I/O. The service feeds it observations from the
//! Claude presence file, the Claude hook journal, the Codex rollout, the
//! Codex terminal notifications, the input path, and the PTY exit. Each
//! observation can change the public status of the pane.
//!
//! For a Claude pane, the presence file is the authority for the live state
//! (working, waiting, at rest). The hooks give the outcome of a turn (done
//! or failed) and the detail of a wait. Without a presence file, the hooks
//! also drive the live state.
//!
//! The terminal title is the fallback of every pane. It applies only while
//! the pane has no fresh signal: no presence file, and no hook or rollout
//! event in the last 30 minutes. A spinner title starts a turn only after a
//! prompt. Without a prompt it shows work that has no result.

use halv_supervisor::agent_hook::HookRecord;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// An Esc key press this close to the end of a turn marks the turn as
/// interrupted, not done.
pub(crate) const ESCAPE_INTERRUPT_WINDOW_MS: u64 = 3_000;
/// With hooks on, a turn that rests without a Stop event waits this long for
/// a late one. A turn with no Stop event at all was stopped. Without a
/// presence file, a Stop event waits this long too: a Stop hook of the user
/// can continue the turn.
pub(crate) const REST_GRACE_MS: u64 = 1_500;
/// A hook or rollout signal older than this no longer hides the terminal
/// title. Orca uses the same time for its hook rows.
pub(crate) const SIGNAL_STALE_AFTER_MS: u64 = 30 * 60 * 1_000;
/// The store notes a key after the PTY write ends, so the title of the work
/// that the key started can come first. Work in the title that began this
/// short time before Enter is the work of that Enter.
pub(crate) const PROMPT_RACE_MS: u64 = 500;

/// The public status of an agent pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AgentStatus {
    Working,
    Done,
    NeedsInput,
    NeedsApproval,
    Idle,
    Exited,
    /// The last turn ended with an API or model error.
    Failed,
    /// The user stopped the last turn.
    Interrupted,
}

/// The signal that set the current status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum StatusSource {
    /// No agent signal exists yet. The UI uses its own fallback.
    None,
    Presence,
    Hook,
    Rollout,
    /// A notification that the agent wrote to its terminal.
    Terminal,
    /// The terminal title that the agent sets. It is the fallback signal.
    Title,
    Process,
}

/// What a waiting agent asks for. Hook payloads supply this detail.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum PromptDetail {
    Approval {
        tool_name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        command: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        file_path: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        message: Option<String>,
    },
    Question {
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        options: Option<Vec<String>>,
    },
}

/// One pane in the snapshot, and the payload of the `agent-status` event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentStatusEntry {
    pub session_id: String,
    pub status: AgentStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Unix milliseconds of the last status, reason, or prompt change.
    pub updated_at: u64,
    pub unread: bool,
    /// Counts the waits for input or approval. A new wait gets a new value.
    pub wait_seq: u64,
    /// Counts the turns that started in this pane.
    pub turn_seq: u64,
    pub source: StatusSource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<PromptDetail>,
}

/// The live state that Claude Code writes to its session presence file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PresenceState {
    Busy,
    Rest,
    Waiting(String),
}

/// A Codex rollout turn event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RolloutTurn {
    Started,
    Finished { aborted: bool },
}

/// A notification that Codex writes to its terminal (OSC 9).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TerminalNotice {
    /// Codex asks to run a command or to change a file.
    Approval {
        command: Option<String>,
        file_path: Option<String>,
        message: Option<String>,
    },
    /// Codex asks a question.
    Question(String),
    /// Codex asks how to go on with a plan.
    Plan(String),
}

/// The agent state in a terminal title (OSC 0 or OSC 2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TitleStatus {
    Working,
    /// The agent asks for an approval.
    Permission,
    Idle,
}

/// A key press in a Codex pane that can answer its prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AnswerKey {
    /// Enter or Esc. Each closes a prompt.
    Submit,
    /// A choice key of an approval: y, a, n, or a digit. In a question, the
    /// same key can be the first letter of the answer.
    Choice,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PaneInput {
    Presence(PresenceState),
    /// The presence file is gone or belongs to another process.
    PresenceLost,
    /// One line of the hook journal of the pane.
    Hook(Box<HookRecord>),
    Rollout(RolloutTurn),
    /// The rollout recorded a tool result at this time. A tool result after
    /// a wait began means that the user answered.
    RolloutActivity {
        at: u64,
    },
    Terminal(TerminalNotice),
    /// The terminal title changed to show this state.
    Title(TitleStatus),
    /// The user pressed Enter, which can send a prompt. Work in the title
    /// after it is a turn.
    Prompt,
    /// The user pressed a key that can answer a prompt.
    Answer(AnswerKey),
    Escape,
    /// Time passed. It ends a rest that waits for a late Stop event.
    Tick,
    ProcessExited {
        error: bool,
    },
}

enum WaitKind {
    Approval(&'static str),
    Input(&'static str),
    /// A dialog that is news only while a turn runs.
    Dialog,
}

/// Maps the `waitingFor` text of the presence file to a wait.
fn classify_wait(waiting_for: &str) -> WaitKind {
    match waiting_for.trim() {
        "permission prompt" => WaitKind::Approval("permission"),
        "sandbox request" => WaitKind::Approval("sandbox"),
        "worker request" => WaitKind::Approval("worker"),
        "input needed" => WaitKind::Input("question"),
        "goal proposal" => WaitKind::Input("goal"),
        _ => WaitKind::Dialog,
    }
}

const QUESTION_TOOL: &str = "AskUserQuestion";

fn approval_detail(record: &HookRecord) -> PromptDetail {
    PromptDetail::Approval {
        tool_name: record.tool.clone().unwrap_or_default(),
        command: record.command.clone(),
        file_path: record.file_path.clone(),
        message: record.message.clone(),
    }
}

fn question_detail(record: &HookRecord) -> Option<PromptDetail> {
    let text = record.question.clone().or_else(|| record.message.clone())?;
    Some(PromptDetail::Question {
        text,
        options: record.options.clone(),
    })
}

fn detail_fits(status: AgentStatus, detail: &PromptDetail) -> bool {
    matches!(
        (status, detail),
        (AgentStatus::NeedsApproval, PromptDetail::Approval { .. })
            | (AgentStatus::NeedsInput, PromptDetail::Question { .. })
    )
}

#[derive(Debug, Clone)]
pub(crate) struct PaneStatusMachine {
    status: AgentStatus,
    reason: Option<String>,
    prompt: Option<PromptDetail>,
    source: StatusSource,
    updated_at: u64,
    turn_seq: u64,
    wait_seq: u64,
    /// The turn of the last turn that came to rest.
    settled_turn_seq: u64,
    /// The last rested turn that the user saw.
    viewed_turn_seq: u64,
    escape_at: Option<u64>,
    /// When the current wait began.
    wait_since: u64,
    /// A presence file drives the live state of this pane.
    presence_active: bool,
    /// The hooks of this pane run. A hooked turn always ends with a Stop or
    /// StopFailure event unless the user stops it.
    hooks_seen: bool,
    /// The outcome that a Stop or StopFailure event gave the current turn.
    turn_outcome: Option<AgentStatus>,
    /// The live state rested, and the machine waits for a late Stop event.
    rest_pending_since: Option<u64>,
    /// The detail of the last prompt that a hook announced. A wait from the
    /// presence file takes it.
    stash: Option<PromptDetail>,
    /// The tool call of the current wait, when a hook named it.
    waiting_tool_use_id: Option<String>,
    /// The subagent of the current wait. `None` is the main agent.
    waiting_agent_id: Option<String>,
    /// The tool name and id of the last PreToolUse event of each agent in
    /// this turn. The key `None` is the main agent.
    last_tool_uses: HashMap<Option<String>, (String, String)>,
    /// When the last hook or rollout signal arrived.
    signal_at: Option<u64>,
    /// The state of the last terminal title that told one.
    title: Option<TitleStatus>,
    /// The user pressed Enter at rest, and no turn began since.
    prompt_pending: bool,
    /// The state before the current work in the title, while no prompt
    /// started that work. Codex animates its title while it starts, before
    /// any prompt. Such work is not a turn and has no result.
    title_work: Option<PriorState>,
}

/// The state that a pane goes back to after work with no prompt.
#[derive(Debug, Clone)]
struct PriorState {
    status: AgentStatus,
    reason: Option<String>,
    source: StatusSource,
    /// When the work began.
    since: u64,
}

type Fingerprint = (
    AgentStatus,
    Option<String>,
    Option<PromptDetail>,
    StatusSource,
    bool,
    u64,
    u64,
);

impl PaneStatusMachine {
    pub(crate) fn new(now: u64) -> Self {
        Self {
            status: AgentStatus::Idle,
            reason: None,
            prompt: None,
            source: StatusSource::None,
            updated_at: now,
            turn_seq: 0,
            wait_seq: 0,
            settled_turn_seq: 0,
            viewed_turn_seq: 0,
            escape_at: None,
            wait_since: 0,
            presence_active: false,
            hooks_seen: false,
            turn_outcome: None,
            rest_pending_since: None,
            stash: None,
            waiting_tool_use_id: None,
            waiting_agent_id: None,
            last_tool_uses: HashMap::new(),
            signal_at: None,
            title: None,
            prompt_pending: false,
            title_work: None,
        }
    }

    #[cfg(test)]
    pub(crate) fn status(&self) -> AgentStatus {
        self.status
    }

    pub(crate) fn entry(&self, session_id: &str) -> AgentStatusEntry {
        AgentStatusEntry {
            session_id: session_id.to_string(),
            status: self.status,
            reason: self.reason.clone(),
            updated_at: self.updated_at,
            unread: self.unread(),
            wait_seq: self.wait_seq,
            turn_seq: self.turn_seq,
            source: self.source,
            prompt: self.prompt.clone(),
        }
    }

    /// True while the machine waits for a late Stop event.
    #[cfg(test)]
    pub(crate) fn rest_pending(&self) -> bool {
        self.rest_pending_since.is_some()
    }

    /// Applies one observation. `focused` is true when the user looks at
    /// this pane in a focused window. Returns true when the entry changed.
    pub(crate) fn apply(&mut self, input: PaneInput, now: u64, focused: bool) -> bool {
        let before = self.fingerprint();
        if self.status == AgentStatus::Exited {
            // The PTY process is gone. Nothing can bring this pane back.
            return false;
        }
        // A Codex notification does not hide the title: it never tells the
        // end of its wait, and the title does.
        if matches!(
            input,
            PaneInput::Hook(_) | PaneInput::Rollout(_) | PaneInput::RolloutActivity { .. }
        ) {
            self.signal_at = Some(now);
        }
        match input {
            PaneInput::Presence(state) => {
                self.presence_active = true;
                self.source = StatusSource::Presence;
                self.apply_live(state, now, focused);
            }
            PaneInput::PresenceLost => {
                self.presence_active = false;
                if self.source == StatusSource::Presence {
                    // The hooks still describe this pane when they run.
                    self.source = if self.hooks_seen {
                        StatusSource::Hook
                    } else {
                        StatusSource::None
                    };
                }
                // The last title is the best signal that is left.
                if let Some(title) = self.title {
                    self.apply_title(title, now, focused);
                }
            }
            PaneInput::Hook(record) => self.apply_hook(&record, now, focused),
            PaneInput::Rollout(RolloutTurn::Started) => {
                // A late turn start must not end a wait in the same turn.
                if !self.in_turn() {
                    self.source = StatusSource::Rollout;
                    self.enter_working(now);
                }
            }
            PaneInput::Rollout(RolloutTurn::Finished { aborted }) => {
                if self.in_turn() {
                    self.source = StatusSource::Rollout;
                    let outcome = if aborted {
                        AgentStatus::Interrupted
                    } else {
                        AgentStatus::Done
                    };
                    self.settle(outcome, now, focused);
                }
            }
            PaneInput::RolloutActivity { at } => {
                if self.source == StatusSource::Terminal
                    && self.needs_user()
                    && at >= self.wait_since
                {
                    self.source = StatusSource::Rollout;
                    self.enter_working(now);
                }
            }
            PaneInput::Terminal(notice) => self.apply_terminal_notice(notice, now),
            PaneInput::Title(title) => {
                self.title = Some(title);
                self.apply_title(title, now, focused);
            }
            PaneInput::Prompt => {
                // The title can show the work just before the key reaches
                // this machine. The key makes that work a turn. Enter during
                // older work, such as the start animation of Codex, is for
                // the next work.
                let just_began = self
                    .title_work
                    .as_ref()
                    .is_some_and(|work| now.saturating_sub(work.since) <= PROMPT_RACE_MS);
                if just_began {
                    self.claim_title_work(now);
                } else if !self.in_turn() {
                    self.prompt_pending = true;
                }
            }
            PaneInput::Answer(key) => {
                // Codex sends no event when the user answers. Enter or Esc
                // closes each prompt. A choice key closes an approval only.
                let answers = match key {
                    AnswerKey::Submit => true,
                    AnswerKey::Choice => self.status == AgentStatus::NeedsApproval,
                };
                if self.source == StatusSource::Terminal && self.needs_user() && answers {
                    self.enter_working(now);
                }
            }
            PaneInput::Escape => self.escape_at = Some(now),
            PaneInput::Tick => {
                if self
                    .rest_pending_since
                    .is_some_and(|since| now.saturating_sub(since) >= REST_GRACE_MS)
                {
                    let outcome = self.turn_outcome.unwrap_or(AgentStatus::Interrupted);
                    self.settle(outcome, now, focused);
                }
            }
            PaneInput::ProcessExited { error } => {
                self.source = StatusSource::Process;
                self.rest_pending_since = None;
                self.set_status(AgentStatus::Exited, error.then(|| "error".to_string()), now);
            }
        }
        before != self.fingerprint()
    }

    /// Marks every rested turn as seen. A wait for input stays: only the
    /// agent can end it. Returns true when the unread flag changed.
    pub(crate) fn mark_viewed(&mut self) -> bool {
        let was_unread = self.unread();
        self.viewed_turn_seq = self.settled_turn_seq;
        was_unread
    }

    pub(crate) fn unread(&self) -> bool {
        matches!(self.status, AgentStatus::Done | AgentStatus::Failed)
            && self.viewed_turn_seq < self.settled_turn_seq
    }

    pub(crate) fn needs_user(&self) -> bool {
        matches!(
            self.status,
            AgentStatus::NeedsInput | AgentStatus::NeedsApproval
        )
    }

    /// True while a turn runs or waits. Codex panes poll faster then.
    pub(crate) fn active(&self) -> bool {
        self.in_turn()
    }

    /// Makes the work in the title a turn, when no prompt started it.
    /// Returns true when it did. A spinner in the scrollback of an older
    /// pane shows a turn in progress.
    pub(crate) fn claim_title_work(&mut self, now: u64) -> bool {
        if self.title_work.is_none() {
            return false;
        }
        self.begin_turn();
        self.updated_at = now;
        true
    }

    fn fingerprint(&self) -> Fingerprint {
        (
            self.status,
            self.reason.clone(),
            self.prompt.clone(),
            self.source,
            self.unread(),
            self.wait_seq,
            self.turn_seq,
        )
    }

    /// True while a turn runs or waits. Work in the title with no prompt is
    /// not a turn.
    fn in_turn(&self) -> bool {
        self.title_work.is_none()
            && matches!(
                self.status,
                AgentStatus::Working | AgentStatus::NeedsInput | AgentStatus::NeedsApproval
            )
    }

    fn apply_live(&mut self, state: PresenceState, now: u64, focused: bool) {
        match state {
            PresenceState::Busy => self.enter_working(now),
            PresenceState::Waiting(waiting_for) => match classify_wait(&waiting_for) {
                WaitKind::Approval(reason) => {
                    let detail = self.stash_for(AgentStatus::NeedsApproval);
                    self.enter_wait(AgentStatus::NeedsApproval, reason, detail, now)
                }
                WaitKind::Input(reason) => {
                    let detail = self.stash_for(AgentStatus::NeedsInput);
                    self.enter_wait(AgentStatus::NeedsInput, reason, detail, now)
                }
                // A dialog that the user opens at rest, such as /config, is
                // not news. A dialog during a turn pauses the work.
                WaitKind::Dialog if self.in_turn() => {
                    self.enter_wait(AgentStatus::NeedsInput, "dialog", None, now)
                }
                WaitKind::Dialog => {}
            },
            PresenceState::Rest => self.rest(now, focused),
        }
    }

    fn apply_hook(&mut self, record: &HookRecord, now: u64, focused: bool) {
        self.hooks_seen = true;
        // Without a presence file, the hooks drive the live state.
        let live = !self.presence_active;
        if live {
            self.source = StatusSource::Hook;
        }
        let main_agent = record.agent_id.is_none();
        let tool = record.tool.as_deref().unwrap_or_default();
        match record.event.as_str() {
            "SessionStart" => {
                let fresh = matches!(
                    record.source.as_deref(),
                    Some("startup" | "resume" | "clear")
                );
                if fresh && !self.in_turn() {
                    // A new or cleared conversation has no result to read.
                    self.viewed_turn_seq = self.settled_turn_seq;
                    self.set_status(AgentStatus::Idle, None, now);
                }
            }
            "UserPromptSubmit" => {
                // A queued prompt can come right after the Stop of the last
                // turn, before the presence file rests. It is a new turn.
                self.last_tool_uses.clear();
                if !self.in_turn() || self.turn_outcome.is_some() {
                    self.begin_turn();
                } else {
                    // The busy state of this prompt came first. Keep its
                    // turn, but an older Esc press does not stop it.
                    self.escape_at = None;
                    self.clear_wait_context();
                }
                self.rest_pending_since = None;
                self.set_status(AgentStatus::Working, None, now);
            }
            "PreToolUse" if tool == QUESTION_TOOL => {
                self.remember_tool_use(record);
                if main_agent {
                    self.turn_outcome = None;
                }
                let detail = question_detail(record);
                self.offer_wait(AgentStatus::NeedsInput, "question", detail, live, now);
                self.waiting_tool_use_id = record.tool_use_id.clone();
                self.waiting_agent_id = record.agent_id.clone();
            }
            "PreToolUse" => {
                self.remember_tool_use(record);
                if !self.drives_pane(record) {
                    return;
                }
                // A new tool call means that the agent works again, also
                // after a Stop that a Stop hook of the user blocked.
                self.stash = None;
                self.turn_outcome = None;
                if live {
                    self.enter_working(now);
                }
            }
            "PostToolUse" | "PostToolUseFailure" => {
                if !self.drives_pane(record) {
                    return;
                }
                if self.needs_user() {
                    if let (Some(waiting), Some(finished)) =
                        (&self.waiting_tool_use_id, &record.tool_use_id)
                    {
                        if waiting != finished {
                            // A parallel tool call finished. The wait goes on.
                            return;
                        }
                        // The tool of the wait ran, so the user answered. The
                        // presence file can skip the short busy state before
                        // the next prompt, so the hook ends this wait.
                        self.enter_working(now);
                        return;
                    }
                }
                self.clear_wait_context();
                if live {
                    self.enter_working(now);
                }
            }
            "PermissionRequest" => {
                if main_agent {
                    self.turn_outcome = None;
                }
                // The event has no tool_use_id. The PreToolUse event of the
                // same tool by the same agent comes just before it.
                let waiting_tool_use_id = self
                    .last_tool_uses
                    .get(&record.agent_id)
                    .filter(|(name, _)| name == tool)
                    .map(|(_, id)| id.clone());
                let (status, reason, detail) = match tool {
                    QUESTION_TOOL => (AgentStatus::NeedsInput, "question", question_detail(record)),
                    "ExitPlanMode" | "EnterPlanMode" => (
                        AgentStatus::NeedsApproval,
                        "plan",
                        Some(approval_detail(record)),
                    ),
                    _ => (
                        AgentStatus::NeedsApproval,
                        "permission",
                        Some(approval_detail(record)),
                    ),
                };
                self.offer_wait(status, reason, detail, live, now);
                self.waiting_tool_use_id = waiting_tool_use_id;
                self.waiting_agent_id = record.agent_id.clone();
            }
            "Notification" => match record.notification_type.as_deref() {
                Some("elicitation_dialog" | "elicitation_url_dialog") => {
                    let detail = question_detail(record);
                    self.offer_wait(AgentStatus::NeedsInput, "input", detail, live, now);
                }
                Some("worker_permission_prompt") => {
                    let detail = Some(approval_detail(record));
                    self.offer_wait(AgentStatus::NeedsApproval, "worker", detail, live, now);
                }
                Some("permission_prompt") => {
                    // Older builds send this instead of PermissionRequest.
                    // Newer builds send it some seconds after each dialog,
                    // also after a question, so it never opens a second wait.
                    let detail = PromptDetail::Approval {
                        tool_name: self
                            .last_tool_uses
                            .get(&None)
                            .map(|(name, _)| name.clone())
                            .unwrap_or_default(),
                        command: None,
                        file_path: None,
                        message: record.message.clone(),
                    };
                    if self.needs_user() {
                        self.attach_prompt(AgentStatus::NeedsApproval, Some(detail), now);
                    } else {
                        let status = AgentStatus::NeedsApproval;
                        self.offer_wait(status, "permission", Some(detail), live, now);
                    }
                }
                // Claude waits at its prompt. A turn that the hooks still
                // show as running ended with no Stop event.
                Some("idle_prompt") if live => self.rest(now, focused),
                _ => {}
            },
            "Stop" | "StopFailure" if main_agent => {
                let outcome = if record.event == "Stop" {
                    AgentStatus::Done
                } else {
                    AgentStatus::Failed
                };
                self.turn_outcome = Some(outcome);
                if self.rest_pending_since.is_some() {
                    // The live state rested first. This is the late Stop.
                    self.settle(outcome, now, focused);
                } else if live && self.in_turn() {
                    // A Stop hook of the user can continue the turn. A tool
                    // call or a prompt in the grace time keeps it open.
                    self.rest_pending_since = Some(now);
                }
                // With a presence file, the turn ends when the file rests.
            }
            // Subagent events and SessionEnd do not change the pane. The PTY
            // exit owns the end of the session.
            _ => {}
        }
    }

    fn apply_terminal_notice(&mut self, notice: TerminalNotice, now: u64) {
        let (status, reason, detail) = match notice {
            TerminalNotice::Approval {
                command,
                file_path,
                message,
            } => (
                AgentStatus::NeedsApproval,
                "permission",
                Some(PromptDetail::Approval {
                    tool_name: if file_path.is_some() {
                        "apply_patch"
                    } else {
                        "exec"
                    }
                    .to_string(),
                    command,
                    file_path,
                    message,
                }),
            ),
            TerminalNotice::Question(text) => (
                AgentStatus::NeedsInput,
                "question",
                (!text.is_empty()).then_some(PromptDetail::Question {
                    text,
                    options: None,
                }),
            ),
            TerminalNotice::Plan(text) => (
                AgentStatus::NeedsInput,
                "plan",
                (!text.is_empty()).then_some(PromptDetail::Question {
                    text,
                    options: None,
                }),
            ),
        };
        self.source = StatusSource::Terminal;
        self.enter_wait(status, reason, detail, now);
    }

    /// True while a signal other than the title drives the pane: a presence
    /// file, or a recent hook or rollout event. Orca: a fresh hook row is
    /// the authority, and the title is the fallback.
    fn signal_fresh(&self, now: u64) -> bool {
        self.presence_active
            || self
                .signal_at
                .is_some_and(|at| now.saturating_sub(at) < SIGNAL_STALE_AFTER_MS)
    }

    /// Applies the state of the terminal title when no fresh signal drives
    /// the pane.
    fn apply_title(&mut self, title: TitleStatus, now: u64, focused: bool) {
        // The title ends the work that it started, also when another signal
        // came since. Else a fresh signal could keep that work for good.
        if title == TitleStatus::Idle {
            if let Some(source) = self.end_title_work(now) {
                self.source = match source {
                    StatusSource::None => StatusSource::Title,
                    source => source,
                };
                return;
            }
        }
        if self.signal_fresh(now) {
            return;
        }
        let before = (self.status, self.turn_seq, self.wait_seq);
        match title {
            // Work with no prompt shows the spinner, but it is not a turn.
            // It cannot end as a result or send a banner.
            TitleStatus::Working if self.title_work.is_some() => {}
            TitleStatus::Working if !self.in_turn() && !self.prompt_pending => {
                self.title_work = Some(PriorState {
                    status: self.status,
                    reason: self.reason.clone(),
                    source: self.source,
                    since: now,
                });
                self.rest_pending_since = None;
                self.set_status(AgentStatus::Working, None, now);
            }
            TitleStatus::Working => self.enter_working(now),
            TitleStatus::Permission => {
                self.enter_wait(AgentStatus::NeedsApproval, "permission", None, now)
            }
            // The title cannot tell a stop from a finish, or a denied prompt
            // from an answered one. Only a recent Esc press marks a stop.
            TitleStatus::Idle if self.in_turn() => {
                let outcome = if self.escape_recent(now) {
                    AgentStatus::Interrupted
                } else {
                    AgentStatus::Done
                };
                self.settle(outcome, now, focused);
            }
            TitleStatus::Idle => {}
        }
        // A title that changes nothing keeps the source of the state, so
        // the banner of that state does not come twice. A pane without any
        // signal now reports through its title.
        if (self.status, self.turn_seq, self.wait_seq) != before
            || self.source == StatusSource::None
        {
            self.source = StatusSource::Title;
        }
    }

    fn remember_tool_use(&mut self, record: &HookRecord) {
        if let (Some(tool), Some(id)) = (&record.tool, &record.tool_use_id) {
            self.last_tool_uses
                .insert(record.agent_id.clone(), (tool.clone(), id.clone()));
        }
    }

    /// True for an event of the main agent, or of the subagent whose prompt
    /// the pane waits on. Other subagent events do not change the pane.
    fn drives_pane(&self, record: &HookRecord) -> bool {
        record.agent_id.is_none() || (self.needs_user() && record.agent_id == self.waiting_agent_id)
    }

    /// A hook announced a wait. Without a presence file it starts the wait.
    /// With one, the presence file starts the wait and takes the detail.
    fn offer_wait(
        &mut self,
        status: AgentStatus,
        reason: &str,
        detail: Option<PromptDetail>,
        live: bool,
        now: u64,
    ) {
        self.stash.clone_from(&detail);
        if live {
            self.enter_wait(status, reason, detail, now);
        } else {
            self.attach_prompt(status, detail, now);
        }
    }

    /// Adds the detail to a wait of the same kind that has none yet.
    fn attach_prompt(&mut self, status: AgentStatus, detail: Option<PromptDetail>, now: u64) {
        if self.status != status || self.prompt.is_some() {
            return;
        }
        if let Some(detail) = detail.filter(|detail| detail_fits(status, detail)) {
            self.prompt = Some(detail);
            self.updated_at = now;
        }
    }

    fn stash_for(&self, status: AgentStatus) -> Option<PromptDetail> {
        self.stash
            .clone()
            .filter(|detail| detail_fits(status, detail))
    }

    fn clear_wait_context(&mut self) {
        self.stash = None;
        self.waiting_tool_use_id = None;
        self.waiting_agent_id = None;
    }

    /// The live state came to rest. Picks the outcome of the turn.
    fn rest(&mut self, now: u64, focused: bool) {
        if self.end_title_work(now).is_some() || !self.in_turn() {
            return;
        }
        if let Some(outcome) = self.turn_outcome {
            self.settle(outcome, now, focused);
        } else if self.escape_recent(now) {
            self.settle(AgentStatus::Interrupted, now, focused);
        } else if !self.hooks_seen {
            self.settle(AgentStatus::Done, now, focused);
        } else if self.needs_user() {
            // The user denied or cancelled the prompt, and the turn ended.
            self.settle(AgentStatus::Interrupted, now, focused);
        } else if self.rest_pending_since.is_none() {
            self.rest_pending_since = Some(now);
        }
    }

    fn begin_turn(&mut self) {
        self.turn_seq += 1;
        // An Esc press at rest, such as one that clears the prompt, does
        // not stop the new turn.
        self.escape_at = None;
        self.turn_outcome = None;
        self.prompt_pending = false;
        self.title_work = None;
        self.clear_wait_context();
    }

    /// Ends the work in the title that no prompt started. The pane goes
    /// back to its state before that work. Returns the source of that state.
    fn end_title_work(&mut self, now: u64) -> Option<StatusSource> {
        let prior = self.title_work.take()?;
        self.set_status(prior.status, prior.reason, now);
        Some(prior.source)
    }

    fn enter_working(&mut self, now: u64) {
        if !self.in_turn() {
            self.begin_turn();
        }
        self.rest_pending_since = None;
        self.set_status(AgentStatus::Working, None, now);
    }

    fn enter_wait(
        &mut self,
        status: AgentStatus,
        reason: &str,
        detail: Option<PromptDetail>,
        now: u64,
    ) {
        if !self.in_turn() {
            // A wait outside a turn still belongs to a new turn.
            self.begin_turn();
        }
        self.rest_pending_since = None;
        if self.status == status && self.reason.as_deref() == Some(reason) {
            self.attach_prompt(status, detail, now);
            return;
        }
        self.wait_seq += 1;
        self.wait_since = now;
        self.status = status;
        self.reason = Some(reason.to_string());
        self.prompt = detail.filter(|detail| detail_fits(status, detail));
        self.updated_at = now;
    }

    fn settle(&mut self, outcome: AgentStatus, now: u64, focused: bool) {
        self.settled_turn_seq = self.turn_seq;
        if focused {
            self.viewed_turn_seq = self.settled_turn_seq;
        }
        self.escape_at = None;
        self.rest_pending_since = None;
        let reason = (outcome == AgentStatus::Failed).then(|| "error".to_string());
        self.set_status(outcome, reason, now);
    }

    fn escape_recent(&self, now: u64) -> bool {
        self.escape_at
            .is_some_and(|at| now.saturating_sub(at) <= ESCAPE_INTERRUPT_WINDOW_MS)
    }

    fn set_status(&mut self, status: AgentStatus, reason: Option<String>, now: u64) {
        if self.status == status && self.reason == reason && self.prompt.is_none() {
            return;
        }
        if self.needs_user()
            && !matches!(status, AgentStatus::NeedsInput | AgentStatus::NeedsApproval)
        {
            self.clear_wait_context();
        }
        if status != AgentStatus::Working {
            // Another signal ended the work in the title.
            self.title_work = None;
        }
        self.status = status;
        self.reason = reason;
        self.prompt = None;
        self.updated_at = now;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn presence(machine: &mut PaneStatusMachine, state: PresenceState, now: u64) -> bool {
        machine.apply(PaneInput::Presence(state), now, false)
    }

    fn waiting(text: &str) -> PresenceState {
        PresenceState::Waiting(text.to_string())
    }

    #[test]
    fn a_new_pane_is_idle_and_read() {
        let mut machine = PaneStatusMachine::new(10);
        assert!(presence(&mut machine, PresenceState::Rest, 20));
        let entry = machine.entry("pane");
        assert_eq!(entry.status, AgentStatus::Idle);
        assert_eq!(entry.source, StatusSource::Presence);
        assert!(!entry.unread);
        assert_eq!(entry.updated_at, 10);
        assert_eq!(entry.turn_seq, 0);
    }

    #[test]
    fn a_turn_works_then_rests_as_unread_done() {
        let mut machine = PaneStatusMachine::new(0);
        presence(&mut machine, PresenceState::Busy, 100);
        assert_eq!(machine.status(), AgentStatus::Working);
        assert_eq!(machine.entry("pane").turn_seq, 1);
        presence(&mut machine, PresenceState::Rest, 900);
        let entry = machine.entry("pane");
        assert_eq!(entry.status, AgentStatus::Done);
        assert!(entry.unread);
        assert_eq!(entry.updated_at, 900);

        assert!(machine.mark_viewed());
        assert!(!machine.entry("pane").unread);
        assert!(!machine.mark_viewed());
    }

    #[test]
    fn a_turn_that_rests_in_the_focused_pane_is_read() {
        let mut machine = PaneStatusMachine::new(0);
        presence(&mut machine, PresenceState::Busy, 1);
        machine.apply(PaneInput::Presence(PresenceState::Rest), 2, true);
        assert_eq!(machine.status(), AgentStatus::Done);
        assert!(!machine.unread());
    }

    #[test]
    fn viewing_a_running_turn_does_not_read_its_result() {
        let mut machine = PaneStatusMachine::new(0);
        presence(&mut machine, PresenceState::Busy, 1);
        machine.mark_viewed();
        presence(&mut machine, PresenceState::Rest, 2);
        assert!(machine.unread());
    }

    #[test]
    fn a_permission_prompt_needs_approval_and_survives_viewing() {
        let mut machine = PaneStatusMachine::new(0);
        presence(&mut machine, PresenceState::Busy, 1);
        assert!(presence(&mut machine, waiting("permission prompt"), 2));
        let entry = machine.entry("pane");
        assert_eq!(entry.status, AgentStatus::NeedsApproval);
        assert_eq!(entry.reason.as_deref(), Some("permission"));
        assert_eq!(entry.wait_seq, 1);
        assert_eq!(entry.turn_seq, 1);

        machine.mark_viewed();
        assert_eq!(machine.status(), AgentStatus::NeedsApproval);

        // Approval resumes the same turn.
        presence(&mut machine, PresenceState::Busy, 3);
        assert_eq!(machine.status(), AgentStatus::Working);
        assert_eq!(machine.entry("pane").turn_seq, 1);
        // A second prompt in the same turn is a new wait.
        presence(&mut machine, waiting("permission prompt"), 4);
        assert_eq!(machine.entry("pane").wait_seq, 2);
    }

    #[test]
    fn questions_and_other_waits_need_input() {
        for (text, reason) in [("input needed", "question"), ("goal proposal", "goal")] {
            let mut machine = PaneStatusMachine::new(0);
            presence(&mut machine, PresenceState::Busy, 1);
            presence(&mut machine, waiting(text), 2);
            let entry = machine.entry("pane");
            assert_eq!(entry.status, AgentStatus::NeedsInput, "{text}");
            assert_eq!(entry.reason.as_deref(), Some(reason));
        }
        for (text, reason) in [("sandbox request", "sandbox"), ("worker request", "worker")] {
            let mut machine = PaneStatusMachine::new(0);
            presence(&mut machine, waiting(text), 2);
            let entry = machine.entry("pane");
            assert_eq!(entry.status, AgentStatus::NeedsApproval, "{text}");
            assert_eq!(entry.reason.as_deref(), Some(reason));
            assert_eq!(entry.turn_seq, 1);
        }
    }

    #[test]
    fn a_dialog_is_news_only_during_a_turn() {
        let mut machine = PaneStatusMachine::new(0);
        presence(&mut machine, waiting("dialog open"), 1);
        assert_eq!(machine.status(), AgentStatus::Idle);
        presence(&mut machine, waiting("a future wait"), 2);
        assert_eq!(machine.status(), AgentStatus::Idle);

        presence(&mut machine, PresenceState::Busy, 3);
        presence(&mut machine, waiting("dialog open"), 4);
        let entry = machine.entry("pane");
        assert_eq!(entry.status, AgentStatus::NeedsInput);
        assert_eq!(entry.reason.as_deref(), Some("dialog"));
    }

    #[test]
    fn an_answered_question_at_the_end_of_a_turn_is_done() {
        let mut machine = PaneStatusMachine::new(0);
        presence(&mut machine, PresenceState::Busy, 1);
        presence(&mut machine, waiting("input needed"), 2);
        presence(&mut machine, PresenceState::Rest, 3);
        assert_eq!(machine.status(), AgentStatus::Done);
        assert!(machine.unread());
    }

    #[test]
    fn escape_before_the_rest_interrupts_the_turn() {
        let mut machine = PaneStatusMachine::new(0);
        presence(&mut machine, PresenceState::Busy, 1_000);
        machine.apply(PaneInput::Escape, 2_000, false);
        presence(&mut machine, PresenceState::Rest, 2_500);
        assert_eq!(machine.status(), AgentStatus::Interrupted);
        assert!(!machine.unread());

        // An old Esc press does not change the next outcome.
        presence(&mut machine, PresenceState::Busy, 3_000);
        machine.apply(PaneInput::Escape, 3_100, false);
        presence(
            &mut machine,
            PresenceState::Rest,
            3_100 + ESCAPE_INTERRUPT_WINDOW_MS + 1,
        );
        assert_eq!(machine.status(), AgentStatus::Done);
    }

    #[test]
    fn escape_at_rest_does_not_interrupt_the_next_turn() {
        let mut machine = PaneStatusMachine::new(0);
        machine.apply(PaneInput::Escape, 1_000, false);
        presence(&mut machine, PresenceState::Busy, 1_500);
        presence(&mut machine, PresenceState::Rest, 2_000);
        assert_eq!(machine.status(), AgentStatus::Done);
        assert!(machine.unread());
    }

    #[test]
    fn escape_at_a_permission_prompt_interrupts_the_turn() {
        let mut machine = PaneStatusMachine::new(0);
        presence(&mut machine, PresenceState::Busy, 1);
        presence(&mut machine, waiting("permission prompt"), 2);
        machine.apply(PaneInput::Escape, 3, false);
        presence(&mut machine, PresenceState::Rest, 4);
        assert_eq!(machine.status(), AgentStatus::Interrupted);
    }

    #[test]
    fn work_without_a_prompt_counts_as_a_new_turn() {
        let mut machine = PaneStatusMachine::new(0);
        presence(&mut machine, PresenceState::Busy, 1);
        presence(&mut machine, PresenceState::Rest, 2);
        machine.mark_viewed();
        // A scheduled wakeup starts work with no user prompt.
        presence(&mut machine, PresenceState::Busy, 3);
        presence(&mut machine, PresenceState::Rest, 4);
        let entry = machine.entry("pane");
        assert_eq!(entry.turn_seq, 2);
        assert!(entry.unread);
    }

    #[test]
    fn a_lost_presence_file_returns_the_pane_to_the_fallback() {
        let mut machine = PaneStatusMachine::new(0);
        presence(&mut machine, PresenceState::Busy, 1);
        assert!(machine.apply(PaneInput::PresenceLost, 2, false));
        let entry = machine.entry("pane");
        assert_eq!(entry.source, StatusSource::None);
        assert_eq!(entry.status, AgentStatus::Working);
        assert!(!machine.apply(PaneInput::PresenceLost, 3, false));
    }

    #[test]
    fn rollout_turns_drive_codex_panes() {
        let mut machine = PaneStatusMachine::new(0);
        machine.apply(PaneInput::Rollout(RolloutTurn::Started), 1, false);
        assert_eq!(machine.status(), AgentStatus::Working);
        assert_eq!(machine.entry("pane").source, StatusSource::Rollout);
        machine.apply(
            PaneInput::Rollout(RolloutTurn::Finished { aborted: false }),
            2,
            false,
        );
        assert_eq!(machine.status(), AgentStatus::Done);
        assert!(machine.unread());

        machine.apply(PaneInput::Rollout(RolloutTurn::Started), 3, false);
        machine.apply(
            PaneInput::Rollout(RolloutTurn::Finished { aborted: true }),
            4,
            false,
        );
        assert_eq!(machine.status(), AgentStatus::Interrupted);
        assert!(!machine.unread());

        // A finish without a running turn is history, not news.
        assert!(!machine.apply(
            PaneInput::Rollout(RolloutTurn::Finished { aborted: false }),
            5,
            false,
        ));
        assert_eq!(machine.status(), AgentStatus::Interrupted);
    }

    #[test]
    fn process_exit_is_final() {
        let mut machine = PaneStatusMachine::new(0);
        presence(&mut machine, PresenceState::Busy, 1);
        assert!(machine.apply(PaneInput::ProcessExited { error: true }, 2, false));
        let entry = machine.entry("pane");
        assert_eq!(entry.status, AgentStatus::Exited);
        assert_eq!(entry.reason.as_deref(), Some("error"));
        assert_eq!(entry.source, StatusSource::Process);
        assert!(!presence(&mut machine, PresenceState::Busy, 3));
        assert_eq!(machine.status(), AgentStatus::Exited);
    }

    #[test]
    fn unchanged_observations_report_no_change() {
        let mut machine = PaneStatusMachine::new(0);
        assert!(presence(&mut machine, PresenceState::Busy, 1));
        assert!(!presence(&mut machine, PresenceState::Busy, 2));
        assert!(presence(&mut machine, waiting("input needed"), 3));
        assert!(!presence(&mut machine, waiting("input needed"), 4));
        assert_eq!(machine.entry("pane").updated_at, 3);
    }

    #[test]
    fn the_entry_serializes_with_the_contract_names() {
        let mut machine = PaneStatusMachine::new(5);
        presence(&mut machine, waiting("permission prompt"), 7);
        let value = serde_json::to_value(machine.entry("terminal-1")).unwrap();
        assert_eq!(
            value,
            serde_json::json!({
                "sessionId": "terminal-1",
                "status": "needsApproval",
                "reason": "permission",
                "updatedAt": 7,
                "unread": false,
                "waitSeq": 1,
                "turnSeq": 1,
                "source": "presence",
            })
        );

        let approval = PromptDetail::Approval {
            tool_name: "Bash".to_string(),
            command: Some("npm test".to_string()),
            file_path: None,
            message: None,
        };
        assert_eq!(
            serde_json::to_value(&approval).unwrap(),
            serde_json::json!({ "kind": "approval", "toolName": "Bash", "command": "npm test" })
        );
        let question = PromptDetail::Question {
            text: "Which file?".to_string(),
            options: Some(vec!["a.rs".to_string(), "b.rs".to_string()]),
        };
        let encoded = serde_json::to_string(&question).unwrap();
        assert_eq!(
            serde_json::from_str::<PromptDetail>(&encoded).unwrap(),
            question
        );
        for status in [
            AgentStatus::Working,
            AgentStatus::Done,
            AgentStatus::NeedsInput,
            AgentStatus::NeedsApproval,
            AgentStatus::Idle,
            AgentStatus::Exited,
            AgentStatus::Failed,
            AgentStatus::Interrupted,
        ] {
            let encoded = serde_json::to_string(&status).unwrap();
            assert_eq!(
                serde_json::from_str::<AgentStatus>(&encoded).unwrap(),
                status
            );
        }
        assert_eq!(
            serde_json::to_string(&AgentStatus::NeedsInput).unwrap(),
            "\"needsInput\""
        );
    }

    fn hook(event: &str) -> HookRecord {
        HookRecord {
            v: 1,
            pane: "pane".to_string(),
            agent: "claude".to_string(),
            event: event.to_string(),
            ..HookRecord::default()
        }
    }

    fn tool_hook(event: &str, tool: &str, tool_use_id: Option<&str>) -> HookRecord {
        HookRecord {
            tool: Some(tool.to_string()),
            tool_use_id: tool_use_id.map(str::to_string),
            ..hook(event)
        }
    }

    fn send(machine: &mut PaneStatusMachine, record: HookRecord, now: u64) -> bool {
        machine.apply(PaneInput::Hook(Box::new(record)), now, false)
    }

    #[test]
    fn hooks_drive_a_pane_without_a_presence_file() {
        let mut machine = PaneStatusMachine::new(0);
        assert!(send(&mut machine, hook("UserPromptSubmit"), 1));
        let entry = machine.entry("pane");
        assert_eq!(entry.status, AgentStatus::Working);
        assert_eq!(entry.source, StatusSource::Hook);
        assert_eq!(entry.turn_seq, 1);

        send(&mut machine, tool_hook("PreToolUse", "Bash", Some("t1")), 2);
        let request = HookRecord {
            command: Some("cargo test".to_string()),
            ..tool_hook("PermissionRequest", "Bash", None)
        };
        send(&mut machine, request, 3);
        let entry = machine.entry("pane");
        assert_eq!(entry.status, AgentStatus::NeedsApproval);
        assert_eq!(entry.reason.as_deref(), Some("permission"));
        assert_eq!(
            entry.prompt,
            Some(PromptDetail::Approval {
                tool_name: "Bash".to_string(),
                command: Some("cargo test".to_string()),
                file_path: None,
                message: None,
            })
        );
        // A parallel tool call that finishes does not end the wait.
        send(
            &mut machine,
            tool_hook("PostToolUse", "Read", Some("t0")),
            4,
        );
        assert_eq!(machine.status(), AgentStatus::NeedsApproval);
        send(
            &mut machine,
            tool_hook("PostToolUse", "Bash", Some("t1")),
            5,
        );
        assert_eq!(machine.status(), AgentStatus::Working);
        assert_eq!(machine.entry("pane").prompt, None);

        // The Stop waits for the grace time: a Stop hook can continue.
        send(&mut machine, hook("Stop"), 6);
        assert_eq!(machine.status(), AgentStatus::Working);
        machine.apply(PaneInput::Tick, 6 + REST_GRACE_MS, false);
        let entry = machine.entry("pane");
        assert_eq!(entry.status, AgentStatus::Done);
        assert!(entry.unread);
    }

    #[test]
    fn a_subagent_stop_does_not_finish_the_turn() {
        let mut machine = PaneStatusMachine::new(0);
        send(&mut machine, hook("UserPromptSubmit"), 1);
        let subagent_stop = HookRecord {
            agent_id: Some("a1".to_string()),
            ..hook("Stop")
        };
        assert!(!send(&mut machine, subagent_stop, 2));
        assert_eq!(machine.status(), AgentStatus::Working);
        // Subagent tool calls after the main turn ended do not restart it.
        send(&mut machine, hook("Stop"), 3);
        machine.apply(PaneInput::Tick, 3 + REST_GRACE_MS, false);
        let subagent_tool = HookRecord {
            agent_id: Some("a1".to_string()),
            ..tool_hook("PreToolUse", "Bash", Some("t9"))
        };
        assert!(!send(&mut machine, subagent_tool, 4));
        assert_eq!(machine.status(), AgentStatus::Done);
    }

    #[test]
    fn with_presence_the_hooks_give_the_outcome_and_the_detail() {
        let mut machine = PaneStatusMachine::new(0);
        presence(&mut machine, PresenceState::Busy, 1);
        // The hook comes before the presence file shows the wait.
        let request = HookRecord {
            file_path: Some("/repo/src/lib.rs".to_string()),
            ..tool_hook("PermissionRequest", "Edit", None)
        };
        assert!(!send(&mut machine, request, 2));
        assert_eq!(machine.status(), AgentStatus::Working);
        presence(&mut machine, waiting("permission prompt"), 3);
        let entry = machine.entry("pane");
        assert_eq!(entry.source, StatusSource::Presence);
        assert!(matches!(
            entry.prompt,
            Some(PromptDetail::Approval { ref file_path, .. })
                if file_path.as_deref() == Some("/repo/src/lib.rs")
        ));

        // A Stop event alone does not end the turn: a Stop hook of the user
        // can continue it. The presence file decides.
        presence(&mut machine, PresenceState::Busy, 4);
        send(&mut machine, hook("Stop"), 5);
        assert_eq!(machine.status(), AgentStatus::Working);
        presence(&mut machine, PresenceState::Rest, 6);
        assert_eq!(machine.status(), AgentStatus::Done);
        assert!(machine.unread());
    }

    #[test]
    fn a_hook_detail_after_the_presence_wait_joins_the_same_wait() {
        let mut machine = PaneStatusMachine::new(0);
        presence(&mut machine, PresenceState::Busy, 1);
        presence(&mut machine, waiting("input needed"), 2);
        let before = machine.entry("pane");
        assert_eq!(before.prompt, None);
        let question = HookRecord {
            question: Some("Which file?".to_string()),
            options: Some(vec!["a.rs".to_string()]),
            ..tool_hook("PreToolUse", "AskUserQuestion", Some("q1"))
        };
        assert!(send(&mut machine, question, 3));
        let after = machine.entry("pane");
        assert_eq!(after.wait_seq, before.wait_seq);
        assert_eq!(after.source, StatusSource::Presence);
        assert_eq!(
            after.prompt,
            Some(PromptDetail::Question {
                text: "Which file?".to_string(),
                options: Some(vec!["a.rs".to_string()]),
            })
        );
        // A rewrite of the same presence wait keeps the detail and the wait.
        assert!(!presence(&mut machine, waiting("input needed"), 4));
    }

    #[test]
    fn an_old_detail_does_not_reach_a_later_wait() {
        let mut machine = PaneStatusMachine::new(0);
        presence(&mut machine, PresenceState::Busy, 1);
        let request = HookRecord {
            command: Some("ls".to_string()),
            ..tool_hook("PermissionRequest", "Bash", None)
        };
        send(&mut machine, request, 2);
        // Another hook allowed the call: no dialog, and the tool ran.
        send(
            &mut machine,
            tool_hook("PostToolUse", "Bash", Some("t1")),
            3,
        );
        presence(&mut machine, waiting("sandbox request"), 4);
        let entry = machine.entry("pane");
        assert_eq!(entry.status, AgentStatus::NeedsApproval);
        assert_eq!(entry.prompt, None);
    }

    #[test]
    fn a_hooked_turn_that_rests_without_stop_was_stopped() {
        let mut machine = PaneStatusMachine::new(0);
        send(&mut machine, hook("SessionStart"), 0);
        presence(&mut machine, PresenceState::Busy, 1_000);
        presence(&mut machine, PresenceState::Rest, 2_000);
        assert!(machine.rest_pending());
        assert_eq!(machine.status(), AgentStatus::Working);
        assert!(!machine.apply(PaneInput::Tick, 2_000 + REST_GRACE_MS - 1, false));
        assert!(machine.apply(PaneInput::Tick, 2_000 + REST_GRACE_MS, false));
        assert_eq!(machine.status(), AgentStatus::Interrupted);
        assert!(!machine.unread());

        // A new turn during the grace ends the grace.
        presence(&mut machine, PresenceState::Busy, 5_000);
        presence(&mut machine, PresenceState::Rest, 5_100);
        presence(&mut machine, PresenceState::Busy, 5_200);
        assert!(!machine.rest_pending());
        machine.apply(PaneInput::Tick, 9_000, false);
        assert_eq!(machine.status(), AgentStatus::Working);
    }

    #[test]
    fn stop_failure_marks_the_turn_failed() {
        let mut machine = PaneStatusMachine::new(0);
        presence(&mut machine, PresenceState::Busy, 1);
        send(
            &mut machine,
            HookRecord {
                error: Some("rate_limit".to_string()),
                ..hook("StopFailure")
            },
            2,
        );
        presence(&mut machine, PresenceState::Rest, 3);
        let entry = machine.entry("pane");
        assert_eq!(entry.status, AgentStatus::Failed);
        assert_eq!(entry.reason.as_deref(), Some("error"));
        assert!(entry.unread);
        // The next turn starts without the old outcome.
        presence(&mut machine, PresenceState::Busy, 4);
        send(&mut machine, hook("Stop"), 5);
        presence(&mut machine, PresenceState::Rest, 6);
        assert_eq!(machine.status(), AgentStatus::Done);
    }

    #[test]
    fn a_clear_reads_the_last_result() {
        let mut machine = PaneStatusMachine::new(0);
        presence(&mut machine, PresenceState::Busy, 1);
        send(&mut machine, hook("Stop"), 2);
        presence(&mut machine, PresenceState::Rest, 3);
        assert!(machine.unread());
        let clear = HookRecord {
            source: Some("clear".to_string()),
            ..hook("SessionStart")
        };
        send(&mut machine, clear, 4);
        assert_eq!(machine.status(), AgentStatus::Idle);
        assert!(!machine.unread());
        let compact = HookRecord {
            source: Some("compact".to_string()),
            ..hook("SessionStart")
        };
        assert!(!send(&mut machine, compact, 5));
    }

    #[test]
    fn a_lost_presence_file_hands_a_hooked_pane_to_the_hooks() {
        let mut machine = PaneStatusMachine::new(0);
        send(&mut machine, hook("SessionStart"), 0);
        presence(&mut machine, PresenceState::Busy, 1);
        machine.apply(PaneInput::PresenceLost, 2, false);
        assert_eq!(machine.entry("pane").source, StatusSource::Hook);
        send(&mut machine, hook("Stop"), 3);
        machine.apply(PaneInput::Tick, 3 + REST_GRACE_MS, false);
        assert_eq!(machine.status(), AgentStatus::Done);
    }

    #[test]
    fn codex_terminal_notices_start_waits_with_detail() {
        let mut machine = PaneStatusMachine::new(0);
        machine.apply(PaneInput::Rollout(RolloutTurn::Started), 1, false);
        let notice = TerminalNotice::Approval {
            command: Some("npm install".to_string()),
            file_path: None,
            message: None,
        };
        assert!(machine.apply(PaneInput::Terminal(notice), 2_000, false));
        let entry = machine.entry("pane");
        assert_eq!(entry.status, AgentStatus::NeedsApproval);
        assert_eq!(entry.source, StatusSource::Terminal);
        assert_eq!(entry.wait_seq, 1);
        assert!(matches!(
            entry.prompt,
            Some(PromptDetail::Approval { ref tool_name, ref command, .. })
                if tool_name == "exec" && command.as_deref() == Some("npm install")
        ));
        // A late turn start and an old tool result do not end the wait.
        assert!(!machine.apply(PaneInput::Rollout(RolloutTurn::Started), 2_100, false));
        assert!(!machine.apply(PaneInput::RolloutActivity { at: 1_900 }, 2_200, false));
        // A tool result after the wait began means that the user answered.
        assert!(machine.apply(PaneInput::RolloutActivity { at: 2_500 }, 2_600, false));
        assert_eq!(machine.status(), AgentStatus::Working);

        machine.apply(
            PaneInput::Terminal(TerminalNotice::Question("Which branch?".to_string())),
            3_000,
            false,
        );
        let entry = machine.entry("pane");
        assert_eq!(entry.status, AgentStatus::NeedsInput);
        assert_eq!(entry.wait_seq, 2);
        // A letter can start a typed answer. Enter sends the answer.
        let choice = PaneInput::Answer(AnswerKey::Choice);
        assert!(!machine.apply(choice.clone(), 3_050, false));
        assert_eq!(machine.status(), AgentStatus::NeedsInput);
        assert!(machine.apply(PaneInput::Answer(AnswerKey::Submit), 3_100, false));
        assert_eq!(machine.status(), AgentStatus::Working);
        assert!(!machine.apply(PaneInput::Answer(AnswerKey::Submit), 3_200, false));

        machine.apply(
            PaneInput::Terminal(TerminalNotice::Plan("Implement the plan?".to_string())),
            4_000,
            false,
        );
        assert_eq!(machine.entry("pane").reason.as_deref(), Some("plan"));
        machine.apply(
            PaneInput::Rollout(RolloutTurn::Finished { aborted: false }),
            5_000,
            false,
        );
        let entry = machine.entry("pane");
        assert_eq!(entry.status, AgentStatus::Done);
        assert_eq!(entry.source, StatusSource::Rollout);
        assert!(entry.unread);
    }

    #[test]
    fn a_claude_pane_ignores_answer_keys() {
        let mut machine = PaneStatusMachine::new(0);
        presence(&mut machine, PresenceState::Busy, 1);
        presence(&mut machine, waiting("permission prompt"), 2);
        assert!(!machine.apply(PaneInput::Answer(AnswerKey::Submit), 3, false));
        assert_eq!(machine.status(), AgentStatus::NeedsApproval);
    }

    #[test]
    fn a_choice_key_answers_a_codex_approval() {
        let mut machine = PaneStatusMachine::new(0);
        machine.apply(PaneInput::Rollout(RolloutTurn::Started), 1, false);
        let notice = TerminalNotice::Approval {
            command: Some("cargo test".to_string()),
            file_path: None,
            message: None,
        };
        machine.apply(PaneInput::Terminal(notice), 2, false);
        assert!(machine.apply(PaneInput::Answer(AnswerKey::Choice), 3, false));
        assert_eq!(machine.status(), AgentStatus::Working);
    }

    fn subagent(agent_id: &str, record: HookRecord) -> HookRecord {
        HookRecord {
            agent_id: Some(agent_id.to_string()),
            ..record
        }
    }

    #[test]
    fn a_queued_prompt_after_a_stop_is_a_new_turn() {
        // The presence file stays busy from turn A into turn B.
        let mut machine = PaneStatusMachine::new(0);
        presence(&mut machine, PresenceState::Busy, 1);
        send(&mut machine, hook("UserPromptSubmit"), 2);
        send(&mut machine, hook("Stop"), 3);
        send(&mut machine, hook("UserPromptSubmit"), 4);
        assert_eq!(machine.entry("pane").turn_seq, 2);
        machine.apply(PaneInput::Escape, 5_000, false);
        presence(&mut machine, PresenceState::Rest, 5_100);
        assert_eq!(machine.status(), AgentStatus::Interrupted);
        assert!(!machine.unread());

        // A denied prompt in turn B ends it as stopped, too.
        let mut machine = PaneStatusMachine::new(0);
        presence(&mut machine, PresenceState::Busy, 1);
        send(&mut machine, hook("UserPromptSubmit"), 2);
        send(&mut machine, hook("Stop"), 3);
        send(&mut machine, hook("UserPromptSubmit"), 4);
        send(&mut machine, tool_hook("PreToolUse", "Bash", Some("b1")), 5);
        send(
            &mut machine,
            tool_hook("PermissionRequest", "Bash", None),
            6,
        );
        presence(&mut machine, waiting("permission prompt"), 7);
        presence(&mut machine, PresenceState::Rest, 8);
        assert_eq!(machine.status(), AgentStatus::Interrupted);
    }

    #[test]
    fn a_turn_that_goes_on_after_a_blocked_stop_can_still_be_stopped() {
        let mut machine = PaneStatusMachine::new(0);
        send(&mut machine, hook("UserPromptSubmit"), 1);
        presence(&mut machine, PresenceState::Busy, 2);
        // A Stop hook of the user blocks the first Stop. The turn goes on.
        send(&mut machine, hook("Stop"), 3);
        send(&mut machine, tool_hook("PreToolUse", "Bash", Some("t1")), 4);
        send(
            &mut machine,
            tool_hook("PostToolUse", "Bash", Some("t1")),
            5,
        );
        machine.apply(PaneInput::Escape, 6_000, false);
        presence(&mut machine, PresenceState::Rest, 6_300);
        assert_eq!(machine.status(), AgentStatus::Interrupted);
        assert!(!machine.unread());
    }

    #[test]
    fn without_presence_a_blocked_stop_keeps_one_turn() {
        let mut machine = PaneStatusMachine::new(0);
        send(&mut machine, hook("UserPromptSubmit"), 1);
        let mut now = 1;
        for _ in 0..3 {
            // A Stop hook of the user blocks the Stop; the model works on.
            send(&mut machine, hook("Stop"), now + 1);
            now += 1_000;
            machine.apply(PaneInput::Tick, now, false);
            assert_eq!(machine.status(), AgentStatus::Working);
            send(
                &mut machine,
                tool_hook("PreToolUse", "Bash", Some("t")),
                now + 1,
            );
            assert_eq!(machine.status(), AgentStatus::Working);
        }
        send(&mut machine, hook("Stop"), now + 2);
        machine.apply(PaneInput::Tick, now + 2 + REST_GRACE_MS, false);
        let entry = machine.entry("pane");
        assert_eq!(entry.status, AgentStatus::Done);
        assert_eq!(entry.turn_seq, 1);
    }

    #[test]
    fn a_late_permission_notification_joins_the_question_wait() {
        let mut machine = PaneStatusMachine::new(0);
        send(&mut machine, hook("UserPromptSubmit"), 1);
        let question = HookRecord {
            question: Some("Which file?".to_string()),
            ..tool_hook("PreToolUse", "AskUserQuestion", Some("q1"))
        };
        send(&mut machine, question, 2);
        send(
            &mut machine,
            tool_hook("PermissionRequest", "AskUserQuestion", None),
            3,
        );
        let before = machine.entry("pane");
        assert_eq!(before.status, AgentStatus::NeedsInput);
        let late = HookRecord {
            notification_type: Some("permission_prompt".to_string()),
            message: Some("Claude needs your permission".to_string()),
            ..hook("Notification")
        };
        assert!(!send(&mut machine, late, 6_003));
        assert_eq!(machine.entry("pane"), before);
    }

    #[test]
    fn a_subagent_permission_wait_ends_with_that_subagent() {
        let mut machine = PaneStatusMachine::new(0);
        send(&mut machine, hook("UserPromptSubmit"), 1);
        send(&mut machine, tool_hook("PreToolUse", "Task", Some("t0")), 2);
        let bash = || tool_hook("PreToolUse", "Bash", Some("s1"));
        send(&mut machine, subagent("a1", bash()), 3);
        send(
            &mut machine,
            subagent("a2", tool_hook("PreToolUse", "Read", Some("r9"))),
            4,
        );
        send(
            &mut machine,
            subagent("a1", tool_hook("PermissionRequest", "Bash", None)),
            5,
        );
        assert_eq!(machine.status(), AgentStatus::NeedsApproval);
        // Another subagent works on. The wait goes on.
        let other = subagent("a2", tool_hook("PostToolUse", "Read", Some("r9")));
        assert!(!send(&mut machine, other, 6));
        assert_eq!(machine.status(), AgentStatus::NeedsApproval);
        // The tool of the waiting subagent ran: the user approved it.
        let done = subagent("a1", tool_hook("PostToolUse", "Bash", Some("s1")));
        assert!(send(&mut machine, done, 7));
        assert_eq!(machine.status(), AgentStatus::Working);
        // Later events of that subagent do not change the pane.
        let later = subagent("a1", tool_hook("PreToolUse", "Read", Some("s2")));
        assert!(!send(&mut machine, later, 8));
    }

    #[test]
    fn a_second_prompt_after_an_approved_tool_is_a_new_wait() {
        let mut machine = PaneStatusMachine::new(0);
        send(&mut machine, hook("UserPromptSubmit"), 1);
        presence(&mut machine, PresenceState::Busy, 2);
        send(&mut machine, tool_hook("PreToolUse", "Bash", Some("b1")), 3);
        let first = HookRecord {
            command: Some("git status".to_string()),
            ..tool_hook("PermissionRequest", "Bash", None)
        };
        send(&mut machine, first, 4);
        presence(&mut machine, waiting("permission prompt"), 5);
        assert_eq!(machine.entry("pane").wait_seq, 1);
        // The poll missed the short busy state between the two prompts.
        send(
            &mut machine,
            tool_hook("PostToolUse", "Bash", Some("b1")),
            6,
        );
        send(&mut machine, tool_hook("PreToolUse", "Bash", Some("b2")), 7);
        let second = HookRecord {
            command: Some("rm -rf target".to_string()),
            ..tool_hook("PermissionRequest", "Bash", None)
        };
        send(&mut machine, second, 8);
        presence(&mut machine, waiting("permission prompt"), 9);
        let entry = machine.entry("pane");
        assert_eq!(entry.status, AgentStatus::NeedsApproval);
        assert_eq!(entry.wait_seq, 2);
        assert!(matches!(
            entry.prompt,
            Some(PromptDetail::Approval { ref command, .. })
                if command.as_deref() == Some("rm -rf target")
        ));
    }

    /// Plays one frozen scenario. Each line is a raw Claude hook payload, a
    /// presence state, a tick, or an expectation for the entry.
    fn play(name: &str, text: &str) {
        use halv_supervisor::agent_hook::parse_payload;
        use serde_json::Value;
        let mut machine = PaneStatusMachine::new(0);
        let mut now = 1_000;
        for (index, line) in text.lines().enumerate() {
            let step = serde_json::from_str::<Value>(line).unwrap();
            let at = format!("{name}:{}", index + 1);
            now += 100;
            match step["kind"].as_str().unwrap() {
                "hook" => {
                    let payload = serde_json::to_vec(&step["payload"]).unwrap();
                    let record = parse_payload("pane", "claude", now, &payload, false)
                        .unwrap_or_else(|| panic!("{at}: the payload gives no record"));
                    machine.apply(PaneInput::Hook(Box::new(record)), now, false);
                }
                "presence" => {
                    let state = match step["status"].as_str().unwrap() {
                        "busy" => PresenceState::Busy,
                        "idle" => PresenceState::Rest,
                        "waiting" => waiting(step["waitingFor"].as_str().unwrap()),
                        other => panic!("{at}: unknown presence {other}"),
                    };
                    machine.apply(PaneInput::Presence(state), now, false);
                }
                "tick" => {
                    now += step["afterMs"].as_u64().unwrap();
                    machine.apply(PaneInput::Tick, now, false);
                }
                "expect" => {
                    let entry = serde_json::to_value(machine.entry("pane")).unwrap();
                    for (key, expected) in step.as_object().unwrap() {
                        if key != "kind" {
                            assert_eq!(&entry[key], expected, "{at}: {key} in {entry}");
                        }
                    }
                }
                other => panic!("{at}: unknown step {other}"),
            }
        }
    }

    #[test]
    fn frozen_claude_hook_scenarios() {
        for (name, text) in [
            (
                "claude-permission-approved",
                include_str!("fixtures/claude-permission-approved.jsonl"),
            ),
            (
                "claude-question-answered",
                include_str!("fixtures/claude-question-answered.jsonl"),
            ),
            (
                "claude-permission-denied",
                include_str!("fixtures/claude-permission-denied.jsonl"),
            ),
            (
                "claude-stop-failure",
                include_str!("fixtures/claude-stop-failure.jsonl"),
            ),
            (
                "claude-late-stop",
                include_str!("fixtures/claude-late-stop.jsonl"),
            ),
            (
                "claude-hooks-only",
                include_str!("fixtures/claude-hooks-only.jsonl"),
            ),
            (
                "claude-elicitation-and-old-permission",
                include_str!("fixtures/claude-elicitation-and-old-permission.jsonl"),
            ),
        ] {
            play(name, text);
        }
    }

    fn title(machine: &mut PaneStatusMachine, status: TitleStatus, now: u64) -> bool {
        machine.apply(PaneInput::Title(status), now, false)
    }

    #[test]
    fn the_title_drives_a_pane_without_other_signals() {
        let mut machine = PaneStatusMachine::new(0);
        // A rest title on a new pane is not news, but the pane now reports.
        assert!(title(&mut machine, TitleStatus::Idle, 10));
        let entry = machine.entry("pane");
        assert_eq!(entry.status, AgentStatus::Idle);
        assert_eq!(entry.source, StatusSource::Title);
        assert_eq!(entry.turn_seq, 0);

        // Enter, then work in the title: a turn.
        assert!(!machine.apply(PaneInput::Prompt, 15, false));
        assert!(title(&mut machine, TitleStatus::Working, 20));
        assert_eq!(machine.status(), AgentStatus::Working);
        assert_eq!(machine.entry("pane").turn_seq, 1);
        assert!(title(&mut machine, TitleStatus::Idle, 30));
        let entry = machine.entry("pane");
        assert_eq!(entry.status, AgentStatus::Done);
        assert_eq!(entry.source, StatusSource::Title);
        assert!(entry.unread);

        // An approval title is a wait of a new turn. Work after it is the
        // same turn.
        assert!(title(&mut machine, TitleStatus::Permission, 40));
        let entry = machine.entry("pane");
        assert_eq!(entry.status, AgentStatus::NeedsApproval);
        assert_eq!(entry.reason.as_deref(), Some("permission"));
        assert_eq!((entry.turn_seq, entry.wait_seq), (2, 1));
        assert!(!title(&mut machine, TitleStatus::Permission, 45));
        title(&mut machine, TitleStatus::Working, 50);
        assert_eq!(machine.entry("pane").turn_seq, 2);

        // Esc just before the rest marks a stop.
        machine.apply(PaneInput::Escape, 60, false);
        title(&mut machine, TitleStatus::Idle, 70);
        assert_eq!(machine.status(), AgentStatus::Interrupted);
        assert!(!machine.unread());
    }

    #[test]
    fn a_fresh_hook_beats_the_title_and_a_stale_one_does_not() {
        let mut machine = PaneStatusMachine::new(0);
        send(&mut machine, hook("UserPromptSubmit"), 1_000);
        // Claude rests its title at a permission prompt. The hooks own it.
        assert!(!title(&mut machine, TitleStatus::Idle, 2_000));
        let entry = machine.entry("pane");
        assert_eq!(entry.status, AgentStatus::Working);
        assert_eq!(entry.source, StatusSource::Hook);
        send(&mut machine, hook("Stop"), 3_000);
        machine.apply(PaneInput::Tick, 3_000 + REST_GRACE_MS, false);
        assert_eq!(machine.status(), AgentStatus::Done);
        assert!(!title(&mut machine, TitleStatus::Working, 4_000));

        // The hooks went quiet. The title takes over the next turn.
        let stale = 3_000 + SIGNAL_STALE_AFTER_MS;
        machine.apply(PaneInput::Prompt, stale - 10, false);
        assert!(title(&mut machine, TitleStatus::Working, stale));
        let entry = machine.entry("pane");
        assert_eq!(entry.status, AgentStatus::Working);
        assert_eq!(entry.source, StatusSource::Title);
        assert_eq!(entry.turn_seq, 2);
        // The title ends the turn. A hooked pane without a Stop event would
        // wait for one, but the title cannot see the hooks.
        assert!(title(&mut machine, TitleStatus::Idle, stale + 1_000));
        assert_eq!(machine.status(), AgentStatus::Done);

        // A new hook makes the hooks fresh again.
        send(&mut machine, hook("UserPromptSubmit"), stale + 2_000);
        assert!(!title(&mut machine, TitleStatus::Idle, stale + 3_000));
        assert_eq!(machine.entry("pane").source, StatusSource::Hook);
    }

    #[test]
    fn a_fresh_rollout_beats_the_title() {
        let mut machine = PaneStatusMachine::new(0);
        machine.apply(PaneInput::Rollout(RolloutTurn::Started), 1_000, false);
        assert!(!title(&mut machine, TitleStatus::Idle, 1_500));
        machine.apply(
            PaneInput::Terminal(TerminalNotice::Question("Which?".to_string())),
            2_000,
            false,
        );
        assert!(!title(&mut machine, TitleStatus::Working, 2_100));
        assert_eq!(machine.status(), AgentStatus::NeedsInput);
        assert_eq!(machine.entry("pane").source, StatusSource::Terminal);

        // A long quiet wait goes stale. The title then ends it.
        let stale = 1_000 + SIGNAL_STALE_AFTER_MS;
        assert!(title(&mut machine, TitleStatus::Working, stale));
        assert_eq!(machine.status(), AgentStatus::Working);
        assert_eq!(machine.entry("pane").source, StatusSource::Title);
    }

    #[test]
    fn a_codex_notice_does_not_hide_the_title_that_ends_its_turn() {
        // A Codex pane with no rollout. The notice starts a wait, and Enter
        // answers it. Only the title tells the end of the turn.
        let mut machine = PaneStatusMachine::new(0);
        machine.apply(PaneInput::Prompt, 900, false);
        title(&mut machine, TitleStatus::Working, 1_000);
        let notice = TerminalNotice::Approval {
            command: Some("rm -rf target".to_string()),
            file_path: None,
            message: None,
        };
        assert!(machine.apply(PaneInput::Terminal(notice), 2_000, false));
        // The approval title of the same wait changes nothing.
        assert!(!title(&mut machine, TitleStatus::Permission, 2_010));
        let entry = machine.entry("pane");
        assert_eq!(entry.status, AgentStatus::NeedsApproval);
        assert_eq!(entry.source, StatusSource::Terminal);
        assert_eq!(entry.wait_seq, 1);
        assert!(entry.prompt.is_some());

        machine.apply(PaneInput::Prompt, 3_000, false);
        assert!(machine.apply(PaneInput::Answer(AnswerKey::Submit), 3_000, false));
        assert!(!title(&mut machine, TitleStatus::Working, 3_100));
        assert!(title(&mut machine, TitleStatus::Idle, 4_000));
        let entry = machine.entry("pane");
        assert_eq!(entry.status, AgentStatus::Done);
        assert_eq!(entry.source, StatusSource::Title);
        assert_eq!(entry.turn_seq, 1);
        assert!(entry.unread);
    }

    #[test]
    fn a_spinner_without_a_prompt_is_not_a_turn() {
        // Codex 0.160.1 at start, with no input (Orca capture
        // codex-fullscreen-startup): rest, spinner, rest.
        let mut machine = PaneStatusMachine::new(0);
        title(&mut machine, TitleStatus::Idle, 100);
        assert!(title(&mut machine, TitleStatus::Working, 200));
        let entry = machine.entry("pane");
        assert_eq!(entry.status, AgentStatus::Working);
        assert_eq!(entry.source, StatusSource::Title);
        assert_eq!(entry.turn_seq, 0);
        assert!(!machine.active());
        assert!(title(&mut machine, TitleStatus::Idle, 900));
        let entry = machine.entry("pane");
        assert_eq!(entry.status, AgentStatus::Idle);
        assert_eq!(entry.source, StatusSource::Title);
        assert_eq!((entry.turn_seq, entry.wait_seq), (0, 0));
        assert!(!entry.unread);

        // A first title with a spinner leaves the pane on the title.
        let mut machine = PaneStatusMachine::new(0);
        title(&mut machine, TitleStatus::Working, 100);
        title(&mut machine, TitleStatus::Idle, 200);
        let entry = machine.entry("pane");
        assert_eq!(
            (entry.status, entry.source),
            (AgentStatus::Idle, StatusSource::Title)
        );
        assert_eq!(entry.turn_seq, 0);
    }

    #[test]
    fn work_without_a_prompt_goes_back_to_the_last_result() {
        // An unread rollout turn. Much later the title shows work that no
        // prompt started, for example a thread rename.
        let mut machine = PaneStatusMachine::new(0);
        machine.apply(PaneInput::Rollout(RolloutTurn::Started), 1, false);
        machine.apply(
            PaneInput::Rollout(RolloutTurn::Finished { aborted: false }),
            2,
            false,
        );
        let finished = machine.entry("pane");
        assert!(finished.unread);
        let later = 2 + SIGNAL_STALE_AFTER_MS;
        title(&mut machine, TitleStatus::Working, later);
        assert!(!machine.unread());
        title(&mut machine, TitleStatus::Idle, later + 1_000);
        let entry = machine.entry("pane");
        // The same result with the same source: the banner key holds.
        assert_eq!(entry.status, AgentStatus::Done);
        assert_eq!(entry.source, StatusSource::Rollout);
        assert_eq!(entry.turn_seq, finished.turn_seq);
        assert!(entry.unread);
    }

    #[test]
    fn enter_makes_the_work_in_the_title_a_turn() {
        // The title of the work can come just before the key.
        let mut machine = PaneStatusMachine::new(0);
        title(&mut machine, TitleStatus::Idle, 100);
        title(&mut machine, TitleStatus::Working, 1_000);
        assert!(machine.apply(PaneInput::Prompt, 1_000 + PROMPT_RACE_MS, false));
        assert_eq!(machine.entry("pane").turn_seq, 1);
        assert!(machine.active());
        title(&mut machine, TitleStatus::Idle, 5_000);
        let entry = machine.entry("pane");
        assert_eq!(entry.status, AgentStatus::Done);
        assert!(entry.unread);

        // Enter during older work, such as the start animation of Codex, is
        // for the next work.
        let mut machine = PaneStatusMachine::new(0);
        title(&mut machine, TitleStatus::Working, 1_000);
        assert!(!machine.apply(PaneInput::Prompt, 2_000, false));
        title(&mut machine, TitleStatus::Idle, 2_500);
        assert_eq!(machine.status(), AgentStatus::Idle);
        title(&mut machine, TitleStatus::Working, 3_000);
        assert_eq!(machine.entry("pane").turn_seq, 1);
        title(&mut machine, TitleStatus::Idle, 9_000);
        assert_eq!(machine.status(), AgentStatus::Done);

        // A wait in the title is a turn without a prompt.
        let mut machine = PaneStatusMachine::new(0);
        title(&mut machine, TitleStatus::Working, 1_000);
        title(&mut machine, TitleStatus::Permission, 2_000);
        let entry = machine.entry("pane");
        assert_eq!(entry.status, AgentStatus::NeedsApproval);
        assert_eq!((entry.turn_seq, entry.wait_seq), (1, 1));
    }

    #[test]
    fn other_signals_take_over_the_work_in_the_title() {
        // A rollout start makes the work a rollout turn.
        let mut machine = PaneStatusMachine::new(0);
        title(&mut machine, TitleStatus::Working, 1_000);
        machine.apply(PaneInput::Rollout(RolloutTurn::Started), 1_100, false);
        let entry = machine.entry("pane");
        assert_eq!(entry.status, AgentStatus::Working);
        assert_eq!(entry.source, StatusSource::Rollout);
        assert_eq!(entry.turn_seq, 1);

        // A fresh signal that does not start a turn hides later titles. The
        // title still ends the work that it started.
        let mut machine = PaneStatusMachine::new(0);
        title(&mut machine, TitleStatus::Working, 1_000);
        machine.apply(PaneInput::RolloutActivity { at: 1_100 }, 1_100, false);
        assert!(title(&mut machine, TitleStatus::Idle, 2_000));
        let entry = machine.entry("pane");
        assert_eq!(entry.status, AgentStatus::Idle);
        assert_eq!(entry.turn_seq, 0);

        // A presence file at rest ends it too.
        let mut machine = PaneStatusMachine::new(0);
        title(&mut machine, TitleStatus::Working, 1_000);
        presence(&mut machine, PresenceState::Rest, 1_100);
        let entry = machine.entry("pane");
        assert_eq!(entry.status, AgentStatus::Idle);
        assert_eq!(entry.source, StatusSource::Presence);
        assert_eq!(entry.turn_seq, 0);

        // The PTY exit ends it.
        let mut machine = PaneStatusMachine::new(0);
        title(&mut machine, TitleStatus::Working, 1_000);
        machine.apply(PaneInput::ProcessExited { error: false }, 1_100, false);
        assert_eq!(machine.status(), AgentStatus::Exited);
    }

    #[test]
    fn claiming_the_work_in_the_title_starts_a_turn() {
        let mut machine = PaneStatusMachine::new(0);
        assert!(!machine.claim_title_work(10));
        title(&mut machine, TitleStatus::Working, 20);
        assert!(machine.claim_title_work(30));
        assert_eq!(machine.entry("pane").turn_seq, 1);
        assert!(!machine.claim_title_work(40));
    }

    #[test]
    fn a_title_that_changes_nothing_keeps_the_source() {
        // A Codex turn that the rollout finished long ago. A rest title does
        // not repaint the finished turn as a title turn: the banner key of a
        // turn holds its source.
        let mut machine = PaneStatusMachine::new(0);
        machine.apply(PaneInput::Rollout(RolloutTurn::Started), 1, false);
        machine.apply(
            PaneInput::Rollout(RolloutTurn::Finished { aborted: false }),
            2,
            false,
        );
        let before = machine.entry("pane");
        assert!(!title(
            &mut machine,
            TitleStatus::Idle,
            2 + SIGNAL_STALE_AFTER_MS
        ));
        assert_eq!(machine.entry("pane"), before);
    }

    #[test]
    fn a_lost_presence_file_falls_back_to_the_last_title() {
        let mut machine = PaneStatusMachine::new(0);
        presence(&mut machine, PresenceState::Busy, 1);
        // The title keeps its state while the presence file drives.
        assert!(!title(&mut machine, TitleStatus::Idle, 2));
        assert_eq!(machine.status(), AgentStatus::Working);
        assert!(machine.apply(PaneInput::PresenceLost, 3, false));
        let entry = machine.entry("pane");
        assert_eq!(entry.status, AgentStatus::Done);
        assert_eq!(entry.source, StatusSource::Title);
    }

    #[test]
    fn the_title_source_serializes_as_title() {
        assert_eq!(
            serde_json::to_string(&StatusSource::Title).unwrap(),
            "\"title\""
        );
        let mut machine = PaneStatusMachine::new(5);
        title(&mut machine, TitleStatus::Working, 7);
        let value = serde_json::to_value(machine.entry("terminal-1")).unwrap();
        assert_eq!(value["source"], "title");
        assert_eq!(value["status"], "working");
    }
}
