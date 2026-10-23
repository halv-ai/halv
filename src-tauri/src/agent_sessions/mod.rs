mod adapter;
mod claude;
mod cli_binary;
mod codex;
mod ghostty;
mod google;
pub mod install;
mod kimi;
pub(crate) mod memories;
pub(crate) mod model_catalog;
mod probe_cache;
pub(crate) mod session_files;
mod transcript_watcher;
pub(crate) mod usage_meter;

pub(crate) use transcript_watcher::spawn_transcript_watcher;

use crate::{
    auth::profiles,
    chat, exec, extensions, host_env,
    ponytail::PonytailMode,
    preferences::PreferencesManager,
    pricing::{self, PricingSource},
    savings::dollars::{self, PricedSavings, PromptRates, SavedTokens},
    sidecar::{
        halv_managed_listener_port, ProjectSavingsTotals, SidecarManager, SidecarStatus,
        SidecarUpstream,
    },
    storage::{
        new_uuid_v4,
        savings_dollars::{ReportDollars, SnapshotDollars, TurnDollars},
        AgentSessionRecord, AgentSessionRow, AgentSessionTurnRecord, CredentialProfile, NewMessage,
        Storage,
    },
    terminal::{self, TerminalManager, TerminalSession},
};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    ffi::OsString,
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::{Child, ChildStderr, ChildStdin, ChildStdout, Command},
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc, Arc, Mutex,
    },
    thread,
    time::{Duration, Instant, SystemTime},
};
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::oneshot;

#[cfg(test)]
use adapter::PermissionFlags;
use adapter::{
    AdapterProcess, AdapterState, AgentAdapter, AgentProbe, CanonicalMode,
    InteractiveLaunchContext, LaunchContext, ParsedTurn, TurnKind,
};
use probe_cache::{file_fingerprint, FileFingerprint, SharedCheck};

const AGENT_SESSION_EVENT: &str = "agent-session-event";
const DEFAULT_PERMISSION_MODE: &str = "ask";
// RTK does not support the Codex or Kimi hook payloads.
// Each session uses this guidance through its instruction channel.
pub(super) const RTK_DEVELOPER_INSTRUCTIONS: &str = "Prefix read-only shell commands with `rtk` \
(examples: rtk git status, rtk git diff, rtk ls, rtk grep <pattern>, rtk read <file>). \
rtk compresses command output and keeps the same information. \
If rtk does not support a command, run the command directly.";
const DUPLICATE_CREDENTIAL_REFRESH_ERROR: &str =
    "failed to duplicate agent session: sign in again to refresh its credentials";
// A runaway backstop, not a product limit — Orca runs with no cap at all.
const MAX_AGENT_SESSIONS: usize = 32;
const SAVINGS_SETTLE_ATTEMPTS: usize = 7;
#[cfg(test)]
static CLAUDE_CONFIG_DIR_TEST_LOCK: Mutex<()> = Mutex::new(());
const SAVINGS_SETTLE_STABLE_SAMPLES: usize = 3;
const SAVINGS_SETTLE_DELAY: Duration = Duration::from_millis(350);
const SAVINGS_SETTLE_ZERO_GRACE: Duration = Duration::from_millis(1_500);
const TERMINAL_SAVINGS_POLL_INTERVAL: Duration = Duration::from_secs(5);
const CODEX_INIT_TIMEOUT: Duration = Duration::from_secs(30);
const CODEX_PENDING_MESSAGE_LIMIT: usize = 16;
const AGENT_STDERR_TAIL_BYTES: usize = 8 * 1024;
// The writer thread drains this queue into the agent's stdin. The bound
// keeps a wedged CLI from blocking async handlers forever.
const PROTOCOL_WRITE_QUEUE_CAPACITY: usize = 256;
const PROTOCOL_WRITE_ENQUEUE_TIMEOUT: Duration = Duration::from_secs(2);
const PROTOCOL_WRITE_RETRY_DELAY: Duration = Duration::from_millis(10);

#[derive(Debug, PartialEq, Eq)]
enum CodexMessageDisposition {
    Queued,
    Ready,
}

#[derive(Debug)]
enum CodexInitPhase {
    Initializing(VecDeque<String>),
    Flushing(VecDeque<String>),
    Initialized,
    Failed,
}

#[derive(Debug)]
struct CodexInitState {
    started_at: Instant,
    phase: CodexInitPhase,
    last_protocol_error: Option<String>,
}

#[derive(Debug, PartialEq, Eq)]
struct CodexInitTimeout {
    queued_messages: usize,
    last_protocol_error: Option<String>,
}

impl CodexInitState {
    fn new(started_at: Instant) -> Self {
        Self {
            started_at,
            phase: CodexInitPhase::Initializing(VecDeque::new()),
            last_protocol_error: None,
        }
    }

    fn queue_message(&mut self, text: &str) -> AgentSessionResult<CodexMessageDisposition> {
        match &mut self.phase {
            CodexInitPhase::Initializing(messages) | CodexInitPhase::Flushing(messages) => {
                if messages.len() >= CODEX_PENDING_MESSAGE_LIMIT {
                    return Err(format!(
                        "Codex initialization queue is full ({CODEX_PENDING_MESSAGE_LIMIT} messages)"
                    ));
                }
                messages.push_back(text.to_string());
                Ok(CodexMessageDisposition::Queued)
            }
            CodexInitPhase::Initialized => Ok(CodexMessageDisposition::Ready),
            CodexInitPhase::Failed => Err("Codex app-server initialization has failed".to_string()),
        }
    }

    fn begin_flush(&mut self) -> bool {
        let phase = std::mem::replace(&mut self.phase, CodexInitPhase::Failed);
        match phase {
            CodexInitPhase::Initializing(messages) => {
                self.phase = CodexInitPhase::Flushing(messages);
                true
            }
            other => {
                self.phase = other;
                false
            }
        }
    }

    fn next_queued_message(&mut self) -> Option<String> {
        match &mut self.phase {
            CodexInitPhase::Flushing(messages) => {
                if let Some(message) = messages.pop_front() {
                    Some(message)
                } else {
                    self.phase = CodexInitPhase::Initialized;
                    None
                }
            }
            _ => None,
        }
    }

    fn remember_protocol_error(&mut self, message: &str) {
        self.last_protocol_error = Some(message.to_string());
    }

    fn timeout(&mut self, now: Instant) -> Option<CodexInitTimeout> {
        if now.saturating_duration_since(self.started_at) < CODEX_INIT_TIMEOUT {
            return None;
        }
        let phase = std::mem::replace(&mut self.phase, CodexInitPhase::Failed);
        match phase {
            CodexInitPhase::Initializing(messages) => Some(CodexInitTimeout {
                queued_messages: messages.len(),
                last_protocol_error: self.last_protocol_error.clone(),
            }),
            other => {
                self.phase = other;
                None
            }
        }
    }

    fn mark_failed(&mut self) {
        self.phase = CodexInitPhase::Failed;
    }
}

#[derive(Debug)]
struct StderrTail {
    output: String,
    max_bytes: usize,
}

impl StderrTail {
    fn new(max_bytes: usize) -> Self {
        Self {
            output: String::new(),
            max_bytes,
        }
    }

    fn push_line(&mut self, line: &str) {
        if !self.output.is_empty() {
            self.output.push('\n');
        }
        self.output.push_str(line);
        if self.output.len() <= self.max_bytes {
            return;
        }
        let mut drain_bytes = self.output.len() - self.max_bytes;
        while !self.output.is_char_boundary(drain_bytes) {
            drain_bytes += 1;
        }
        self.output.drain(..drain_bytes);
    }

    fn snapshot(&self) -> String {
        self.output.clone()
    }
}

// Each session owns one writer thread. The thread makes stdin writes
// asynchronous, so a wedged CLI cannot block an async handler.
#[derive(Clone)]
struct ProtocolWriter {
    sender: mpsc::SyncSender<String>,
    // The writer thread stores its first write failure here.
    failure: Arc<Mutex<Option<String>>>,
    enqueue_timeout: Duration,
}

impl ProtocolWriter {
    fn spawn(stdin: ChildStdin) -> Self {
        Self::spawn_with(
            stdin,
            PROTOCOL_WRITE_QUEUE_CAPACITY,
            PROTOCOL_WRITE_ENQUEUE_TIMEOUT,
        )
    }

    fn spawn_with<W: Write + Send + 'static>(
        mut writer: W,
        capacity: usize,
        enqueue_timeout: Duration,
    ) -> Self {
        let (sender, receiver) = mpsc::sync_channel::<String>(capacity);
        let failure = Arc::new(Mutex::new(None));
        let thread_failure = failure.clone();
        thread::spawn(move || {
            // One receive loop keeps the protocol write order.
            while let Ok(line) = receiver.recv() {
                let result = writeln!(writer, "{line}").and_then(|()| writer.flush());
                if let Err(error) = result {
                    *thread_failure
                        .lock()
                        .expect("agent writer failure lock poisoned") = Some(error.to_string());
                    return;
                }
            }
        });
        Self {
            sender,
            failure,
            enqueue_timeout,
        }
    }

    // The enqueue waits for a short bounded time when the queue is full.
    // A closed queue or a timeout becomes a protocol error.
    fn enqueue_line(&self, line: &str) -> AgentSessionResult<()> {
        let deadline = Instant::now() + self.enqueue_timeout;
        let mut pending = line.to_string();
        loop {
            match self.sender.try_send(pending) {
                Ok(()) => return Ok(()),
                Err(mpsc::TrySendError::Full(line)) => {
                    if Instant::now() >= deadline {
                        return Err(
                            "failed to write agent protocol message: the agent stopped reading \
                             its input"
                                .to_string(),
                        );
                    }
                    pending = line;
                    thread::sleep(PROTOCOL_WRITE_RETRY_DELAY);
                }
                Err(mpsc::TrySendError::Disconnected(_)) => return Err(self.failure_message()),
            }
        }
    }

    fn failure_message(&self) -> String {
        let failure = self
            .failure
            .lock()
            .expect("agent writer failure lock poisoned")
            .clone();
        match failure {
            Some(error) => format!("failed to write agent protocol message: {error}"),
            None => "agent session input is closed".to_string(),
        }
    }
}

fn codex_init_timeout_message(timeout: &CodexInitTimeout, stderr_tail: &str) -> String {
    let mut message = format!(
        "Codex app-server did not initialize its thread within 30 seconds. {} queued user message(s) failed.",
        timeout.queued_messages
    );
    if let Some(protocol_error) = timeout.last_protocol_error.as_deref() {
        message.push_str("\nLast protocol error: ");
        message.push_str(protocol_error);
    }
    let stderr_tail = stderr_tail.trim();
    if !stderr_tail.is_empty() {
        message.push_str("\nApp-server stderr tail:\n");
        message.push_str(stderr_tail);
    }
    message
}

pub const CLAUDE_CODE_MODELS: [(&str, &str); 4] = [
    ("fable", "Fable 5"),
    ("opus", "Opus 5"),
    ("sonnet", "Sonnet 5"),
    ("haiku", "Haiku 4.5"),
];

pub(crate) fn is_claude_code_model(model: &str) -> bool {
    let model = model.trim();
    CLAUDE_CODE_MODELS.iter().any(|(alias, _)| *alias == model)
        || model
            .strip_prefix("claude-")
            .is_some_and(|suffix| !suffix.is_empty())
}

fn actionable_agent_error(agent_kind: AgentKind, error: String) -> String {
    format!("{} error: {error}", agent_kind.display_name())
}

type AgentSessionResult<T> = Result<T, String>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LaunchSpec {
    pub program: PathBuf,
    pub args: Vec<OsString>,
    pub env: Vec<(OsString, OsString)>,
    pub env_remove: Vec<OsString>,
}

fn command_environment(command: &Command) -> (Vec<(OsString, OsString)>, Vec<OsString>) {
    let mut environment = Vec::new();
    let mut removed = Vec::new();
    for (name, value) in command.get_envs() {
        if let Some(value) = value {
            environment.push((name.to_os_string(), value.to_os_string()));
        } else {
            removed.push(name.to_os_string());
        }
    }
    (environment, removed)
}

fn savings_stack_environment(session_id: &str) -> Vec<(OsString, OsString)> {
    let mut environment =
        savings_stack_environment_with_rtk(exec::rtk_session_database_path(session_id));
    environment.push((
        OsString::from("CRUX_SAVINGS_LEDGER"),
        exec::crux_session_ledger_path(session_id).into_os_string(),
    ));
    environment
}

fn savings_stack_environment_with_rtk(
    rtk_database_path: std::path::PathBuf,
) -> Vec<(OsString, OsString)> {
    let mut environment = vec![
        (
            OsString::from("PATH"),
            OsString::from(extensions::crux_path_environment()),
        ),
        (
            OsString::from(exec::RTK_DB_PATH_ENV),
            rtk_database_path.into_os_string(),
        ),
        (
            OsString::from("RTK_TELEMETRY_DISABLED"),
            OsString::from("1"),
        ),
    ];
    environment.extend(crate::locale::locale_environment());
    environment
}

fn apply_savings_stack_environment(command: &mut Command, session_id: &str) {
    for (name, value) in savings_stack_environment(session_id) {
        command.env(name, value);
    }
}

fn add_savings_stack_to_launch_spec(mut spec: LaunchSpec, session_id: &str) -> LaunchSpec {
    let environment = savings_stack_environment(session_id);
    let names = environment
        .iter()
        .map(|(name, _)| name.clone())
        .collect::<Vec<_>>();
    spec.env
        .retain(|(name, _)| !names.iter().any(|candidate| name == candidate));
    // Each CLI writes savings to session-scoped ledgers. The sidebar meter
    // attributes each source to exactly one session.
    spec.env.extend(environment);
    spec
}

/// The hook helper of an agent pane finds its pane and journal folder
/// through `pane_environment`. Orca variables would send the hooks of Orca
/// to an Orca pane, so the launch removes them.
fn add_agent_status_to_launch_spec(
    mut spec: LaunchSpec,
    pane_environment: Vec<(OsString, OsString)>,
    orca_variables: Vec<OsString>,
) -> LaunchSpec {
    spec.env.retain(|(name, _)| {
        !pane_environment
            .iter()
            .any(|(candidate, _)| name == candidate)
            && !orca_variables.contains(name)
    });
    spec.env.extend(pane_environment);
    for name in orca_variables {
        if !spec.env_remove.contains(&name) {
            spec.env_remove.push(name);
        }
    }
    spec
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AgentKind {
    Antigravity,
    Claude,
    Codex,
    Gemini,
    Grok,
    Kimi,
    Zai,
}

impl AgentKind {
    pub(crate) fn storage_name(self) -> &'static str {
        match self {
            Self::Antigravity => "antigravity",
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Gemini => "gemini",
            Self::Grok => "grok",
            Self::Kimi => "kimi",
            Self::Zai => "zai",
        }
    }

    fn display_name(self) -> &'static str {
        match self {
            Self::Antigravity => "Antigravity CLI",
            Self::Claude => "Claude Code",
            Self::Codex => "Codex CLI",
            Self::Gemini => "Gemini CLI",
            Self::Grok => "Grok",
            Self::Kimi => "Kimi Code",
            Self::Zai => "GLM Coding Plan",
        }
    }

    fn from_storage_name(value: &str) -> AgentSessionResult<Self> {
        match value {
            "antigravity" => Ok(Self::Antigravity),
            "claude" => Ok(Self::Claude),
            "codex" => Ok(Self::Codex),
            "gemini" => Ok(Self::Gemini),
            "grok" => Ok(Self::Grok),
            "kimi" => Ok(Self::Kimi),
            "zai" => Ok(Self::Zai),
            _ => Err(format!("failed to decode agent kind `{value}`")),
        }
    }
}

#[allow(private_interfaces)]
pub(super) fn adapter_for(kind: AgentKind) -> &'static dyn AgentAdapter {
    let adapter: &'static dyn AgentAdapter = match kind {
        AgentKind::Antigravity => &google::ANTIGRAVITY_ADAPTER,
        AgentKind::Claude => &claude::CLAUDE_ADAPTER,
        AgentKind::Codex => &codex::CODEX_ADAPTER,
        AgentKind::Gemini => &google::GEMINI_ADAPTER,
        AgentKind::Grok => &claude::GROK_ADAPTER,
        AgentKind::Kimi => &kimi::KIMI_ADAPTER,
        AgentKind::Zai => &claude::ZAI_ADAPTER,
    };
    debug_assert_eq!(adapter.kind(), kind);
    adapter
}

fn canonical_permission_mode(halv_mode: &str) -> AgentSessionResult<CanonicalMode> {
    match halv_mode.trim() {
        "" | "default" | "ask" => Ok(CanonicalMode::Ask),
        "read_only" => Ok(CanonicalMode::ReadOnly),
        "on_request" => Ok(CanonicalMode::OnRequest),
        "on_failure" | "on-failure" => Ok(CanonicalMode::OnFailure),
        "never" => Ok(CanonicalMode::Never),
        "skip_all" => Ok(CanonicalMode::SkipAll),
        mode => Err(format!("unsupported Halv approval policy `{mode}`")),
    }
}

#[cfg(test)]
fn permission_flags(agent_kind: AgentKind, halv_mode: &str) -> AgentSessionResult<PermissionFlags> {
    adapter_for(agent_kind).permission_flags(canonical_permission_mode(halv_mode)?)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionMode {
    Gui,
    TerminalEmbedded,
    TerminalGhostty,
}

impl SessionMode {
    fn storage_name(self) -> &'static str {
        match self {
            Self::Gui => "gui",
            Self::TerminalEmbedded => "terminal_embedded",
            Self::TerminalGhostty => "terminal_ghostty",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum SessionLane {
    ClaudeSubscription,
    ClaudeApiKey {
        #[serde(flatten)]
        credential: SessionCredential,
        base_url: Option<String>,
    },
    CodexChatGpt {
        proxied: bool,
    },
    ZaiGlm {
        #[serde(flatten)]
        credential: SessionCredential,
        model_overrides: Option<ZaiModelOverrides>,
    },
    GrokApiKey {
        #[serde(flatten)]
        credential: SessionCredential,
        model_overrides: Option<GrokModelOverrides>,
    },
    KimiApiKey {
        #[serde(flatten)]
        credential: SessionCredential,
    },
    KimiSubscription,
    Custom {
        env: Vec<(String, String)>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(untagged, rename_all_fields = "camelCase")]
pub enum SessionCredential {
    ApiKey { api_key: String },
    CredentialProfile { credential_profile_id: String },
}

impl SessionCredential {
    pub(crate) fn api_key(&self) -> Result<&str, String> {
        match self {
            Self::ApiKey { api_key } => Ok(api_key),
            Self::CredentialProfile { .. } => {
                Err("credential profile secret was not resolved before launch".to_string())
            }
        }
    }
}

fn resolve_lane_credentials(
    storage: &Storage,
    lane: SessionLane,
) -> AgentSessionResult<SessionLane> {
    resolve_lane_credentials_with(lane, |profile_id, provider| {
        profiles::resolve_agent_api_key_profile(storage, profile_id, provider)
    })
}

fn resolve_lane_credentials_with<F>(
    lane: SessionLane,
    mut resolve: F,
) -> AgentSessionResult<SessionLane>
where
    F: FnMut(&str, &str) -> AgentSessionResult<String>,
{
    match lane {
        SessionLane::ClaudeApiKey {
            credential:
                SessionCredential::CredentialProfile {
                    credential_profile_id,
                },
            base_url,
        } => Ok(SessionLane::ClaudeApiKey {
            credential: SessionCredential::ApiKey {
                api_key: resolve(&credential_profile_id, "anthropic")?,
            },
            base_url,
        }),
        SessionLane::KimiApiKey {
            credential:
                SessionCredential::CredentialProfile {
                    credential_profile_id,
                },
        } => Ok(SessionLane::KimiApiKey {
            credential: SessionCredential::ApiKey {
                api_key: resolve(&credential_profile_id, "moonshot")?,
            },
        }),
        SessionLane::ZaiGlm {
            credential:
                SessionCredential::CredentialProfile {
                    credential_profile_id,
                },
            model_overrides,
        } => Ok(SessionLane::ZaiGlm {
            credential: SessionCredential::ApiKey {
                api_key: resolve(&credential_profile_id, "zai")?,
            },
            model_overrides,
        }),
        SessionLane::GrokApiKey {
            credential:
                SessionCredential::CredentialProfile {
                    credential_profile_id,
                },
            model_overrides,
        } => Ok(SessionLane::GrokApiKey {
            credential: SessionCredential::ApiKey {
                api_key: resolve(&credential_profile_id, "xai")?,
            },
            model_overrides,
        }),
        lane => Ok(lane),
    }
}

fn rehydrated_lane_credential_provider(lane: &SessionLane) -> Option<&'static str> {
    match lane {
        SessionLane::ClaudeApiKey { .. } => Some("anthropic"),
        SessionLane::KimiApiKey { .. } => Some("moonshot"),
        SessionLane::ZaiGlm { .. } => Some("zai"),
        SessionLane::GrokApiKey { .. } => Some("xai"),
        SessionLane::ClaudeSubscription
        | SessionLane::CodexChatGpt { .. }
        | SessionLane::KimiSubscription
        | SessionLane::Custom { .. } => None,
    }
}

fn resolve_rehydrated_lane_credentials_with<F>(
    lane: SessionLane,
    credential_profiles: &[CredentialProfile],
    resolve: F,
) -> AgentSessionResult<SessionLane>
where
    F: FnMut(&str, &str) -> AgentSessionResult<String>,
{
    let Some(provider) = rehydrated_lane_credential_provider(&lane) else {
        return Ok(lane);
    };
    let credential_profile_id = credential_profiles
        .iter()
        .filter(|profile| {
            profile.provider == provider && profile.kind == "api_key" && profile.status == "active"
        })
        .min_by(|left, right| {
            left.priority
                .cmp(&right.priority)
                .then_with(|| left.name.cmp(&right.name))
        })
        .map(|profile| profile.id.clone())
        .ok_or_else(|| format!("no active {provider} credential profile is available"))?;
    let lane = match lane {
        SessionLane::ClaudeApiKey { base_url, .. } => SessionLane::ClaudeApiKey {
            credential: SessionCredential::CredentialProfile {
                credential_profile_id,
            },
            base_url,
        },
        SessionLane::KimiApiKey { .. } => SessionLane::KimiApiKey {
            credential: SessionCredential::CredentialProfile {
                credential_profile_id,
            },
        },
        SessionLane::ZaiGlm {
            model_overrides, ..
        } => SessionLane::ZaiGlm {
            credential: SessionCredential::CredentialProfile {
                credential_profile_id,
            },
            model_overrides,
        },
        SessionLane::GrokApiKey {
            model_overrides, ..
        } => SessionLane::GrokApiKey {
            credential: SessionCredential::CredentialProfile {
                credential_profile_id,
            },
            model_overrides,
        },
        lane => lane,
    };
    resolve_lane_credentials_with(lane, resolve)
}

fn resolve_rehydrated_lane_credentials(
    storage: &Storage,
    lane: SessionLane,
) -> AgentSessionResult<SessionLane> {
    if rehydrated_lane_credential_provider(&lane).is_none() {
        return Ok(lane);
    }
    profiles::ensure_default_credential_profiles(storage);
    let credential_profiles = storage
        .list_credential_profiles()
        .map_err(|error| error.to_string())?;
    resolve_rehydrated_lane_credentials_with(lane, &credential_profiles, |profile_id, provider| {
        profiles::resolve_agent_api_key_profile(storage, profile_id, provider)
    })
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ZaiModelOverrides {
    sonnet: Option<String>,
    opus: Option<String>,
    haiku: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GrokModelOverrides {
    sonnet: Option<String>,
    opus: Option<String>,
    haiku: Option<String>,
}

impl SessionLane {
    fn kind(&self) -> SessionLaneKind {
        match self {
            Self::ClaudeSubscription => SessionLaneKind::ClaudeSubscription,
            Self::ClaudeApiKey { .. } => SessionLaneKind::ClaudeApiKey,
            Self::CodexChatGpt { .. } => SessionLaneKind::CodexChatGpt,
            Self::ZaiGlm { .. } => SessionLaneKind::ZaiGlm,
            Self::GrokApiKey { .. } => SessionLaneKind::GrokApiKey,
            Self::KimiApiKey { .. } => SessionLaneKind::KimiApiKey,
            Self::KimiSubscription => SessionLaneKind::KimiSubscription,
            Self::Custom { .. } => SessionLaneKind::Custom,
        }
    }

    fn uses_proxy(&self, agent_kind: AgentKind, claude_subscription_proxy: bool) -> bool {
        match self {
            Self::ClaudeSubscription => claude_subscription_proxy,
            Self::CodexChatGpt { proxied } => *proxied,
            Self::ClaudeApiKey { .. }
            | Self::ZaiGlm { .. }
            | Self::GrokApiKey { .. }
            | Self::KimiApiKey { .. }
            | Self::KimiSubscription => true,
            Self::Custom { env } => {
                !matches!(
                    agent_kind,
                    AgentKind::Codex | AgentKind::Gemini | AgentKind::Antigravity
                ) || env
                    .iter()
                    .rev()
                    .find(|(name, _)| name == "OPENAI_API_KEY")
                    .is_some_and(|(_, value)| !value.trim().is_empty())
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
/// The first user message rides as the CLI's positional prompt argument.
/// Typing it into the TUI after spawn raced the CLI's raw-mode setup, and
/// the CLI dropped the text. Resumed sessions get no prompt argument. Kimi
/// and Antigravity have no documented interactive prompt argument, so their
/// callers keep typed delivery.
pub(super) fn initial_prompt_arg(
    agent_kind: AgentKind,
    resume_cli_session_id: Option<&str>,
    initial_prompt: Option<&str>,
) -> Option<OsString> {
    if resume_cli_session_id.is_some() {
        return None;
    }
    if !adapter_for(agent_kind).accepts_initial_prompt() {
        return None;
    }
    initial_prompt
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(OsString::from)
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn launch_spec(
    agent_kind: AgentKind,
    lane: &SessionLane,
    cwd: &Path,
    extension_manager: &extensions::ExtensionManager,
    resume_cli_session_id: Option<&str>,
    assigned_cli_session_id: Option<&str>,
    permission_mode: Option<&str>,
    initial_prompt: Option<&str>,
    proxy_port: Option<u16>,
    headroom_project: &str,
    savings_database_path: &Path,
    nested_codex_home: Option<&Path>,
    ponytail_mode: PonytailMode,
    claude_subscription_proxy: bool,
    model: Option<&str>,
    conversation_id: Option<&str>,
    task_routing: bool,
    worker_lane: crate::preferences::WorkerLanePreference,
    tier_agents: crate::task_routing::TierAgentAssignments,
    workflow_routing: bool,
    claude_ultracode: bool,
    agent_status: bool,
) -> AgentSessionResult<LaunchSpec> {
    if !cwd.is_dir() {
        return Err(format!(
            "agent session cwd `{}` is not a directory",
            cwd.display()
        ));
    }
    let permission_mode = permission_mode
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(DEFAULT_PERMISSION_MODE);
    let permission_mode = canonical_permission_mode(permission_mode)?;
    let adapter = adapter_for(agent_kind);
    let context = InteractiveLaunchContext {
        cwd,
        lane,
        permission_mode,
        resume_cli_session_id,
        assigned_cli_session_id,
        proxy_port,
        project: headroom_project,
        savings_database_path,
        nested_codex_home,
        ponytail_mode,
        claude_subscription_proxy,
        model,
        conversation_id,
        task_routing,
        worker_lane,
        tier_agents,
        workflow_routing,
        claude_ultracode,
        extension_manager,
        agent_status,
    };
    let spec = adapter::launch_spec_with_prompt(adapter, &context, initial_prompt)?;
    let spec = add_savings_stack_to_launch_spec(spec, headroom_project);
    // Only a Halv pane reads the hook journal of its pane id.
    let pane_environment = if agent_status {
        crate::agent_status::pane_environment(headroom_project)
    } else {
        Vec::new()
    };
    Ok(add_agent_status_to_launch_spec(
        spec,
        pane_environment,
        crate::agent_status::orca_variables(),
    ))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SessionLaneKind {
    ClaudeSubscription,
    ClaudeApiKey,
    CodexChatGpt,
    ZaiGlm,
    GrokApiKey,
    KimiApiKey,
    KimiSubscription,
    Custom,
}

impl SessionLaneKind {
    pub(crate) fn storage_name(self) -> &'static str {
        match self {
            Self::ClaudeSubscription => "claude_subscription",
            Self::ClaudeApiKey => "claude_api_key",
            Self::CodexChatGpt => "codex_chatgpt",
            Self::ZaiGlm => "zai_glm",
            Self::GrokApiKey => "grok_api_key",
            Self::KimiApiKey => "kimi_api_key",
            Self::KimiSubscription => "kimi_subscription",
            Self::Custom => "custom",
        }
    }

    pub(crate) fn from_storage_name(value: &str) -> AgentSessionResult<Self> {
        match value {
            "claude_subscription" => Ok(Self::ClaudeSubscription),
            "claude_api_key" => Ok(Self::ClaudeApiKey),
            "codex_chatgpt" => Ok(Self::CodexChatGpt),
            "zai_glm" => Ok(Self::ZaiGlm),
            "grok_api_key" => Ok(Self::GrokApiKey),
            "kimi_api_key" => Ok(Self::KimiApiKey),
            "kimi_subscription" => Ok(Self::KimiSubscription),
            "custom" => Ok(Self::Custom),
            _ => Err(format!("failed to decode agent lane `{value}`")),
        }
    }

    fn rehydrated_lane(self) -> SessionLane {
        let empty_credential = || SessionCredential::ApiKey {
            api_key: String::new(),
        };
        match self {
            Self::ClaudeSubscription => SessionLane::ClaudeSubscription,
            Self::ClaudeApiKey => SessionLane::ClaudeApiKey {
                credential: empty_credential(),
                base_url: None,
            },
            Self::CodexChatGpt => SessionLane::CodexChatGpt { proxied: true },
            Self::ZaiGlm => SessionLane::ZaiGlm {
                credential: empty_credential(),
                model_overrides: None,
            },
            Self::GrokApiKey => SessionLane::GrokApiKey {
                credential: empty_credential(),
                model_overrides: None,
            },
            Self::KimiApiKey => SessionLane::KimiApiKey {
                credential: empty_credential(),
            },
            Self::KimiSubscription => SessionLane::KimiSubscription,
            Self::Custom => SessionLane::Custom { env: Vec::new() },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiVaultSessionTitle {
    pub session_id: String,
    pub title: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentSessionTitleState {
    pub default_title: String,
    pub generated_title: Option<String>,
    pub ai_vault_title: Option<AiVaultSessionTitle>,
    pub custom_title: Option<String>,
    pub generation: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentSession {
    pub id: String,
    pub default_title: String,
    pub generated_title: Option<String>,
    pub ai_vault_title: Option<AiVaultSessionTitle>,
    pub custom_title: Option<String>,
    pub generation: u64,
    pub agent_kind: AgentKind,
    pub lane_kind: SessionLaneKind,
    pub mode: SessionMode,
    pub ponytail_mode: Option<PonytailMode>,
    pub project_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conversation_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conversation_title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    pub cwd: String,
    pub pid: u32,
    pub status: String,
    pub cli_session_id: Option<String>,
    pub created_at: String,
    pub last_active_at: String,
    pub exit_code: Option<i32>,
    pub savings_sources: SavingsSources,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SavingsSourceState {
    Missing,
    Present,
    Instruction,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SavingsSources {
    pub rtk: SavingsSourceState,
    pub crux: SavingsSourceState,
    pub proxy: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SessionSavingsPaths {
    rtk: PathBuf,
    crux: PathBuf,
}

impl SessionSavingsPaths {
    fn from_paths(session_id: &str, stored_rtk: Option<&str>, stored_crux: Option<&str>) -> Self {
        Self {
            rtk: stored_rtk
                .map(PathBuf::from)
                .unwrap_or_else(|| exec::rtk_session_database_path(session_id)),
            crux: stored_crux
                .map(PathBuf::from)
                .unwrap_or_else(|| exec::crux_session_ledger_path(session_id)),
        }
    }
}

impl SavingsSources {
    fn from_paths(rtk: &Path, crux: &Path, proxy: bool, rtk_instruction: bool) -> Self {
        let state = |path| {
            if exec::savings_source_exists(path) {
                SavingsSourceState::Present
            } else {
                SavingsSourceState::Missing
            }
        };
        Self {
            rtk: if rtk_instruction {
                SavingsSourceState::Instruction
            } else {
                state(rtk)
            },
            crux: state(crux),
            proxy,
        }
    }
}

fn replace_savings_sources(current: &mut SavingsSources, next: SavingsSources) -> bool {
    if *current == next {
        return false;
    }
    *current = next;
    true
}

fn discover_session_savings_paths(row: &AgentSessionRow) -> SessionSavingsPaths {
    let mut paths = SessionSavingsPaths::from_paths(
        &row.id,
        row.rtk_db_path.as_deref(),
        row.crux_ledger_path.as_deref(),
    );

    if row.rtk_db_path.is_none() && !paths.rtk.is_file() {
        if let Some(found) = exec::find_sibling_rtk_state_file(&format!("session-{}.db", row.id)) {
            paths.rtk = found;
        }
    }
    if row.crux_ledger_path.is_none() && !paths.crux.is_file() {
        if let Some(found) =
            exec::find_sibling_rtk_state_file(&format!("crux-session-{}.jsonl", row.id))
        {
            paths.crux = found;
        }
    }

    paths
}

fn resolve_session_savings_paths(storage: &Storage, row: &AgentSessionRow) -> SessionSavingsPaths {
    let paths = discover_session_savings_paths(row);
    let rtk_update =
        (row.rtk_db_path.is_none() && paths.rtk.is_file()).then_some(paths.rtk.as_path());
    let crux_update =
        (row.crux_ledger_path.is_none() && paths.crux.is_file()).then_some(paths.crux.as_path());

    if let Err(error) = storage.update_agent_session_savings_paths(&row.id, rtk_update, crux_update)
    {
        let id = &row.id;
        eprintln!("failed to resolve savings paths for agent session `{id}`: {error}");
    }
    paths
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_creation_tokens: u64,
    pub total_cost_usd: Option<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum EmbeddedTurnOutcome {
    Completed,
    Aborted,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum AgentEvent {
    SessionInit {
        cli_session_id: String,
        model: String,
    },
    TurnStarted {
        #[serde(skip_serializing_if = "Option::is_none")]
        turn_id: Option<String>,
    },
    EmbeddedTurnStarted {
        #[serde(skip_serializing_if = "Option::is_none")]
        turn_id: Option<String>,
    },
    EmbeddedTurnFinished {
        #[serde(skip_serializing_if = "Option::is_none")]
        turn_id: Option<String>,
        outcome: EmbeddedTurnOutcome,
    },
    TextDelta {
        text: String,
    },
    ToolCallStarted {
        name: String,
        summary_json: String,
    },
    ToolCallCompleted {
        name: String,
    },
    #[allow(dead_code)]
    PermissionRequest {
        request_id: String,
        description: String,
        payload_json: String,
    },
    PermissionResolved {
        request_id: String,
        approved: bool,
    },
    TurnCompleted {
        usage: TurnUsage,
    },
    SavingsSample {
        savings_tokens: u64,
        savings_source: String,
        #[serde(default)]
        cache_read_tokens: u64,
        #[serde(default)]
        turn_boundary: bool,
        /// Dollars saved by this sample with the methods of ADR 0015.
        #[serde(default)]
        dollars_usd: f64,
    },
    SavingsSourcesChanged {
        savings_sources: SavingsSources,
    },
    SessionExited {
        code: Option<i32>,
    },
    ProtocolError {
        message: String,
    },
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct AgentSessionEvent {
    session_id: String,
    event: AgentEvent,
}

/// How a launched session uses the Halv Engine. Halv saves it in the session
/// row, so the next launch knows which restored sessions need the engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HeadroomRoute {
    /// Nothing in the session environment points at an engine.
    Direct,
    /// The session traffic goes through the shared engine.
    Engine,
    /// The session traffic goes direct, but its nested Codex home routes
    /// the JEV Codex workers through the shared engine.
    Workers,
    /// The session traffic goes through an isolated engine (Kimi). That
    /// engine stops with the app and its port is not saved.
    Isolated,
}

impl HeadroomRoute {
    fn storage_name(self) -> &'static str {
        match self {
            Self::Direct => "direct",
            Self::Engine => "engine",
            Self::Workers => "workers",
            Self::Isolated => "isolated",
        }
    }

    fn from_storage_name(value: &str) -> Option<Self> {
        match value {
            "direct" => Some(Self::Direct),
            "engine" => Some(Self::Engine),
            "workers" => Some(Self::Workers),
            "isolated" => Some(Self::Isolated),
            _ => None,
        }
    }

    /// The route of a launch: `meter` is the Headroom meter of the session
    /// and `proxy_source` tells if the session traffic goes through it.
    fn of_launch(meter: Option<&SessionSavingsMeter>, proxy_source: bool) -> Self {
        match meter {
            None => Self::Direct,
            Some(meter) if meter.shutdown_on_exit => Self::Isolated,
            Some(_) if proxy_source => Self::Engine,
            Some(_) => Self::Workers,
        }
    }

    /// The route of a row from 1.0.0 or older. The engine was always on
    /// then, so the lane and the stored subscription opt-in tell the route.
    fn legacy(
        agent_kind: AgentKind,
        lane_kind: SessionLaneKind,
        claude_subscription_proxy: bool,
    ) -> Self {
        match (agent_kind, lane_kind) {
            (AgentKind::Kimi, SessionLaneKind::KimiApiKey | SessionLaneKind::KimiSubscription) => {
                Self::Isolated
            }
            (AgentKind::Codex, SessionLaneKind::CodexChatGpt)
            | (
                AgentKind::Claude | AgentKind::Zai,
                SessionLaneKind::ClaudeApiKey | SessionLaneKind::ZaiGlm | SessionLaneKind::Custom,
            )
            | (AgentKind::Grok, SessionLaneKind::GrokApiKey) => Self::Engine,
            (AgentKind::Claude | AgentKind::Zai, SessionLaneKind::ClaudeSubscription) => {
                if claude_subscription_proxy {
                    Self::Engine
                } else {
                    Self::Workers
                }
            }
            _ => Self::Direct,
        }
    }

    /// True when something in the session environment points at the port of
    /// the shared engine, so a restored session needs it to run.
    fn needs_shared_engine(self) -> bool {
        matches!(self, Self::Engine | Self::Workers)
    }

    /// The proxy source of the savings meter.
    fn proxy_source(self) -> bool {
        matches!(self, Self::Engine | Self::Isolated)
    }
}

/// The Halv Engine route of one session. With the engine off, `sidecar` and
/// `port` are `None`: the lane goes direct to its upstream, and the context
/// only holds the nested Codex home that measures JEV worker usage.
#[derive(Clone)]
struct SessionProxyContext {
    sidecar: Option<SidecarManager>,
    port: Option<u16>,
    shutdown_on_exit: bool,
    nested_codex_home: Option<PathBuf>,
}

impl SessionProxyContext {
    fn direct() -> Self {
        Self {
            sidecar: None,
            port: None,
            shutdown_on_exit: false,
            nested_codex_home: None,
        }
    }

    fn routed(&self) -> bool {
        self.port.is_some()
    }

    /// A Headroom meter exists only when the session uses an engine.
    fn savings_meter(&self, project: &str) -> Option<SessionSavingsMeter> {
        self.sidecar.clone().map(|sidecar| SessionSavingsMeter {
            sidecar,
            project: project.to_string(),
            before: Mutex::new(None),
            shutdown_on_exit: self.shutdown_on_exit,
        })
    }
}

struct SessionSavingsMeter {
    sidecar: SidecarManager,
    project: String,
    before: Mutex<Option<ProjectSavingsTotals>>,
    shutdown_on_exit: bool,
}

#[derive(Clone, Copy, Default)]
struct TerminalSavingsSnapshot {
    proxy: Option<(u64, u64, u64)>,
    rtk: Option<u64>,
    crux: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TerminalSavingsSample {
    /// True for a Headroom sample; false for RTK and crux.
    from_proxy: bool,
    savings_tokens: u64,
    crux_tokens_saved: u64,
    cache_read_tokens: u64,
    turn_boundary: bool,
}

#[derive(Default)]
struct TerminalSavingsState {
    previous_proxy: u64,
    previous_requests: u64,
    previous_cache: u64,
    previous_rtk: u64,
    previous_crux: u64,
}

impl TerminalSavingsState {
    fn apply(&mut self, snapshot: TerminalSavingsSnapshot) -> Vec<TerminalSavingsSample> {
        let mut samples = Vec::new();
        if let Some((requests, tokens, cache_read)) = snapshot.proxy {
            let cache_growth = cache_read.saturating_sub(self.previous_cache);
            let proxy_sample = proxy_savings_sample(
                &mut self.previous_requests,
                requests,
                &mut self.previous_proxy,
                tokens,
            );
            if let Some((savings_tokens, turn_boundary)) = proxy_sample {
                samples.push(TerminalSavingsSample {
                    from_proxy: true,
                    savings_tokens,
                    crux_tokens_saved: 0,
                    cache_read_tokens: cache_growth,
                    turn_boundary,
                });
            } else if cache_growth > 0 {
                samples.push(TerminalSavingsSample {
                    from_proxy: true,
                    savings_tokens: 0,
                    crux_tokens_saved: 0,
                    cache_read_tokens: cache_growth,
                    turn_boundary: false,
                });
            }
            self.previous_cache = cache_read;
        }
        if let Some(tokens) = snapshot.rtk {
            let mut savings_tokens = None;
            apply_project_savings_sample(&mut self.previous_rtk, tokens, |savings| {
                savings_tokens = Some(savings);
            });
            if let Some(savings_tokens) = savings_tokens {
                samples.push(TerminalSavingsSample {
                    from_proxy: false,
                    savings_tokens,
                    crux_tokens_saved: 0,
                    cache_read_tokens: 0,
                    turn_boundary: false,
                });
            }
        }
        if let Some(tokens) = snapshot.crux {
            let mut savings_tokens = None;
            apply_project_savings_sample(&mut self.previous_crux, tokens, |savings| {
                savings_tokens = Some(savings);
            });
            if let Some(savings_tokens) = savings_tokens {
                samples.push(TerminalSavingsSample {
                    from_proxy: false,
                    savings_tokens,
                    crux_tokens_saved: savings_tokens,
                    cache_read_tokens: 0,
                    turn_boundary: false,
                });
            }
        }
        samples
    }
}

fn sample_terminal_savings_once(
    state: &Mutex<TerminalSavingsState>,
    finished: &AtomicBool,
    finalize: bool,
    read_snapshot: impl FnOnce() -> TerminalSavingsSnapshot,
    mut record: impl FnMut(TerminalSavingsSample),
) {
    let mut state = state.lock().expect("terminal savings state lock poisoned");
    if finished.load(Ordering::SeqCst) {
        return;
    }
    for sample in state.apply(read_snapshot()) {
        record(sample);
    }
    if finalize {
        finished.store(true, Ordering::SeqCst);
    }
}

#[derive(Default)]
struct AgentSavingsSettleTracker {
    last: Option<ProjectSavingsTotals>,
    stable_samples: usize,
}

impl AgentSavingsSettleTracker {
    fn observe(
        &mut self,
        before: ProjectSavingsTotals,
        after: ProjectSavingsTotals,
        elapsed: Duration,
    ) -> bool {
        if self.last == Some(after) {
            self.stable_samples += 1;
        } else {
            self.last = Some(after);
            self.stable_samples = 1;
        }
        let tokens_increased = after.tokens_saved.saturating_sub(before.tokens_saved) > 0;
        let usd_increased = match (before.savings_usd, after.savings_usd) {
            (Some(before), Some(after)) => after > before,
            _ => false,
        };
        self.stable_samples >= SAVINGS_SETTLE_STABLE_SAMPLES
            && (tokens_increased || usd_increased || elapsed >= SAVINGS_SETTLE_ZERO_GRACE)
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct MeasuredSavings {
    tokens_saved: u64,
    savings_usd: Option<f64>,
    /// The RTK and crux tokens inside `tokens_saved`. They count once, at
    /// the input price (ADR 0015). The rest are Headroom tokens.
    once_tokens: u64,
}

/// The transcript facts that price one saved turn (ADR 0015).
#[derive(Debug, Clone, Default, PartialEq)]
struct TurnPriceContext {
    observed_model: Option<String>,
    window_rate: Option<f64>,
    running_rate: Option<f64>,
    cache_write_1h_tokens: u64,
}

#[derive(Debug)]
pub(crate) struct ConversationAgentTurnResult {
    pub assistant_message: String,
    pub usage: TurnUsage,
}

struct ConversationTurnBridge {
    conversation_id: String,
    model: String,
    assistant_message: String,
    completion: Option<oneshot::Sender<AgentSessionResult<ConversationAgentTurnResult>>>,
}

#[derive(Debug, Clone, PartialEq)]
enum ConversationEventMapping {
    Delta(String),
    Completed(TurnUsage),
    Error(String),
    Ignore,
}

fn humanize_tool_name(name: &str) -> String {
    if name.contains('/') || name.chars().any(char::is_whitespace) {
        return name.to_string();
    }

    let mut humanized = String::with_capacity(name.len());
    let mut characters = name.chars().peekable();
    let mut previous = None;

    while let Some(character) = characters.next() {
        let next_is_lowercase = characters
            .peek()
            .map(|next| next.is_lowercase())
            .unwrap_or(false);
        let starts_word = previous
            .map(|previous: char| {
                character.is_uppercase() && (previous.is_lowercase() || next_is_lowercase)
            })
            .unwrap_or(false);

        if starts_word {
            humanized.push(' ');
        }
        if previous.is_none() || starts_word {
            humanized.extend(character.to_uppercase());
        } else {
            humanized.push(character);
        }

        previous = Some(character);
    }

    humanized
}

fn tool_call_detail(summary_json: &str) -> Option<String> {
    let summary = serde_json::from_str::<serde_json::Value>(summary_json).ok()?;
    let object = summary.as_object()?;

    for key in [
        "command",
        "query",
        "file_path",
        "path",
        "pattern",
        "url",
        "description",
    ] {
        let Some(value) = object.get(key).and_then(serde_json::Value::as_str) else {
            continue;
        };
        let normalized = value
            .replace('`', "")
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        if normalized.is_empty() {
            continue;
        }

        let mut chars = normalized.chars();
        let mut detail = chars.by_ref().take(96).collect::<String>();
        if chars.next().is_some() {
            detail.push('…');
        }
        return Some(detail);
    }

    None
}

fn map_agent_event_to_conversation(
    agent_kind: AgentKind,
    event: &AgentEvent,
) -> ConversationEventMapping {
    match event {
        AgentEvent::TextDelta { text } => ConversationEventMapping::Delta(text.clone()),
        AgentEvent::ToolCallStarted { name, summary_json } => {
            let human_name = humanize_tool_name(name);
            ConversationEventMapping::Delta(match tool_call_detail(summary_json) {
                Some(detail) => format!("\n\n_{human_name}: {detail}._\n\n"),
                None => format!("\n\n_{human_name}._\n\n"),
            })
        }
        AgentEvent::TurnCompleted { usage } => ConversationEventMapping::Completed(usage.clone()),
        AgentEvent::ProtocolError { message } => ConversationEventMapping::Error(message.clone()),
        AgentEvent::SessionExited { code } => ConversationEventMapping::Error(format!(
            "{} exited before the turn completed{}",
            agent_kind.display_name(),
            code.map(|value| format!(" with status {value}"))
                .unwrap_or_default()
        )),
        _ => ConversationEventMapping::Ignore,
    }
}

fn conversation_token_usage(usage: &TurnUsage) -> serde_json::Value {
    serde_json::json!({
        "inputTokens": u64_to_i64(usage.input_tokens),
        "outputTokens": u64_to_i64(usage.output_tokens),
        "totalTokens": u64_to_i64(
            usage.input_tokens.saturating_add(usage.output_tokens),
        ),
        "cachedTokens": u64_to_i64(usage.cache_read_tokens),
        "reasoningTokens": 0,
    })
}

struct ManagedAgentSession {
    record: Mutex<AgentSession>,
    adapter: &'static dyn AgentAdapter,
    adapter_state: Mutex<AdapterState>,
    lane: SessionLane,
    credentials_rehydrated: bool,
    permission_mode: String,
    writer: Mutex<Option<ProtocolWriter>>,
    process: Mutex<Option<Child>>,
    stderr_output: Arc<Mutex<StderrTail>>,
    closing: AtomicBool,
    finishing: AtomicBool,
    finished: AtomicBool,
    // True after a waiter thread owns the child reap for this session.
    process_waiter_spawned: AtomicBool,
    active_sessions: Arc<AtomicUsize>,
    turn_index: AtomicUsize,
    savings_meter: Option<SessionSavingsMeter>,
    terminal_savings: Option<Mutex<TerminalSavingsState>>,
    // Session-scoped RTK and crux ledgers share one delta state. GUI turns
    // consume these deltas when their Headroom savings settle.
    rtk_session_db: Option<std::path::PathBuf>,
    crux_session_ledger: Option<std::path::PathBuf>,
    rtk_source_path: std::path::PathBuf,
    crux_source_path: std::path::PathBuf,
    proxy_source: bool,
    rtk_instruction: bool,
    memory_generation_effort: String,
    memories_injected: AtomicBool,
    conversation_turn: Mutex<Option<ConversationTurnBridge>>,
    nested_codex_usage_before: Mutex<Option<TurnUsage>>,
    nested_codex_home: Option<PathBuf>,
    // The transcript reader of ADR 0015, and the window that the current
    // turn already read.
    usage_meter: Mutex<usage_meter::SessionUsageMeter>,
    pending_usage_window: Mutex<Option<usage_meter::PricedUsage>>,
}

fn latest_user_message_for_memory(
    storage: &Storage,
    conversation_id: &str,
) -> AgentSessionResult<String> {
    storage
        .get_messages(conversation_id)
        .map_err(|error| error.to_string())
        .map(|messages| {
            messages
                .into_iter()
                .rev()
                .find(|message| message.role == "user" && !message.error)
                .map(|message| message.content)
                .unwrap_or_default()
        })
}

fn prefix_memories_for_first_turn(
    memories_injected: &AtomicBool,
    original_text: &str,
    memory_instructions: impl FnOnce() -> Option<String>,
) -> String {
    if memories_injected.load(Ordering::SeqCst) {
        return original_text.to_string();
    }
    let Some(memory_instructions) = memory_instructions() else {
        return original_text.to_string();
    };
    memories_injected.store(true, Ordering::SeqCst);
    format!("<halv-memories>\n{memory_instructions}\n</halv-memories>\n\n{original_text}")
}

#[cfg(test)]
fn find_pending_permission<'a>(
    pending: &'a HashMap<String, adapter::PendingPermission>,
    request_id: &str,
) -> AgentSessionResult<&'a adapter::PendingPermission> {
    pending
        .get(request_id)
        .ok_or_else(|| format!("permission request `{request_id}` is unknown or already resolved"))
}

#[cfg(test)]
fn drain_pending_permission_events(
    pending: &mut HashMap<String, adapter::PendingPermission>,
) -> Vec<AgentEvent> {
    let mut request_ids = pending
        .drain()
        .map(|(request_id, _)| request_id)
        .collect::<Vec<_>>();
    request_ids.sort();
    request_ids
        .into_iter()
        .map(|request_id| AgentEvent::PermissionResolved {
            request_id,
            approved: false,
        })
        .collect()
}

impl ManagedAgentSession {
    fn snapshot(&self) -> AgentSession {
        self.record
            .lock()
            .expect("agent session record lock poisoned")
            .clone()
    }

    fn refresh_savings_sources(&self, app: &AppHandle) {
        let savings_sources = SavingsSources::from_paths(
            &self.rtk_source_path,
            &self.crux_source_path,
            self.proxy_source,
            self.rtk_instruction,
        );
        let changed = {
            let mut record = self
                .record
                .lock()
                .expect("agent session record lock poisoned");
            replace_savings_sources(&mut record.savings_sources, savings_sources)
        };
        if changed {
            self.emit(app, AgentEvent::SavingsSourcesChanged { savings_sources });
        }
    }

    fn set_status(&self, status: &str) -> AgentSession {
        let mut record = self
            .record
            .lock()
            .expect("agent session record lock poisoned");
        record.status = status.to_string();
        record.last_active_at = Utc::now().to_rfc3339();
        record.clone()
    }

    fn begin_codex_init_flush(&self) -> bool {
        self.adapter_state
            .lock()
            .expect("agent adapter state lock poisoned")
            .begin_init_flush()
    }

    fn remember_codex_protocol_error(&self, message: &str) {
        self.adapter_state
            .lock()
            .expect("agent adapter state lock poisoned")
            .remember_protocol_error(message);
    }

    fn flush_codex_init_messages(&self, cli_session_id: &str) -> AgentSessionResult<()> {
        loop {
            let line = {
                let mut state = self
                    .adapter_state
                    .lock()
                    .expect("agent adapter state lock poisoned");
                let Some(message) = state.next_queued_message() else {
                    return Ok(());
                };
                self.adapter.encode_user_message(
                    &mut state,
                    &message,
                    TurnKind::Queued { cli_session_id },
                )?
            };
            if let Some(line) = line {
                self.write_protocol_line(&line)?;
            }
        }
    }

    fn fail_codex_initialization(&self, app: &AppHandle, message: String) {
        if self.finished.load(Ordering::SeqCst) {
            return;
        }
        self.adapter_state
            .lock()
            .expect("agent adapter state lock poisoned")
            .mark_init_failed();
        self.set_status("failed");
        self.persist(app);
        self.handle_event(app, AgentEvent::ProtocolError { message });
        self.writer
            .lock()
            .expect("agent session writer lock poisoned")
            .take();
        if self.process_waiter_spawned.load(Ordering::SeqCst) {
            let mut process = self
                .process
                .lock()
                .expect("agent session process lock poisoned");
            if let Some(child) = process.as_mut() {
                let _ = child.kill();
            }
        } else {
            // No waiter thread exists. Reap the child here so it cannot
            // stay a zombie, then finish the session.
            stop_managed_child(self);
            self.finish(app, None, false);
        }
    }

    fn persist_with_storage(&self, storage: &Storage) -> AgentSessionResult<()> {
        persist_snapshot(storage, &self.snapshot())
    }

    fn persist_with_storage_and_savings_paths(&self, storage: &Storage) -> AgentSessionResult<()> {
        self.persist_with_storage(storage)?;
        let snapshot = self.snapshot();
        let id = snapshot.id;
        let billing =
            crate::storage::savings_dollars::lane_billing(snapshot.lane_kind.storage_name());
        if let Err(error) = storage.set_agent_session_billing(&id, billing) {
            eprintln!("failed to save the billing of agent session `{id}`: {error}");
        }
        let route =
            HeadroomRoute::of_launch(self.savings_meter.as_ref(), snapshot.savings_sources.proxy);
        if let Err(error) = storage.set_agent_session_headroom_route(&id, route.storage_name()) {
            eprintln!("failed to save the Halv Engine route of agent session `{id}`: {error}");
        }
        storage
            .update_agent_session_savings_paths(
                &id,
                Some(&self.rtk_source_path),
                Some(&self.crux_source_path),
            )
            .map_err(|error| error.to_string())
    }

    fn persist(&self, app: &AppHandle) {
        let Some(storage) = app.try_state::<Storage>() else {
            return;
        };
        if let Err(error) = self.persist_with_storage(&storage) {
            eprintln!("failed to persist agent session: {error}");
        }
    }

    fn emit(&self, app: &AppHandle, event: AgentEvent) {
        let snapshot = self.snapshot();
        if let AgentEvent::TurnStarted { turn_id } = &event {
            crate::checkpoints::spawn_turn_checkpoint(
                app,
                snapshot.id.clone(),
                snapshot.cwd.clone(),
                turn_id.clone(),
            );
        }
        let payload = AgentSessionEvent {
            session_id: snapshot.id,
            event,
        };
        let _ = app.emit(AGENT_SESSION_EVENT, payload);
    }

    fn tag_conversation(&self, conversation_id: &str, conversation_title: &str, model: &str) {
        let mut record = self
            .record
            .lock()
            .expect("agent session record lock poisoned");
        record.conversation_id = Some(conversation_id.to_string());
        record.conversation_title = Some(conversation_title.to_string());
        record.model = Some(model.to_string());
    }

    fn set_cli_session_id(&self, cli_session_id: &str) {
        self.record
            .lock()
            .expect("agent session record lock poisoned")
            .cli_session_id = Some(cli_session_id.to_string());
    }

    fn begin_conversation_turn(
        &self,
        conversation_id: &str,
        model: &str,
    ) -> AgentSessionResult<oneshot::Receiver<AgentSessionResult<ConversationAgentTurnResult>>>
    {
        let mut bridge = self
            .conversation_turn
            .lock()
            .expect("conversation turn lock poisoned");
        if bridge.is_some() {
            return Err(format!(
                "the {} conversation already has an active turn",
                self.snapshot().agent_kind.display_name()
            ));
        }
        let (completion, receiver) = oneshot::channel();
        *bridge = Some(ConversationTurnBridge {
            conversation_id: conversation_id.to_string(),
            model: model.to_string(),
            assistant_message: String::new(),
            completion: Some(completion),
        });
        Ok(receiver)
    }

    fn has_active_conversation_turn(&self, conversation_id: &str) -> bool {
        self.conversation_turn
            .lock()
            .expect("conversation turn lock poisoned")
            .as_ref()
            .is_some_and(|turn| turn.conversation_id == conversation_id)
    }

    fn cancel_conversation_turn(&self, message: &str) {
        let mut bridge = self
            .conversation_turn
            .lock()
            .expect("conversation turn lock poisoned");
        if let Some(mut turn) = bridge.take() {
            if let Some(completion) = turn.completion.take() {
                let _ = completion.send(Err(message.to_string()));
            }
        }
    }

    fn handle_conversation_event(
        &self,
        app: &AppHandle,
        event: &AgentEvent,
        savings: Option<MeasuredSavings>,
        crux_tokens_saved: u64,
    ) -> bool {
        match map_agent_event_to_conversation(self.snapshot().agent_kind, event) {
            ConversationEventMapping::Delta(text) => {
                let conversation_id = {
                    let mut bridge = self
                        .conversation_turn
                        .lock()
                        .expect("conversation turn lock poisoned");
                    let Some(turn) = bridge.as_mut() else {
                        return false;
                    };
                    turn.assistant_message.push_str(&text);
                    turn.conversation_id.clone()
                };
                let _ = app.emit(
                    "chat_delta",
                    serde_json::json!({
                        "conversationId": conversation_id,
                        "kind": "delta",
                        "text": text,
                    }),
                );
                false
            }
            ConversationEventMapping::Completed(usage) => {
                let turn = self
                    .conversation_turn
                    .lock()
                    .expect("conversation turn lock poisoned")
                    .take();
                let Some(mut turn) = turn else {
                    return false;
                };
                let result = self.persist_conversation_assistant(
                    app,
                    &turn,
                    &usage,
                    savings,
                    crux_tokens_saved,
                );
                match result {
                    Ok(has_message_snapshot) => {
                        let _ = app.emit(
                            "chat_delta",
                            serde_json::json!({
                                "conversationId": turn.conversation_id,
                                "kind": "done",
                                "usage": conversation_token_usage(&usage),
                            }),
                        );
                        if !turn.assistant_message.trim().is_empty() {
                            match app.try_state::<Storage>() {
                                Some(storage) => match latest_user_message_for_memory(
                                    storage.inner(),
                                    &turn.conversation_id,
                                ) {
                                    Ok(recent_user_message) => {
                                        chat::schedule_agent_memory_generation_for_turn(
                                            app,
                                            &turn.conversation_id,
                                            &turn.model,
                                            &self.memory_generation_effort,
                                            &recent_user_message,
                                            &turn.assistant_message,
                                        );
                                    }
                                    Err(error) => eprintln!(
                                        "Automatic memory generation failed for {}: could not load the recent user message: {error}",
                                        turn.conversation_id
                                    ),
                                },
                                None => eprintln!(
                                    "Automatic memory generation failed for {}: conversation storage is unavailable",
                                    turn.conversation_id
                                ),
                            }
                        }
                        if let Some(completion) = turn.completion.take() {
                            let _ = completion.send(Ok(ConversationAgentTurnResult {
                                assistant_message: turn.assistant_message,
                                usage,
                            }));
                        }
                        has_message_snapshot
                    }
                    Err(error) => {
                        if let Some(completion) = turn.completion.take() {
                            let _ = completion.send(Err(error));
                        }
                        false
                    }
                }
            }
            ConversationEventMapping::Error(message) => {
                self.finish_conversation_error(app, &message);
                false
            }
            ConversationEventMapping::Ignore => false,
        }
    }

    fn persist_conversation_assistant(
        &self,
        app: &AppHandle,
        turn: &ConversationTurnBridge,
        usage: &TurnUsage,
        savings: Option<MeasuredSavings>,
        crux_tokens_saved: u64,
    ) -> AgentSessionResult<bool> {
        let storage = app
            .try_state::<Storage>()
            .ok_or_else(|| "conversation storage is unavailable".to_string())?;
        let ponytail_mode = self.snapshot().ponytail_mode;
        let assistant = storage
            .insert_message_with_ponytail(
                NewMessage {
                    conversation_id: &turn.conversation_id,
                    role: "assistant",
                    content: &turn.assistant_message,
                    model: Some(&turn.model),
                    input_tokens: Some(u64_to_i64(usage.input_tokens)),
                    output_tokens: Some(u64_to_i64(usage.output_tokens)),
                    error: false,
                    mentions: None,
                },
                ponytail_mode,
            )
            .map_err(|error| error.to_string())?;
        storage
            .upsert_agent_message_usage(
                &assistant.id,
                u64_to_i64(usage.cache_read_tokens),
                if savings.is_some() {
                    "measured"
                } else {
                    "usage"
                },
            )
            .map_err(|error| error.to_string())?;
        let has_message_snapshot =
            if let Some(savings) = savings.filter(|savings| savings.tokens_saved > 0) {
                let snapshot = self.snapshot();
                // The turn row reads the same window later, so peek here.
                let price_context = self.price_context_from(self.peek_usage_window(app));
                let priced = price_turn_savings(
                    snapshot.agent_kind,
                    snapshot.mode,
                    price_context
                        .observed_model
                        .as_deref()
                        .or(Some(turn.model.as_str())),
                    usage,
                    &price_context,
                    savings,
                    crux_tokens_saved,
                );
                // The snapshot stores crux tokens in their own column. Pass only
                // the non-crux share from the combined turn measurement.
                let non_crux_savings = measured_savings_without_crux(savings, crux_tokens_saved);
                storage
                    .insert_priced_savings_snapshot(
                        &assistant.id,
                        u64_to_i64(non_crux_savings.tokens_saved),
                        u64_to_i64(crux_tokens_saved),
                        &SnapshotDollars {
                            usd: priced.usd,
                            source: priced.source,
                            usd_method: priced.method,
                            price_version: &priced.price_version,
                        },
                    )
                    .map_err(|error| error.to_string())?;
                true
            } else {
                false
            };
        Ok(has_message_snapshot)
    }

    fn finish_conversation_error(&self, app: &AppHandle, message: &str) {
        let turn = self
            .conversation_turn
            .lock()
            .expect("conversation turn lock poisoned")
            .take();
        let Some(mut turn) = turn else {
            return;
        };
        if let Some(storage) = app.try_state::<Storage>() {
            let _ = storage.insert_message_with_ponytail(
                NewMessage {
                    conversation_id: &turn.conversation_id,
                    role: "assistant",
                    content: message,
                    model: Some(&turn.model),
                    input_tokens: None,
                    output_tokens: None,
                    error: true,
                    mentions: None,
                },
                self.snapshot().ponytail_mode,
            );
        }
        let _ = app.emit(
            "chat_delta",
            serde_json::json!({
                "conversationId": turn.conversation_id,
                "kind": "error",
                "text": message,
            }),
        );
        if let Some(completion) = turn.completion.take() {
            let _ = completion.send(Err(message.to_string()));
        }
    }

    fn handle_event(&self, app: &AppHandle, mut event: AgentEvent) {
        let session_init_id = match &event {
            AgentEvent::SessionInit { cli_session_id, .. } => {
                if !self.begin_codex_init_flush() {
                    return;
                }
                Some(cli_session_id.clone())
            }
            AgentEvent::ProtocolError { message } => {
                self.remember_codex_protocol_error(message);
                None
            }
            _ => None,
        };
        let (savings, crux_tokens_saved) = if let AgentEvent::TurnCompleted { usage } = &mut event {
            let savings = self.measure_savings_after_turn();
            let claude_usage = self.measure_claude_usage_after_turn();
            let nested_codex_usage = self.measure_nested_codex_usage_after_turn();
            self.apply_measured_usage(usage, claude_usage, nested_codex_usage);
            savings
        } else {
            (None, 0)
        };
        match &event {
            AgentEvent::TurnStarted { turn_id } => {
                self.adapter_state
                    .lock()
                    .expect("agent adapter state lock poisoned")
                    .set_active_turn(turn_id.clone());
            }
            AgentEvent::TurnCompleted { .. } | AgentEvent::SessionExited { .. } => {
                self.adapter_state
                    .lock()
                    .expect("agent adapter state lock poisoned")
                    .set_active_turn(None);
            }
            _ => {}
        }
        let mut persist_session = false;
        let mut completed_usage = None;
        {
            let mut record = self
                .record
                .lock()
                .expect("agent session record lock poisoned");
            record.last_active_at = Utc::now().to_rfc3339();
            match &event {
                AgentEvent::SessionInit { cli_session_id, .. } => {
                    record.cli_session_id = Some(cli_session_id.clone());
                    record.status = "running".to_string();
                    persist_session = true;
                }
                AgentEvent::TurnCompleted { usage } => {
                    record.status = "waiting_input".to_string();
                    persist_session = true;
                    completed_usage = Some(usage.clone());
                }
                AgentEvent::PermissionRequest { .. } => {
                    record.status = "waiting_input".to_string();
                    persist_session = true;
                }
                AgentEvent::PermissionResolved { .. } => {
                    if self
                        .adapter_state
                        .lock()
                        .expect("agent adapter state lock poisoned")
                        .pending_permissions_empty()
                    {
                        record.status = "running".to_string();
                    }
                    persist_session = true;
                }
                _ => {}
            }
        }

        if persist_session {
            self.persist(app);
        }
        self.refresh_savings_sources(app);
        let has_message_snapshot =
            self.handle_conversation_event(app, &event, savings, crux_tokens_saved);
        if let Some(usage) = completed_usage {
            self.persist_turn(
                app,
                &usage,
                savings,
                crux_tokens_saved,
                has_message_snapshot,
            );
        }
        self.emit(app, event);
        if let Some(cli_session_id) = session_init_id {
            if let Err(error) = self.flush_codex_init_messages(&cli_session_id) {
                self.fail_codex_initialization(
                    app,
                    format!("failed to flush queued Codex message after initialization: {error}"),
                );
            }
        }
    }

    /// Reads the transcript usage of the current turn. Later calls in the
    /// same turn return the same window.
    fn peek_usage_window(&self, app: &AppHandle) -> usage_meter::PricedUsage {
        let mut pending = self
            .pending_usage_window
            .lock()
            .expect("usage window lock poisoned");
        pending
            .get_or_insert_with(|| self.poll_usage_meter(app))
            .clone()
    }

    /// Reads the transcript usage since the previous saved turn and ends the
    /// window.
    fn take_usage_window(&self, app: &AppHandle) -> usage_meter::PricedUsage {
        let pending = self
            .pending_usage_window
            .lock()
            .expect("usage window lock poisoned")
            .take();
        pending.unwrap_or_else(|| self.poll_usage_meter(app))
    }

    /// The transcript facts of the current turn, for the dollar methods.
    fn turn_price_context(&self, app: &AppHandle) -> TurnPriceContext {
        let window = self.take_usage_window(app);
        self.price_context_from(window)
    }

    fn price_context_from(&self, window: usage_meter::PricedUsage) -> TurnPriceContext {
        let running = self
            .usage_meter
            .lock()
            .expect("usage meter lock poisoned")
            .running()
            .clone();
        TurnPriceContext {
            observed_model: window.latest_model.clone().or(running.latest_model.clone()),
            window_rate: window.prompt_rate_usd_per_million(),
            running_rate: running.prompt_rate_usd_per_million(),
            cache_write_1h_tokens: window.usage.cache_write_1h,
        }
    }

    fn poll_usage_meter(&self, app: &AppHandle) -> usage_meter::PricedUsage {
        let snapshot = self.snapshot();
        let window = self
            .usage_meter
            .lock()
            .expect("usage meter lock poisoned")
            .poll(snapshot.cli_session_id.as_deref(), Path::new(&snapshot.cwd));
        if let (Some(model), Some(storage)) =
            (window.latest_model.as_deref(), app.try_state::<Storage>())
        {
            if let Err(error) = storage.set_agent_session_observed_model(&snapshot.id, model) {
                eprintln!("failed to save the observed model of an agent session: {error}");
            }
        }
        window
    }

    /// Saves the turn and returns its dollars saved.
    fn persist_turn(
        &self,
        app: &AppHandle,
        usage: &TurnUsage,
        savings: Option<MeasuredSavings>,
        crux_tokens_saved: u64,
        has_message_snapshot: bool,
    ) -> f64 {
        let Some(storage) = app.try_state::<Storage>() else {
            return 0.0;
        };
        let price_context = self.turn_price_context(app);
        let snapshot = self.snapshot();
        let session_id = snapshot.id;
        let turn_index = self.turn_index.fetch_add(1, Ordering::SeqCst);
        let created_at = Utc::now().to_rfc3339();
        match persist_agent_turn(
            &storage,
            &session_id,
            snapshot.agent_kind,
            snapshot.model.as_deref(),
            snapshot.mode,
            snapshot.ponytail_mode,
            turn_index,
            usage,
            savings,
            crux_tokens_saved,
            has_message_snapshot,
            &created_at,
            &price_context,
        ) {
            Ok(dollars_usd) => dollars_usd,
            Err(error) => {
                eprintln!("failed to persist agent session turn: {error}");
                0.0
            }
        }
    }

    fn read_terminal_savings_snapshot(&self) -> TerminalSavingsSnapshot {
        let proxy = (self.snapshot().mode != SessionMode::Gui)
            .then(|| {
                self.savings_meter.as_ref().and_then(|meter| {
                    match tauri::async_runtime::block_on(
                        meter.sidecar.project_savings_snapshot(&meter.project),
                    ) {
                        Ok(snapshot) => Some(snapshot),
                        Err(error) => {
                            eprintln!("failed to poll terminal savings: {error}");
                            None
                        }
                    }
                })
            })
            .flatten();
        TerminalSavingsSnapshot {
            proxy,
            rtk: self
                .rtk_session_db
                .as_deref()
                .and_then(exec::rtk_tokens_saved_at),
            crux: self
                .crux_session_ledger
                .as_deref()
                .and_then(terminal_crux_tokens_saved_at),
        }
    }

    fn take_session_source_delta(&self) -> (u64, u64) {
        let Some(state) = self.terminal_savings.as_ref() else {
            return (0, 0);
        };
        let snapshot = self.read_terminal_savings_snapshot();
        let mut state = state.lock().expect("terminal savings state lock poisoned");
        state
            .apply(TerminalSavingsSnapshot {
                proxy: None,
                ..snapshot
            })
            .into_iter()
            .fold((0u64, 0u64), |(tokens, crux), sample| {
                (
                    tokens.saturating_add(sample.savings_tokens),
                    crux.saturating_add(sample.crux_tokens_saved),
                )
            })
    }

    fn sample_terminal_savings(&self, app: &AppHandle, finalize: bool) {
        let Some(state) = self.terminal_savings.as_ref() else {
            return;
        };
        sample_terminal_savings_once(
            state,
            &self.finished,
            finalize,
            || self.read_terminal_savings_snapshot(),
            |sample| {
                self.record_savings_sample_with_cache(
                    app,
                    sample.from_proxy,
                    sample.savings_tokens,
                    sample.crux_tokens_saved,
                    sample.cache_read_tokens,
                    sample.turn_boundary,
                );
            },
        );
    }

    /// Provider cache reads ride on the sample turn, so the meter shows the
    /// session's context reuse next to its savings.
    fn record_savings_sample_with_cache(
        &self,
        app: &AppHandle,
        from_proxy: bool,
        savings_tokens: u64,
        crux_tokens_saved: u64,
        cache_read_tokens: u64,
        turn_boundary: bool,
    ) {
        let usage = TurnUsage {
            cache_read_tokens,
            ..TurnUsage::default()
        };
        let dollars_usd = self.persist_turn(
            app,
            &usage,
            Some(MeasuredSavings {
                tokens_saved: savings_tokens,
                savings_usd: None,
                once_tokens: if from_proxy { 0 } else { savings_tokens },
            }),
            crux_tokens_saved,
            false,
        );
        self.emit(
            app,
            AgentEvent::SavingsSample {
                savings_tokens,
                savings_source: "measured".to_string(),
                cache_read_tokens,
                turn_boundary,
                dollars_usd,
            },
        );
    }

    async fn prepare_savings_turn(&self) {
        let snapshot = self.snapshot();
        self.adapter.prepare_usage(
            &mut self
                .adapter_state
                .lock()
                .expect("agent adapter state lock poisoned"),
            snapshot.cli_session_id.as_deref(),
        );
        if let Some(codex_home) = &self.nested_codex_home {
            *self
                .nested_codex_usage_before
                .lock()
                .expect("nested Codex usage lock poisoned") = Some(codex::nested_usage(codex_home));
        }

        let Some(meter) = &self.savings_meter else {
            return;
        };
        let before = match meter.sidecar.project_savings_totals(&meter.project).await {
            Ok(totals) => Some(totals),
            Err(error) => {
                eprintln!("failed to snapshot agent turn savings: {error}");
                None
            }
        };
        *meter
            .before
            .lock()
            .expect("agent session savings lock poisoned") = before;
    }

    fn measure_savings_after_turn(&self) -> (Option<MeasuredSavings>, u64) {
        let headroom = self.measure_headroom_savings_after_turn();
        let (source_tokens, crux_tokens_saved) = self.take_session_source_delta();
        if source_tokens == 0 {
            return (headroom, crux_tokens_saved);
        }
        let tokens_saved = headroom
            .map(|savings| savings.tokens_saved)
            .unwrap_or_default()
            .saturating_add(source_tokens);
        // Mixed sources use the model table for the combined record. This
        // prices RTK and crux savings without treating them as Headroom USD.
        (
            Some(MeasuredSavings {
                tokens_saved,
                savings_usd: None,
                once_tokens: source_tokens,
            }),
            crux_tokens_saved,
        )
    }

    fn measure_headroom_savings_after_turn(&self) -> Option<MeasuredSavings> {
        let meter = self.savings_meter.as_ref()?;
        let before = meter
            .before
            .lock()
            .expect("agent session savings lock poisoned")
            .take()?;
        let started_at = Instant::now();
        let mut tracker = AgentSavingsSettleTracker::default();
        for attempt in 0..SAVINGS_SETTLE_ATTEMPTS {
            match tauri::async_runtime::block_on(
                meter.sidecar.project_savings_totals(&meter.project),
            ) {
                Ok(totals) => {
                    if tracker.observe(before, totals, started_at.elapsed()) {
                        return Some(MeasuredSavings {
                            tokens_saved: totals.tokens_saved.saturating_sub(before.tokens_saved),
                            savings_usd: match (before.savings_usd, totals.savings_usd) {
                                (Some(before), Some(after)) => Some((after - before).max(0.0)),
                                _ => None,
                            },
                            once_tokens: 0,
                        });
                    }
                }
                Err(error) if attempt + 1 == SAVINGS_SETTLE_ATTEMPTS => {
                    eprintln!("failed to settle agent turn savings: {error}");
                }
                Err(_) => {}
            }
            if attempt + 1 < SAVINGS_SETTLE_ATTEMPTS {
                thread::sleep(SAVINGS_SETTLE_DELAY);
            }
        }
        None
    }

    fn measure_claude_usage_after_turn(&self) -> Option<TurnUsage> {
        let snapshot = self.snapshot();
        self.adapter.measured_usage(
            &mut self
                .adapter_state
                .lock()
                .expect("agent adapter state lock poisoned"),
            snapshot.cli_session_id.as_deref(),
        )
    }

    fn apply_measured_usage(
        &self,
        usage: &mut TurnUsage,
        claude_usage: Option<TurnUsage>,
        nested_codex_usage: Option<TurnUsage>,
    ) {
        if let Some(claude_usage) = claude_usage {
            replace_usage_tokens(usage, &claude_usage);
        }
        if let Some(nested_codex_usage) = nested_codex_usage {
            add_usage_tokens(usage, &nested_codex_usage);
        }
    }

    fn measure_nested_codex_usage_after_turn(&self) -> Option<TurnUsage> {
        let codex_home = self.nested_codex_home.as_deref()?;
        let before = self
            .nested_codex_usage_before
            .lock()
            .expect("nested Codex usage lock poisoned")
            .take()?;
        Some(turn_usage_delta(codex::nested_usage(codex_home), before))
    }

    async fn send_user_message(
        self: &Arc<Self>,
        app: &AppHandle,
        text: &str,
    ) -> AgentSessionResult<()> {
        let snapshot = self.snapshot();
        if !matches!(
            snapshot.status.as_str(),
            "starting" | "running" | "waiting_input"
        ) {
            return Err(format!("agent session is {}", snapshot.status));
        }
        if self.adapter.is_per_turn() {
            return self.send_per_turn_user_message(app, text).await;
        }

        let line = {
            let mut state = self
                .adapter_state
                .lock()
                .expect("agent adapter state lock poisoned");
            self.adapter.encode_user_message(
                &mut state,
                text,
                TurnKind::Start {
                    cli_session_id: snapshot.cli_session_id.as_deref(),
                },
            )?
        };
        let Some(line) = line else {
            return Ok(());
        };
        self.prepare_savings_turn().await;
        self.write_protocol_line(&line)?;

        self.set_status("running");
        self.persist(app);
        if self.adapter.emits_turn_started() {
            self.emit(app, AgentEvent::TurnStarted { turn_id: None });
        }
        Ok(())
    }

    fn steer_conversation_message(
        &self,
        conversation_id: &str,
        text: &str,
    ) -> AgentSessionResult<()> {
        let active_conversation_id = self
            .conversation_turn
            .lock()
            .expect("conversation turn lock poisoned")
            .as_ref()
            .map(|turn| turn.conversation_id.clone());
        if active_conversation_id.as_deref() != Some(conversation_id) {
            return Err("the conversation has no active steerable turn".to_string());
        }

        let snapshot = self.snapshot();
        let cli_session_id = snapshot
            .cli_session_id
            .as_deref()
            .ok_or_else(|| "the agent adapter has not initialized its session yet".to_string())?;
        let mut state = self
            .adapter_state
            .lock()
            .expect("agent adapter state lock poisoned");
        let active_turn_id = state.active_turn_id().map(str::to_string);
        let turn = TurnKind::Steer {
            cli_session_id,
            turn_id: active_turn_id.as_deref(),
        };
        let line = self
            .adapter
            .encode_user_message(&mut state, text, turn)?
            .ok_or_else(|| "the agent adapter did not encode the steer message".to_string())?;
        drop(state);
        self.write_protocol_line(&line)
    }

    async fn send_per_turn_user_message(
        self: &Arc<Self>,
        app: &AppHandle,
        text: &str,
    ) -> AgentSessionResult<()> {
        {
            let mut process = self
                .process
                .lock()
                .expect("agent session process lock poisoned");
            if let Some(child) = process.as_mut() {
                match child.try_wait() {
                    Ok(Some(_)) => {
                        process.take();
                    }
                    Ok(None) | Err(_) => {
                        return Err("the Kimi session already has an active turn".to_string());
                    }
                }
            }
        }

        self.prepare_savings_turn().await;
        let resume_cli_session_id = self.snapshot().cli_session_id;
        let mut adapter_process = self.adapter.start_turn(
            &mut self
                .adapter_state
                .lock()
                .expect("agent adapter state lock poisoned"),
            text,
            resume_cli_session_id.as_deref(),
        )?;
        let mut child = adapter_process
            .child
            .take()
            .expect("per-turn adapter should return a child process");
        let stdout = adapter_process
            .stdout
            .take()
            .expect("per-turn adapter should return stdout");
        let stderr = adapter_process.stderr.take();
        let pid = adapter_process.pid;
        {
            let mut process = self
                .process
                .lock()
                .expect("agent session process lock poisoned");
            if process.is_some() {
                stop_unmanaged_child(&mut child);
                return Err("the Kimi session already has an active turn".to_string());
            }
            *process = Some(child);
        }
        {
            let mut record = self
                .record
                .lock()
                .expect("agent session record lock poisoned");
            record.pid = pid;
            record.status = "running".to_string();
            record.last_active_at = Utc::now().to_rfc3339();
        }
        self.persist(app);
        self.emit(app, AgentEvent::TurnStarted { turn_id: None });
        // Each turn starts a new child. A reset keeps the tail scoped to
        // this turn's diagnostics.
        *self
            .stderr_output
            .lock()
            .expect("agent stderr lock poisoned") = StderrTail::new(AGENT_STDERR_TAIL_BYTES);
        let stderr_reader =
            stderr.map(|stderr| spawn_stderr_reader(self.stderr_output.clone(), stderr));
        spawn_stdout_reader(app.clone(), self.clone(), stdout);
        spawn_kimi_process_waiter(app.clone(), self.clone(), stderr_reader);
        Ok(())
    }

    fn write_protocol_line(&self, line: &str) -> AgentSessionResult<()> {
        // The clone releases the lock before the enqueue can wait.
        let writer = self
            .writer
            .lock()
            .expect("agent session writer lock poisoned")
            .clone()
            .ok_or_else(|| "agent session input is closed".to_string())?;
        writer.enqueue_line(line)
    }

    fn register_pending_permission(
        &self,
        app: &AppHandle,
        request: adapter::PendingPermission,
        event: AgentEvent,
    ) {
        let mut state = self
            .adapter_state
            .lock()
            .expect("agent adapter state lock poisoned");
        if self.finishing.load(Ordering::SeqCst) {
            return;
        }
        state.register_pending_permission(request);
        drop(state);
        self.handle_event(app, event);
    }

    fn respond_permission(
        &self,
        app: &AppHandle,
        request_id: &str,
        approve: bool,
    ) -> AgentSessionResult<()> {
        let mut state = self
            .adapter_state
            .lock()
            .expect("agent adapter state lock poisoned");
        let response = state.permission_response(request_id, approve)?;
        self.write_protocol_line(&response)?;
        state.remove_pending_permission(request_id);
        drop(state);
        self.handle_event(
            app,
            AgentEvent::PermissionResolved {
                request_id: request_id.to_string(),
                approved: approve,
            },
        );
        Ok(())
    }

    fn resolve_pending_permissions(&self, app: &AppHandle) {
        let events = {
            let mut state = self
                .adapter_state
                .lock()
                .expect("agent adapter state lock poisoned");
            state.drain_pending_permission_events()
        };
        for event in events {
            self.handle_event(app, event);
        }
    }

    fn kill(&self, app: &AppHandle) -> AgentSessionResult<AgentSession> {
        let snapshot = self.snapshot();
        if !matches!(
            snapshot.status.as_str(),
            "starting" | "running" | "waiting_input"
        ) {
            return Ok(snapshot);
        }

        self.closing.store(true, Ordering::SeqCst);
        self.writer
            .lock()
            .expect("agent session writer lock poisoned")
            .take();
        let had_process = {
            let mut process = self
                .process
                .lock()
                .expect("agent session process lock poisoned");
            match process.as_mut() {
                Some(child) => {
                    child
                        .kill()
                        .map_err(|error| format!("failed to kill agent session: {error}"))?;
                    true
                }
                None => false,
            }
        };
        if !had_process {
            self.finish(app, Some(0), false);
        } else if !self.process_waiter_spawned.load(Ordering::SeqCst) {
            // No waiter thread exists. Reap the child here so it cannot
            // stay a zombie, then finish the session.
            stop_managed_child(self);
            self.finish(app, None, false);
        }
        Ok(self.snapshot())
    }

    fn finish(&self, app: &AppHandle, exit_code: Option<i32>, failed_to_wait: bool) {
        if self.finishing.swap(true, Ordering::SeqCst) {
            return;
        }
        self.resolve_pending_permissions(app);
        if self.terminal_savings.is_some() {
            // The final read shares the poller's state lock. The read also
            // marks the session finished before the proxy can stop.
            self.sample_terminal_savings(app, true);
        } else {
            self.finished.store(true, Ordering::SeqCst);
        }
        self.adapter_state
            .lock()
            .expect("agent adapter state lock poisoned")
            .mark_init_failed();
        self.writer
            .lock()
            .expect("agent session writer lock poisoned")
            .take();
        self.process
            .lock()
            .expect("agent session process lock poisoned")
            .take();

        let was_killed = self.closing.load(Ordering::SeqCst);
        {
            let mut record = self
                .record
                .lock()
                .expect("agent session record lock poisoned");
            record.status = if failed_to_wait || (!was_killed && exit_code != Some(0)) {
                "failed".to_string()
            } else {
                "exited".to_string()
            };
            record.last_active_at = Utc::now().to_rfc3339();
            record.exit_code = exit_code;
            record.generation = next_session_generation(record.generation);
        }
        self.active_sessions.fetch_sub(1, Ordering::SeqCst);
        if let Some(meter) = &self.savings_meter {
            if meter.shutdown_on_exit {
                meter.sidecar.shutdown();
            }
        }
        // The session directory holds the nested Codex home and the agent
        // config files. A rehydrated session has no home path, so the
        // removal uses the session ID.
        session_files::remove(&self.snapshot().id);
        self.refresh_savings_sources(app);
        self.persist(app);
        self.emit(app, AgentEvent::SessionExited { code: exit_code });
    }
}

#[derive(Clone, Default)]
pub struct AgentSessionManager {
    sessions: Arc<Mutex<HashMap<String, Arc<ManagedAgentSession>>>>,
    active_sessions: Arc<AtomicUsize>,
    #[cfg(test)]
    in_memory_sessions: Arc<Mutex<HashMap<String, InMemoryAdapterSession>>>,
}

#[cfg(test)]
struct InMemoryAdapterSession {
    adapter: &'static dyn AgentAdapter,
    state: AdapterState,
    events: Vec<AgentEvent>,
    responses: Vec<String>,
    settled_usage: Option<TurnUsage>,
}

fn remove_stopped_agent_session<T>(
    sessions: &mut HashMap<String, T>,
    id: &str,
    status: impl FnOnce(&T) -> String,
) -> AgentSessionResult<()> {
    let session = sessions
        .get(id)
        .ok_or_else(|| format!("agent session `{id}` was not found"))?;
    if status(session) == "running" {
        return Err(format!("agent session `{id}` is running"));
    }
    sessions.remove(id);
    Ok(())
}

/// Rebuilds a restored session. `legacy_claude_subscription_proxy` is the
/// stored subscription opt-in. Only a row from 1.0.0 or older reads it.
fn rehydrated_agent_session_record(
    row: &AgentSessionRow,
    terminal: &TerminalSession,
    legacy_claude_subscription_proxy: bool,
    savings_paths: &SessionSavingsPaths,
) -> AgentSessionResult<(AgentSession, SessionLane, HeadroomRoute)> {
    if row.id != terminal.id {
        return Err(format!(
            "failed to rehydrate agent session `{}`: terminal id `{}` does not match",
            row.id, terminal.id
        ));
    }
    if row.mode != SessionMode::TerminalEmbedded.storage_name() {
        return Err(format!(
            "failed to rehydrate agent session `{}`: persisted mode `{}` is not terminal_embedded",
            row.id, row.mode
        ));
    }
    let agent_kind = AgentKind::from_storage_name(&row.agent_kind)?;
    let lane_kind = SessionLaneKind::from_storage_name(&row.lane)?;
    let ponytail_mode = row
        .ponytail_mode
        .as_deref()
        .map(str::parse::<PonytailMode>)
        .transpose()
        .map_err(|error| {
            format!(
                "failed to decode Ponytail mode for agent session `{}`: {error}",
                row.id
            )
        })?;
    let ai_vault_title = row
        .ai_vault_session_id
        .as_ref()
        .zip(row.ai_vault_title.as_ref())
        .map(|(session_id, title)| AiVaultSessionTitle {
            session_id: session_id.clone(),
            title: title.clone(),
        });
    let lane = lane_kind.rehydrated_lane();
    let route = rehydrated_headroom_route(
        row.headroom_route.as_deref(),
        agent_kind,
        lane_kind,
        legacy_claude_subscription_proxy,
    );
    // A restored session keeps the proxy source of its launch, whatever the
    // engine preference is now.
    let proxy_source = route.proxy_source();
    let rtk_instruction = adapter_for(agent_kind).uses_rtk_instruction();
    Ok((
        AgentSession {
            id: row.id.clone(),
            default_title: row.default_title.clone(),
            generated_title: row.generated_title.clone(),
            ai_vault_title,
            custom_title: row.custom_title.clone(),
            generation: row.generation,
            agent_kind,
            lane_kind,
            mode: SessionMode::TerminalEmbedded,
            ponytail_mode,
            project_id: row.project_id,
            conversation_id: row.conversation_id.clone(),
            conversation_title: None,
            model: None,
            cwd: row.cwd.clone(),
            pid: terminal.pid,
            status: "running".to_string(),
            cli_session_id: row.cli_session_id.clone(),
            created_at: row.created_at.clone(),
            last_active_at: row.last_active_at.clone(),
            exit_code: row.exit_code,
            savings_sources: SavingsSources::from_paths(
                &savings_paths.rtk,
                &savings_paths.crux,
                proxy_source,
                rtk_instruction,
            ),
        },
        lane,
        route,
    ))
}

/// The Halv Engine route of a restored session. A row from 1.0.1 or newer
/// holds the route of its launch. A row from 1.0.0 or older has none, so the
/// legacy rule reads it from the lane.
fn rehydrated_headroom_route(
    stored: Option<&str>,
    agent_kind: AgentKind,
    lane_kind: SessionLaneKind,
    legacy_claude_subscription_proxy: bool,
) -> HeadroomRoute {
    stored
        .and_then(HeadroomRoute::from_storage_name)
        .unwrap_or_else(|| {
            HeadroomRoute::legacy(agent_kind, lane_kind, legacy_claude_subscription_proxy)
        })
}

/// Marks the agent rows without a running terminal `exited`. A supervisor
/// with another protocol can still run these agents, so their rows stay live
/// and no pane resumes a second copy. See ADR 0008.
fn reconcile_supervised_rows(
    storage: &Storage,
    running_terminal_ids: &HashSet<String>,
    supervisor_protocol_mismatch: bool,
) -> AgentSessionResult<()> {
    if supervisor_protocol_mismatch {
        return Ok(());
    }
    storage
        .reconcile_session_routes(running_terminal_ids)
        .map_err(|error| error.to_string())
}

fn build_rehydrated_managed_session(
    record: AgentSession,
    lane: SessionLane,
    savings_paths: SessionSavingsPaths,
    active_sessions: Arc<AtomicUsize>,
    sidecar: Option<SidecarManager>,
) -> Arc<ManagedAgentSession> {
    let id = record.id.clone();
    let adapter = adapter_for(record.agent_kind);
    let adapter_state = adapter.new_state(
        record.cli_session_id.is_some(),
        CanonicalMode::Ask,
        Instant::now(),
        false,
    );
    let rtk_source_path = savings_paths.rtk;
    let crux_source_path = savings_paths.crux;
    let proxy_source = record.savings_sources.proxy;
    let rtk_instruction = adapter_for(record.agent_kind).uses_rtk_instruction();
    let usage_kind = record.agent_kind.storage_name();
    let savings_meter = sidecar
        .filter(|_| proxy_source)
        .map(|sidecar| SessionSavingsMeter {
            sidecar,
            project: id.clone(),
            before: Mutex::new(None),
            shutdown_on_exit: false,
        });
    Arc::new(ManagedAgentSession {
        record: Mutex::new(record),
        adapter,
        adapter_state: Mutex::new(adapter_state),
        lane,
        credentials_rehydrated: true,
        permission_mode: DEFAULT_PERMISSION_MODE.to_string(),
        writer: Mutex::new(None),
        process: Mutex::new(None),
        stderr_output: Arc::new(Mutex::new(StderrTail::new(AGENT_STDERR_TAIL_BYTES))),
        closing: AtomicBool::new(false),
        finishing: AtomicBool::new(false),
        finished: AtomicBool::new(false),
        process_waiter_spawned: AtomicBool::new(false),
        active_sessions,
        turn_index: AtomicUsize::new(0),
        savings_meter,
        terminal_savings: Some(Mutex::new(TerminalSavingsState::default())),
        rtk_session_db: Some(rtk_source_path.clone()),
        crux_session_ledger: Some(crux_source_path.clone()),
        rtk_source_path,
        crux_source_path,
        proxy_source,
        rtk_instruction,
        memory_generation_effort: chat::DEFAULT_EFFORT.to_string(),
        memories_injected: AtomicBool::new(false),
        conversation_turn: Mutex::new(None),
        nested_codex_usage_before: Mutex::new(None),
        nested_codex_home: None,
        usage_meter: Mutex::new(usage_meter::SessionUsageMeter::new(usage_kind)),
        pending_usage_window: Mutex::new(None),
    })
}

impl AgentSessionManager {
    #[cfg(test)]
    fn register_in_memory_adapter(
        &self,
        id: &str,
        adapter: &'static dyn AgentAdapter,
    ) -> AgentSessionResult<()> {
        let mut sessions = self
            .in_memory_sessions
            .lock()
            .expect("in-memory agent sessions lock poisoned");
        if sessions.contains_key(id) {
            return Err(format!("agent session `{id}` is already registered"));
        }
        sessions.insert(
            id.to_string(),
            InMemoryAdapterSession {
                adapter,
                state: adapter.new_state(false, CanonicalMode::Ask, Instant::now(), false),
                events: Vec::new(),
                responses: Vec::new(),
                settled_usage: None,
            },
        );
        Ok(())
    }

    #[cfg(test)]
    fn drive_in_memory_adapter_line(&self, id: &str, line: &str) -> AgentSessionResult<()> {
        let mut sessions = self
            .in_memory_sessions
            .lock()
            .expect("in-memory agent sessions lock poisoned");
        let session = sessions
            .get_mut(id)
            .ok_or_else(|| format!("agent session `{id}` was not found"))?;
        let parsed = session.adapter.parse_line(&mut session.state, line);
        let mut pending = Vec::new();
        let mut events = Vec::new();
        let mut responses = Vec::new();
        dispatch_parsed_turn(
            parsed,
            |request, event| pending.push((request, event)),
            |event| events.push(event),
            |response| responses.push(response),
        );
        for (request, event) in pending {
            session.state.register_pending_permission(request);
            events.push(event);
        }
        for event in &events {
            if let AgentEvent::TurnCompleted { usage } = event {
                session.settled_usage = Some(usage.clone());
            }
        }
        session.events.extend(events);
        session.responses.extend(responses);
        Ok(())
    }

    #[cfg(test)]
    fn respond_in_memory_permission(
        &self,
        id: &str,
        request_id: &str,
        approve: bool,
    ) -> AgentSessionResult<()> {
        let mut sessions = self
            .in_memory_sessions
            .lock()
            .expect("in-memory agent sessions lock poisoned");
        let session = sessions
            .get_mut(id)
            .ok_or_else(|| format!("agent session `{id}` was not found"))?;
        let response = session.state.permission_response(request_id, approve)?;
        session.state.remove_pending_permission(request_id);
        session.responses.push(response);
        session.events.push(AgentEvent::PermissionResolved {
            request_id: request_id.to_string(),
            approved: approve,
        });
        Ok(())
    }

    fn next_default_title(&self, agent_kind: AgentKind) -> String {
        let number = self
            .sessions
            .lock()
            .expect("agent sessions lock poisoned")
            .values()
            .filter(|session| session.snapshot().agent_kind == agent_kind)
            .count()
            .saturating_add(1);
        let label = match agent_kind {
            AgentKind::Antigravity => "Antigravity",
            AgentKind::Claude => "Claude",
            AgentKind::Codex => "Codex",
            AgentKind::Gemini => "Gemini",
            AgentKind::Grok => "Grok",
            AgentKind::Kimi => "Kimi",
            AgentKind::Zai => "GLM",
        };
        format!("{label} {number}")
    }

    fn get(&self, id: &str) -> AgentSessionResult<Arc<ManagedAgentSession>> {
        self.sessions
            .lock()
            .expect("agent sessions lock poisoned")
            .get(id)
            .cloned()
            .ok_or_else(|| format!("agent session `{id}` was not found"))
    }

    pub fn list(&self) -> Vec<AgentSession> {
        let ids = self
            .sessions
            .lock()
            .expect("agent sessions lock poisoned")
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        let mut snapshots = ids
            .iter()
            .filter_map(|id| self.snapshot(id).ok())
            .collect::<Vec<_>>();
        snapshots.sort_by(|left, right| right.created_at.cmp(&left.created_at));
        snapshots
    }

    /// Adopts the agent sessions that survived the last app run. When a
    /// restored session needs the shared Halv Engine, the engine runs for
    /// this app run (`SidecarManager::keep_for_restored_sessions`), also
    /// while the preference is off. `legacy_claude_subscription_proxy` is the
    /// stored subscription opt-in for rows from 1.0.0 or older.
    pub(crate) fn rehydrate_supervised(
        &self,
        app: &AppHandle,
        storage: &Storage,
        terminals: &TerminalManager,
        legacy_claude_subscription_proxy: bool,
    ) -> AgentSessionResult<()> {
        let running_terminals = terminals
            .list()
            .into_iter()
            .filter(|terminal| terminal.status == "running")
            .collect::<Vec<_>>();
        let running_terminal_ids = running_terminals
            .iter()
            .map(|terminal| terminal.id.clone())
            .collect::<HashSet<_>>();
        let sidecar = app
            .try_state::<SidecarManager>()
            .map(|state| state.inner().clone());

        for terminal in running_terminals
            .iter()
            .filter(|terminal| terminal.agent_kind.is_some())
        {
            let row = match storage.get_agent_session(&terminal.id) {
                Ok(Some(row)) => row,
                Ok(None) => {
                    eprintln!(
                        "failed to rehydrate supervised agent terminal `{}`: agent session row was not found",
                        terminal.id
                    );
                    continue;
                }
                Err(error) => {
                    eprintln!(
                        "failed to load agent session `{}` for rehydration: {error}",
                        terminal.id
                    );
                    continue;
                }
            };
            let savings_paths = resolve_session_savings_paths(storage, &row);
            let (mut record, lane, route) = match rehydrated_agent_session_record(
                &row,
                terminal,
                legacy_claude_subscription_proxy,
                &savings_paths,
            ) {
                Ok(value) => value,
                Err(error) => {
                    eprintln!("{error}");
                    continue;
                }
            };
            // The pane holds the engine port of its launch. Without the
            // engine, its next request gets "connection refused".
            if route.needs_shared_engine() {
                if let Some(sidecar) = &sidecar {
                    sidecar.keep_for_restored_sessions();
                }
            }
            record.last_active_at = Utc::now().to_rfc3339();
            let managed = build_rehydrated_managed_session(
                record,
                lane,
                savings_paths,
                self.active_sessions.clone(),
                sidecar.clone(),
            );
            {
                let mut sessions = self.sessions.lock().expect("agent sessions lock poisoned");
                if sessions.contains_key(&terminal.id) {
                    continue;
                }
                sessions.insert(terminal.id.clone(), managed.clone());
            }
            self.active_sessions.fetch_add(1, Ordering::SeqCst);
            if let Err(error) = managed.persist_with_storage(storage) {
                self.sessions
                    .lock()
                    .expect("agent sessions lock poisoned")
                    .remove(&terminal.id);
                self.active_sessions.fetch_sub(1, Ordering::SeqCst);
                eprintln!(
                    "failed to persist rehydrated agent session `{}`: {error}",
                    terminal.id
                );
                continue;
            }
            spawn_interval_savings_poller(app.clone(), managed.clone());
            managed.emit(
                app,
                AgentEvent::SessionInit {
                    cli_session_id: managed.snapshot().cli_session_id.unwrap_or_default(),
                    model: String::new(),
                },
            );
            spawn_embedded_lifecycle(
                app.clone(),
                self.clone(),
                terminals.clone(),
                terminal.id.clone(),
            );
        }

        reconcile_supervised_rows(
            storage,
            &running_terminal_ids,
            terminals.supervisor_protocol_mismatch().is_some(),
        )
    }

    pub fn snapshot(&self, id: &str) -> AgentSessionResult<AgentSession> {
        Ok(self.get(id)?.snapshot())
    }

    pub fn forget(&self, id: &str) -> AgentSessionResult<()> {
        let mut sessions = self.sessions.lock().expect("agent sessions lock poisoned");
        remove_stopped_agent_session(&mut sessions, id, |session| session.snapshot().status)
    }

    pub fn rename(
        &self,
        storage: &Storage,
        id: &str,
        title: Option<String>,
    ) -> AgentSessionResult<AgentSession> {
        let managed = self.get(id)?;
        let title = normalize_title(title);
        let mut record = managed
            .record
            .lock()
            .expect("agent session record lock poisoned");
        let previous_title = std::mem::replace(&mut record.custom_title, title);
        if let Err(error) = persist_snapshot(storage, &record) {
            record.custom_title = previous_title;
            return Err(error);
        }
        Ok(record.clone())
    }

    fn apply_title_state(
        &self,
        storage: &Storage,
        id: &str,
        state: Option<AgentSessionTitleState>,
    ) -> AgentSessionResult<AgentSession> {
        let managed = self.get(id)?;
        let mut record = managed
            .record
            .lock()
            .expect("agent session record lock poisoned");
        if let Some(state) = state {
            if let Some(default_title) = normalize_title(Some(state.default_title)) {
                record.default_title = default_title;
            }
            record.generated_title = normalize_title(state.generated_title);
            record.ai_vault_title = state.ai_vault_title.and_then(normalize_ai_vault_title);
            record.custom_title = normalize_title(state.custom_title);
            record.generation = state.generation;
        }
        persist_snapshot(storage, &record)?;
        Ok(record.clone())
    }

    fn set_generated_title(
        &self,
        storage: &Storage,
        id: &str,
        prompt: &str,
    ) -> AgentSessionResult<()> {
        let Some(title) = generated_title_from_prompt(prompt) else {
            return Ok(());
        };
        let managed = self.get(id)?;
        let mut record = managed
            .record
            .lock()
            .expect("agent session record lock poisoned");
        if record.generated_title.is_some() {
            return Ok(());
        }
        record.generated_title = Some(title);
        persist_snapshot(storage, &record)
    }

    pub(crate) fn set_ai_vault_title_for_conversation(
        &self,
        storage: &Storage,
        conversation_id: &str,
        title: &str,
    ) -> AgentSessionResult<()> {
        let title = normalize_title(Some(title.to_string()))
            .ok_or_else(|| "agent session title cannot be empty".to_string())?;
        let candidate = self
            .sessions
            .lock()
            .expect("agent sessions lock poisoned")
            .values()
            .filter_map(|session| {
                let snapshot = session.snapshot();
                (snapshot.conversation_id.as_deref() == Some(conversation_id)
                    && snapshot.cli_session_id.is_some())
                .then_some(snapshot)
            })
            .max_by_key(|session| session.created_at.clone());
        let Some(candidate) = candidate else {
            return Ok(());
        };
        let Some(session_id) = candidate.cli_session_id else {
            return Ok(());
        };
        let managed = self.get(&candidate.id)?;
        let mut record = managed
            .record
            .lock()
            .expect("agent session record lock poisoned");
        record.ai_vault_title = Some(AiVaultSessionTitle { session_id, title });
        persist_snapshot(storage, &record)
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn send_conversation_message(
        &self,
        app: AppHandle,
        storage: &Storage,
        sidecar: &SidecarManager,
        agent_kind: AgentKind,
        lane: SessionLane,
        conversation_id: &str,
        conversation_title: &str,
        model: &str,
        effort: Option<&str>,
        cwd: &str,
        project_id: Option<i64>,
        permission_mode: &str,
        ponytail_mode: PonytailMode,
        text: &str,
    ) -> AgentSessionResult<ConversationAgentTurnResult> {
        validate_conversation_agent(agent_kind, &lane, model)?;
        let permission_mode = canonical_permission_mode(permission_mode)?.to_string();
        let lane = resolve_lane_credentials(storage, lane)?;
        let lane_kind = lane.kind();

        let candidates = self
            .sessions
            .lock()
            .expect("agent sessions lock poisoned")
            .values()
            .filter(|session| {
                let snapshot = session.snapshot();
                snapshot.mode == SessionMode::Gui
                    && snapshot.conversation_id.as_deref() == Some(conversation_id)
            })
            .cloned()
            .collect::<Vec<_>>();

        for session in &candidates {
            let snapshot = session.snapshot();
            if (snapshot.agent_kind != agent_kind
                || snapshot.lane_kind != lane_kind
                || session.lane != lane
                || session.permission_mode != permission_mode
                || snapshot.ponytail_mode != Some(ponytail_mode)
                || snapshot.model.as_deref() != Some(model))
                && matches!(
                    snapshot.status.as_str(),
                    "starting" | "running" | "waiting_input"
                )
            {
                let _ = session.kill(&app);
            }
        }

        let matching = candidates
            .into_iter()
            .filter(|session| {
                let snapshot = session.snapshot();
                snapshot.agent_kind == agent_kind
                    && snapshot.lane_kind == lane_kind
                    && session.lane == lane
                    && session.permission_mode == permission_mode
                    && snapshot.ponytail_mode == Some(ponytail_mode)
                    && snapshot.model.as_deref() == Some(model)
            })
            .max_by_key(|session| session.snapshot().created_at);
        let managed = if let Some(session) = matching
            .as_ref()
            .filter(|session| {
                matches!(
                    session.snapshot().status.as_str(),
                    "starting" | "running" | "waiting_input"
                )
            })
            .cloned()
        {
            session
        } else {
            let resume_cli_session_id = matching
                .as_ref()
                .and_then(|session| session.snapshot().cli_session_id);
            let session_proxy = session_proxy_context(agent_kind, &lane, sidecar).await?;
            let session = self
                .spawn_internal(
                    app.clone(),
                    storage,
                    agent_kind,
                    lane,
                    cwd.to_string(),
                    project_id,
                    Some(permission_mode.clone()),
                    ponytail_mode,
                    session_proxy,
                    resume_cli_session_id,
                    Some(model.to_string()),
                    effort.map(str::to_string),
                )
                .map_err(|error| actionable_agent_error(agent_kind, error))?;
            let managed = self.get(&session.id)?;
            managed.tag_conversation(conversation_id, conversation_title, model);
            managed.persist(&app);
            managed
        };

        self.set_generated_title(storage, &managed.snapshot().id, text)?;
        managed.tag_conversation(conversation_id, conversation_title, model);
        let completion = managed.begin_conversation_turn(conversation_id, model)?;
        let cli_text = prefix_memories_for_first_turn(&managed.memories_injected, text, || {
            chat::memory_instructions_for_conversation(&app, storage, conversation_id)
        });
        if let Err(error) = managed.send_user_message(&app, &cli_text).await {
            let message = actionable_agent_error(agent_kind, error);
            managed.cancel_conversation_turn(&message);
            return Err(message);
        }
        completion
            .await
            .map_err(|_| format!("{} turn ended without a result", agent_kind.display_name()))?
    }

    pub(crate) fn steer_conversation_message(
        &self,
        conversation_id: &str,
        text: &str,
    ) -> AgentSessionResult<()> {
        let session = self
            .sessions
            .lock()
            .expect("agent sessions lock poisoned")
            .values()
            .filter(|session| {
                let snapshot = session.snapshot();
                snapshot.mode == SessionMode::Gui
                    && snapshot.conversation_id.as_deref() == Some(conversation_id)
                    && session.has_active_conversation_turn(conversation_id)
            })
            .max_by_key(|session| session.snapshot().created_at)
            .cloned()
            .ok_or_else(|| "the conversation has no active steerable turn".to_string())?;
        session.steer_conversation_message(conversation_id, text)
    }

    #[allow(clippy::too_many_arguments)]
    fn spawn(
        &self,
        app: AppHandle,
        storage: &Storage,
        agent_kind: AgentKind,
        lane: SessionLane,
        cwd: String,
        project_id: Option<i64>,
        permission_mode: Option<String>,
        ponytail_mode: PonytailMode,
        session_proxy: Option<SessionProxyContext>,
        model: Option<String>,
        effort: Option<String>,
    ) -> AgentSessionResult<AgentSession> {
        self.spawn_internal(
            app,
            storage,
            agent_kind,
            lane,
            cwd,
            project_id,
            permission_mode,
            ponytail_mode,
            session_proxy,
            None,
            model,
            effort,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn resume(
        &self,
        app: AppHandle,
        storage: &Storage,
        agent_kind: AgentKind,
        lane: SessionLane,
        cwd: String,
        project_id: Option<i64>,
        permission_mode: Option<String>,
        ponytail_mode: PonytailMode,
        session_proxy: Option<SessionProxyContext>,
        cli_session_id: String,
        model: Option<String>,
        effort: Option<String>,
    ) -> AgentSessionResult<AgentSession> {
        if cli_session_id.trim().is_empty() {
            return Err("the agent resume session ID cannot be empty".to_string());
        }
        self.spawn_internal(
            app,
            storage,
            agent_kind,
            lane,
            cwd,
            project_id,
            permission_mode,
            ponytail_mode,
            session_proxy,
            Some(cli_session_id),
            model,
            effort,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn spawn_internal(
        &self,
        app: AppHandle,
        storage: &Storage,
        agent_kind: AgentKind,
        lane: SessionLane,
        cwd: String,
        project_id: Option<i64>,
        permission_mode: Option<String>,
        ponytail_mode: PonytailMode,
        session_proxy: Option<SessionProxyContext>,
        resume_cli_session_id: Option<String>,
        model: Option<String>,
        effort: Option<String>,
    ) -> AgentSessionResult<AgentSession> {
        reserve_session_slot(&self.active_sessions)?;
        let session_id = new_uuid_v4();
        let mut session_proxy = session_proxy;
        if let Err(error) = prepare_nested_codex_home(agent_kind, &mut session_proxy, &session_id) {
            self.active_sessions.fetch_sub(1, Ordering::SeqCst);
            shutdown_owned_proxy(&session_proxy);
            return Err(error);
        }
        let result = self.spawn_reserved(
            app,
            storage,
            agent_kind,
            lane,
            cwd,
            project_id,
            permission_mode,
            ponytail_mode,
            session_proxy.clone(),
            resume_cli_session_id,
            model,
            effort,
            session_id,
        );
        if result.is_err() {
            self.active_sessions.fetch_sub(1, Ordering::SeqCst);
            shutdown_owned_proxy(&session_proxy);
        }
        result
    }

    #[allow(clippy::too_many_arguments)]
    fn spawn_reserved(
        &self,
        app: AppHandle,
        storage: &Storage,
        agent_kind: AgentKind,
        lane: SessionLane,
        cwd: String,
        project_id: Option<i64>,
        permission_mode: Option<String>,
        ponytail_mode: PonytailMode,
        session_proxy: Option<SessionProxyContext>,
        resume_cli_session_id: Option<String>,
        model: Option<String>,
        effort: Option<String>,
        session_id: String,
    ) -> AgentSessionResult<AgentSession> {
        let cwd_path = PathBuf::from(&cwd);
        if !cwd_path.is_dir() {
            return Err(format!("agent session cwd `{cwd}` is not a directory"));
        }
        let permission_mode = permission_mode
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or(DEFAULT_PERMISSION_MODE);
        let permission_mode = canonical_permission_mode(permission_mode)?;
        let spawn_started_at = Instant::now();
        let extension_manager = app.state::<extensions::ExtensionManager>().inner().clone();
        let adapter = adapter_for(agent_kind);
        let mut adapter_state = adapter.new_state(
            resume_cli_session_id.is_some(),
            permission_mode,
            spawn_started_at,
            true,
        );
        let adapter_process = adapter.start(
            &LaunchContext {
                cwd: &cwd_path,
                lane: &lane,
                permission_mode,
                resume_cli_session_id: resume_cli_session_id.as_deref(),
                proxy_port: session_proxy.as_ref().and_then(|proxy| proxy.port),
                project: &session_id,
                savings_database_path: storage.database_path(),
                model: model.as_deref(),
                effort: effort.as_deref(),
                nested_codex_home: session_proxy
                    .as_ref()
                    .and_then(|proxy| proxy.nested_codex_home.as_deref()),
                ponytail_mode,
                extension_manager: &extension_manager,
            },
            &mut adapter_state,
        )?;
        let AdapterProcess {
            child,
            stdin,
            stdout,
            stderr,
            pid,
        } = adapter_process;

        let now = Utc::now().to_rfc3339();
        let lane_kind = lane.kind();
        let rtk_source_path = exec::rtk_session_database_path(&session_id);
        let crux_source_path = exec::crux_session_ledger_path(&session_id);
        let proxy_source = session_proxy
            .as_ref()
            .is_some_and(SessionProxyContext::routed)
            && lane.uses_proxy(agent_kind, false);
        let rtk_instruction = adapter.uses_rtk_instruction();
        let session = AgentSession {
            id: session_id.clone(),
            agent_kind,
            lane_kind,
            mode: SessionMode::Gui,
            ponytail_mode: Some(ponytail_mode),
            project_id,
            conversation_id: None,
            conversation_title: None,
            default_title: self.next_default_title(agent_kind),
            generated_title: None,
            ai_vault_title: None,
            custom_title: None,
            generation: 0,
            model: model.clone(),
            cwd,
            pid,
            status: "starting".to_string(),
            cli_session_id: resume_cli_session_id,
            created_at: now.clone(),
            last_active_at: now,
            exit_code: None,
            savings_sources: SavingsSources::from_paths(
                &rtk_source_path,
                &crux_source_path,
                proxy_source,
                rtk_instruction,
            ),
        };
        let (savings_meter, nested_codex_home) = match session_proxy {
            Some(proxy) => (proxy.savings_meter(&session_id), proxy.nested_codex_home),
            None => (None, None),
        };
        let managed = Arc::new(ManagedAgentSession {
            record: Mutex::new(session),
            adapter,
            adapter_state: Mutex::new(adapter_state),
            lane,
            credentials_rehydrated: false,
            permission_mode: permission_mode.to_string(),
            writer: Mutex::new(stdin.map(ProtocolWriter::spawn)),
            process: Mutex::new(child),
            stderr_output: Arc::new(Mutex::new(StderrTail::new(AGENT_STDERR_TAIL_BYTES))),
            closing: AtomicBool::new(false),
            finishing: AtomicBool::new(false),
            finished: AtomicBool::new(false),
            process_waiter_spawned: AtomicBool::new(false),
            active_sessions: self.active_sessions.clone(),
            turn_index: AtomicUsize::new(0),
            savings_meter,
            terminal_savings: Some(Mutex::new(TerminalSavingsState::default())),
            rtk_session_db: Some(rtk_source_path.clone()),
            crux_session_ledger: Some(crux_source_path.clone()),
            rtk_source_path,
            crux_source_path,
            proxy_source,
            rtk_instruction,
            memory_generation_effort: effort.unwrap_or_else(|| chat::DEFAULT_EFFORT.to_string()),
            memories_injected: AtomicBool::new(false),
            conversation_turn: Mutex::new(None),
            nested_codex_usage_before: Mutex::new(None),
            nested_codex_home,
            usage_meter: Mutex::new(usage_meter::SessionUsageMeter::new(
                agent_kind.storage_name(),
            )),
            pending_usage_window: Mutex::new(None),
        });

        if let Err(error) = managed.persist_with_storage_and_savings_paths(storage) {
            stop_managed_child(&managed);
            return Err(error);
        }
        managed.set_status(if adapter.is_per_turn() {
            "waiting_input"
        } else {
            "running"
        });
        if let Err(error) = managed.persist_with_storage(storage) {
            managed.set_status("failed");
            let _ = managed.persist_with_storage(storage);
            stop_managed_child(&managed);
            return Err(error);
        }

        let id = managed.snapshot().id;
        self.sessions
            .lock()
            .expect("agent sessions lock poisoned")
            .insert(id, managed.clone());
        spawn_interval_savings_poller(app.clone(), managed.clone());
        let stderr_reader =
            stderr.map(|stderr| spawn_stderr_reader(managed.stderr_output.clone(), stderr));
        if let Some(stdout) = stdout {
            spawn_stdout_reader(app.clone(), managed.clone(), stdout);
            spawn_process_waiter(app.clone(), managed.clone(), stderr_reader);
        }
        if managed
            .adapter_state
            .lock()
            .expect("agent adapter state lock poisoned")
            .has_init_timeout()
        {
            spawn_codex_init_timeout(app, managed.clone());
        }
        Ok(managed.snapshot())
    }

    #[allow(clippy::too_many_arguments)]
    fn register_reserved_external(
        &self,
        app: AppHandle,
        storage: &Storage,
        id: String,
        agent_kind: AgentKind,
        lane: SessionLane,
        mode: SessionMode,
        ponytail_mode: PonytailMode,
        claude_subscription_proxy: bool,
        cwd: String,
        project_id: Option<i64>,
        pid: u32,
        permission_mode: String,
        session_proxy: Option<SessionProxyContext>,
        previous_session_id: Option<&str>,
    ) -> AgentSessionResult<AgentSession> {
        let now = Utc::now().to_rfc3339();
        let lane_kind = lane.kind();
        let rtk_source_path = exec::rtk_session_database_path(&id);
        let crux_source_path = exec::crux_session_ledger_path(&id);
        let adapter = adapter_for(agent_kind);
        let adapter_state = adapter.new_state(
            false,
            canonical_permission_mode(&permission_mode)?,
            Instant::now(),
            false,
        );
        let proxy_source = session_proxy
            .as_ref()
            .is_some_and(SessionProxyContext::routed)
            && lane.uses_proxy(agent_kind, claude_subscription_proxy);
        let rtk_instruction = adapter.uses_rtk_instruction();
        let (savings_meter, nested_codex_home) = match session_proxy {
            Some(proxy) => (proxy.savings_meter(&id), proxy.nested_codex_home),
            None => (None, None),
        };
        let managed = Arc::new(ManagedAgentSession {
            record: Mutex::new(AgentSession {
                id: id.clone(),
                agent_kind,
                lane_kind,
                mode,
                ponytail_mode: Some(ponytail_mode),
                project_id,
                conversation_id: None,
                conversation_title: None,
                default_title: self.next_default_title(agent_kind),
                generated_title: None,
                ai_vault_title: None,
                custom_title: None,
                generation: 0,
                model: None,
                cwd,
                pid,
                status: "running".to_string(),
                cli_session_id: None,
                created_at: now.clone(),
                last_active_at: now,
                exit_code: None,
                savings_sources: SavingsSources::from_paths(
                    &rtk_source_path,
                    &crux_source_path,
                    proxy_source,
                    rtk_instruction,
                ),
            }),
            adapter,
            adapter_state: Mutex::new(adapter_state),
            lane,
            credentials_rehydrated: false,
            permission_mode,
            writer: Mutex::new(None),
            process: Mutex::new(None),
            stderr_output: Arc::new(Mutex::new(StderrTail::new(AGENT_STDERR_TAIL_BYTES))),
            closing: AtomicBool::new(false),
            finishing: AtomicBool::new(false),
            finished: AtomicBool::new(false),
            process_waiter_spawned: AtomicBool::new(false),
            active_sessions: self.active_sessions.clone(),
            turn_index: AtomicUsize::new(0),
            savings_meter,
            terminal_savings: Some(Mutex::new(TerminalSavingsState::default())),
            rtk_session_db: Some(rtk_source_path.clone()),
            crux_session_ledger: Some(crux_source_path.clone()),
            rtk_source_path,
            crux_source_path,
            proxy_source,
            rtk_instruction,
            memory_generation_effort: chat::DEFAULT_EFFORT.to_string(),
            memories_injected: AtomicBool::new(false),
            conversation_turn: Mutex::new(None),
            nested_codex_usage_before: Mutex::new(None),
            nested_codex_home,
            usage_meter: Mutex::new(usage_meter::SessionUsageMeter::new(
                agent_kind.storage_name(),
            )),
            pending_usage_window: Mutex::new(None),
        });
        managed.persist_with_storage_and_savings_paths(storage)?;
        // Registration keeps the session private until migration sets the
        // counter. The poller starts only after this ordering completes.
        let next_turn_index = match previous_session_id.filter(|previous| *previous != id.as_str())
        {
            Some(previous) => match storage.migrate_agent_session_turns(previous, &id) {
                Ok(next_turn_index) => next_turn_index,
                Err(error) => {
                    managed.set_status("failed");
                    let _ = managed.persist_with_storage(storage);
                    return Err(format!(
                        "failed to migrate resumed session savings: {error}"
                    ));
                }
            },
            None => 0,
        };
        managed.turn_index.store(next_turn_index, Ordering::SeqCst);
        self.sessions
            .lock()
            .expect("agent sessions lock poisoned")
            .insert(id, managed.clone());
        spawn_interval_savings_poller(app, managed.clone());
        Ok(managed.snapshot())
    }

    /// GUI sessions with a turn in progress. Their CLI is a child of the app,
    /// so an app restart stops the turn. An idle session can restart. A turn
    /// that waits for a permission answer is still in progress, although
    /// its status is `waiting_input`.
    pub fn busy_gui_session_count(&self) -> usize {
        let sessions = self
            .sessions
            .lock()
            .expect("agent sessions lock poisoned")
            .values()
            .cloned()
            .collect::<Vec<_>>();
        sessions
            .iter()
            .filter(|session| {
                let snapshot = session.snapshot();
                snapshot.mode == SessionMode::Gui
                    && (matches!(snapshot.status.as_str(), "starting" | "running")
                        || (agent_session_is_live(&snapshot.status)
                            && !session
                                .adapter_state
                                .lock()
                                .expect("agent adapter state lock poisoned")
                                .pending_permissions_empty())
                        || session
                            .conversation_turn
                            .lock()
                            .expect("conversation turn lock poisoned")
                            .is_some())
            })
            .count()
    }

    pub async fn send_user_message(
        &self,
        app: &AppHandle,
        id: &str,
        text: &str,
    ) -> AgentSessionResult<()> {
        self.get(id)?.send_user_message(app, text).await
    }

    pub fn respond_permission(
        &self,
        app: &AppHandle,
        id: &str,
        request_id: &str,
        approve: bool,
    ) -> AgentSessionResult<()> {
        self.get(id)?.respond_permission(app, request_id, approve)
    }

    pub fn kill(
        &self,
        app: &AppHandle,
        terminals: &TerminalManager,
        id: &str,
    ) -> AgentSessionResult<AgentSession> {
        let session = self.get(id)?;
        let snapshot = session.snapshot();
        match snapshot.mode {
            SessionMode::Gui => session.kill(app),
            SessionMode::TerminalEmbedded => {
                let was_live = agent_session_is_live(&snapshot.status);
                if was_live {
                    session.closing.store(true, Ordering::SeqCst);
                }
                if let Err(error) = terminals.close_session(id) {
                    if terminal_session_is_missing(&error, id) {
                        // The terminal lifecycle already disappeared. Finish
                        // the agent record so stale-pane close is idempotent.
                        if was_live {
                            session.finish(app, None, false);
                        }
                    } else {
                        if was_live {
                            session.closing.store(false, Ordering::SeqCst);
                        }
                        return Err(error);
                    }
                }
                Ok(session.snapshot())
            }
            SessionMode::TerminalGhostty => Err(
                "Ghostty sessions cannot be killed from Halv; close the Ghostty window".to_string(),
            ),
        }
    }

    pub fn shutdown(&self) {
        let sessions = self
            .sessions
            .lock()
            .expect("agent sessions lock poisoned")
            .values()
            .cloned()
            .collect::<Vec<_>>();
        for session in sessions {
            session.closing.store(true, Ordering::SeqCst);
            if let Some(child) = session
                .process
                .lock()
                .expect("agent session process lock poisoned")
                .as_mut()
            {
                let _ = child.kill();
            }
        }
    }
}

fn agent_session_is_live(status: &str) -> bool {
    matches!(status, "starting" | "running" | "waiting_input")
}

fn terminal_session_is_missing(error: &str, id: &str) -> bool {
    error == format!("terminal session `{id}` was not found")
}

fn spawn_interval_savings_poller(app: AppHandle, session: Arc<ManagedAgentSession>) {
    let samples_sources =
        session.terminal_savings.is_some() && session.snapshot().mode != SessionMode::Gui;
    thread::spawn(move || loop {
        session.refresh_savings_sources(&app);
        if session.finished.load(Ordering::SeqCst) {
            break;
        }
        // GUI turn settlement consumes session source deltas. The interval
        // poller only reports source deltas for terminal sessions.
        if samples_sources {
            session.sample_terminal_savings(&app, false);
        }
        thread::sleep(TERMINAL_SAVINGS_POLL_INTERVAL);
    });
}

fn terminal_crux_tokens_saved_at(ledger_path: &Path) -> Option<u64> {
    let reported_tokens = exec::crux_tokens_saved_at(ledger_path)?;
    if !ledger_path.is_file() {
        return Some(reported_tokens);
    }
    let contents = std::fs::read_to_string(ledger_path).ok()?;
    Some(contents.lines().fold(0u64, |total, line| {
        let tokens = serde_json::from_str::<serde_json::Value>(line)
            .ok()
            .map(|entry| {
                let raw_tokens = entry
                    .get("saved_tokens")
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or_default();
                entry
                    .get("tool")
                    .and_then(serde_json::Value::as_str)
                    .and_then(|tool| {
                        extensions::crux_savings_estimate(extensions::TERMINAL_CRUX_ID, tool)
                    })
                    .unwrap_or(raw_tokens)
            })
            .unwrap_or_default();
        total.saturating_add(tokens)
    }))
}

fn apply_project_savings_sample(previous: &mut u64, current: u64, record: impl FnOnce(u64)) {
    if let Some(growth) = current.checked_sub(*previous).filter(|growth| *growth > 0) {
        record(growth);
    }
    *previous = current;
}

/// One sample per poll that saw activity: savings growth records its delta,
/// and a message that saved nothing still records a zero point, so every
/// message lands on the meter sparkline.
fn proxy_savings_sample(
    previous_requests: &mut u64,
    requests: u64,
    previous_tokens: &mut u64,
    tokens: u64,
) -> Option<(u64, bool)> {
    let request_growth = requests > *previous_requests;
    let savings_growth = tokens
        .checked_sub(*previous_tokens)
        .filter(|growth| *growth > 0);
    *previous_requests = requests;
    *previous_tokens = tokens;
    match (savings_growth, request_growth) {
        (Some(growth), boundary) => Some((growth, boundary)),
        (None, true) => Some((0, true)),
        (None, false) => None,
    }
}

#[allow(clippy::too_many_arguments)]
fn persist_agent_turn(
    storage: &Storage,
    session_id: &str,
    agent_kind: AgentKind,
    model: Option<&str>,
    mode: SessionMode,
    ponytail_mode: Option<PonytailMode>,
    turn_index: usize,
    usage: &TurnUsage,
    savings: Option<MeasuredSavings>,
    crux_tokens_saved: u64,
    has_message_snapshot: bool,
    created_at: &str,
    price_context: &TurnPriceContext,
) -> AgentSessionResult<f64> {
    let savings_tokens = savings.map(|savings| savings.tokens_saved);
    let price_model = price_context.observed_model.as_deref().or(model);
    let priced = savings.map(|savings| {
        price_turn_savings(
            agent_kind,
            mode,
            price_model,
            usage,
            price_context,
            savings,
            crux_tokens_saved,
        )
    });
    let savings_usd = priced.as_ref().map(|priced| priced.usd);
    let pricing_source = priced.as_ref().map(|priced| priced.source);
    let dollars = TurnDollars {
        model: price_model,
        usd_method: priced.as_ref().map(|priced| priced.method),
        price_version: priced.as_ref().map(|priced| priced.price_version.as_str()),
        prompt_rate_usd_per_m: priced.as_ref().and_then(|priced| priced.prompt_rate),
        source_kind: priced.as_ref().and_then(|priced| priced.source_kind),
        cache_write_1h_tokens: u64_to_i64(price_context.cache_write_1h_tokens),
    };
    let row_id = storage
        .insert_priced_agent_session_turn(
            &AgentSessionTurnRecord {
                session_id,
                turn_index: usize_to_i64(turn_index),
                input_tokens: u64_to_i64(usage.input_tokens),
                output_tokens: u64_to_i64(usage.output_tokens),
                cache_read_tokens: u64_to_i64(usage.cache_read_tokens),
                cache_creation_tokens: u64_to_i64(usage.cache_creation_tokens),
                cost_usd: usage.total_cost_usd,
                savings_tokens: savings_tokens.map(u64_to_i64),
                savings_usd,
                pricing_source: pricing_source.map(PricingSource::as_str),
                crux_tokens_saved: u64_to_i64(crux_tokens_saved),
                savings_source: savings_tokens.map(|_| "measured"),
                has_message_snapshot,
                ponytail_mode,
                created_at,
            },
            Some(&dollars),
        )
        .map_err(|error| error.to_string())?;
    if let Some((event_id, tokens_saved)) = agent_turn_savings_event(
        mode,
        has_message_snapshot,
        session_id,
        row_id,
        savings_tokens,
    ) {
        let enqueued = match priced.as_ref() {
            Some(priced) => storage.enqueue_priced_savings_report(
                &event_id,
                tokens_saved,
                &ReportDollars {
                    kind: "context".to_string(),
                    net_usd: priced.usd,
                    method: priced.method.to_string(),
                    price_version: priced.price_version.clone(),
                },
            ),
            None => storage.enqueue_savings_report(
                &event_id,
                tokens_saved,
                savings_usd.unwrap_or_default(),
            ),
        };
        if let Err(error) = enqueued {
            eprintln!("failed to enqueue agent turn savings report: {error}");
        }
    }
    Ok(savings_usd.unwrap_or_default())
}

/// Prices one measured saving with the methods of ADR 0015. Headroom's own
/// dollar figure prices every token at the list input rate, so Halv no
/// longer uses it.
fn price_turn_savings(
    agent_kind: AgentKind,
    mode: SessionMode,
    model: Option<&str>,
    usage: &TurnUsage,
    price_context: &TurnPriceContext,
    savings: MeasuredSavings,
    crux_tokens_saved: u64,
) -> PricedSavings {
    let kind = agent_kind.storage_name();
    // A terminal turn row carries only Headroom's cache reads, so only a
    // GUI turn has the usage of a real request.
    let turn_rate = (mode == SessionMode::Gui)
        .then(|| {
            dollars::usage_prompt_rate(Some(kind), model, &turn_token_usage(agent_kind, usage))
        })
        .flatten();
    let once = savings.once_tokens.min(savings.tokens_saved);
    let crux = crux_tokens_saved.min(once);
    dollars::price_savings(
        &PromptRates {
            agent_kind: Some(kind),
            model,
            window_rate: price_context.window_rate.or(turn_rate),
            running_rate: price_context.running_rate,
        },
        SavedTokens {
            context: savings.tokens_saved - once,
            rtk: once - crux,
            crux,
        },
    )
}

/// The token classes of a turn usage record. Codex counts cached tokens
/// inside `input_tokens`.
fn turn_token_usage(agent_kind: AgentKind, usage: &TurnUsage) -> pricing::TokenUsage {
    let input = if agent_kind == AgentKind::Codex {
        usage.input_tokens.saturating_sub(
            usage
                .cache_read_tokens
                .saturating_add(usage.cache_creation_tokens),
        )
    } else {
        usage.input_tokens
    };
    pricing::TokenUsage {
        input,
        cache_write_5m: usage.cache_creation_tokens,
        cache_write_1h: 0,
        cache_read: usage.cache_read_tokens,
        output: usage.output_tokens,
    }
}

fn measured_savings_without_crux(
    savings: MeasuredSavings,
    crux_tokens_saved: u64,
) -> MeasuredSavings {
    MeasuredSavings {
        tokens_saved: savings.tokens_saved.saturating_sub(crux_tokens_saved),
        savings_usd: savings.savings_usd,
        once_tokens: savings.once_tokens.saturating_sub(crux_tokens_saved),
    }
}

fn agent_turn_savings_event(
    mode: SessionMode,
    has_message_snapshot: bool,
    session_id: &str,
    row_id: i64,
    savings_tokens: Option<u64>,
) -> Option<(String, i64)> {
    if mode == SessionMode::Gui && has_message_snapshot {
        return None;
    }
    let tokens_saved = savings_tokens
        .map(u64_to_i64)
        .filter(|tokens| *tokens > 0)?;
    Some((format!("agent-turn:{session_id}:{row_id}"), tokens_saved))
}

fn reserve_session_slot(active_sessions: &AtomicUsize) -> AgentSessionResult<()> {
    let mut active = active_sessions.load(Ordering::SeqCst);
    loop {
        if active >= MAX_AGENT_SESSIONS {
            return Err(format!(
                "at most {MAX_AGENT_SESSIONS} agent sessions can run at once; {active} are active"
            ));
        }
        // Keep the declared MSRV without using the deprecated fetch_update API.
        match active_sessions.compare_exchange_weak(
            active,
            active + 1,
            Ordering::SeqCst,
            Ordering::SeqCst,
        ) {
            Ok(_) => return Ok(()),
            Err(current) => active = current,
        }
    }
}

fn normalize_title(title: Option<String>) -> Option<String> {
    title.and_then(|title| {
        let title = title.trim();
        (!title.is_empty()).then(|| title.to_string())
    })
}

fn next_session_generation(generation: u64) -> u64 {
    generation.saturating_add(1)
}

fn normalize_ai_vault_title(title: AiVaultSessionTitle) -> Option<AiVaultSessionTitle> {
    let session_id = title.session_id.trim();
    let title_text = normalize_title(Some(title.title))?;
    (!session_id.is_empty()).then(|| AiVaultSessionTitle {
        session_id: session_id.to_string(),
        title: title_text,
    })
}

fn generated_title_from_prompt(prompt: &str) -> Option<String> {
    let first_line = prompt.lines().find(|line| !line.trim().is_empty())?;
    let compact = first_line.split_whitespace().collect::<Vec<_>>().join(" ");
    if compact.is_empty() {
        return None;
    }
    let mut title = compact.chars().take(56).collect::<String>();
    if compact.chars().count() > 56 {
        title.push('…');
    }
    Some(title)
}

fn persist_snapshot(storage: &Storage, session: &AgentSession) -> AgentSessionResult<()> {
    storage
        .upsert_agent_session(&AgentSessionRecord {
            id: &session.id,
            agent_kind: session.agent_kind.storage_name(),
            lane: session.lane_kind.storage_name(),
            mode: session.mode.storage_name(),
            ponytail_mode: session.ponytail_mode,
            project_id: session.project_id,
            conversation_id: session.conversation_id.as_deref(),
            cwd: &session.cwd,
            cli_session_id: session.cli_session_id.as_deref(),
            custom_title: session.custom_title.as_deref(),
            default_title: &session.default_title,
            generated_title: session.generated_title.as_deref(),
            ai_vault_title: session
                .ai_vault_title
                .as_ref()
                .map(|title| title.title.as_str()),
            ai_vault_session_id: session
                .ai_vault_title
                .as_ref()
                .map(|title| title.session_id.as_str()),
            generation: session.generation,
            status: &session.status,
            created_at: &session.created_at,
            last_active_at: &session.last_active_at,
            exit_code: session.exit_code,
        })
        .map_err(|error| error.to_string())
}

fn spawn_stderr_reader(
    output: Arc<Mutex<StderrTail>>,
    stderr: ChildStderr,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        for line in BufReader::new(stderr).lines() {
            let line = match line {
                Ok(line) => line,
                Err(error) => format!("failed to read agent error output: {error}"),
            };
            output
                .lock()
                .expect("agent stderr lock poisoned")
                .push_line(&line);
        }
    })
}

fn spawn_codex_init_timeout(app: AppHandle, session: Arc<ManagedAgentSession>) {
    let started_at = session
        .adapter_state
        .lock()
        .expect("agent adapter state lock poisoned")
        .init_started_at()
        .expect("initialization timeout requires adapter initialization state");
    thread::spawn(move || {
        let remaining = CODEX_INIT_TIMEOUT.saturating_sub(started_at.elapsed());
        if !remaining.is_zero() {
            thread::sleep(remaining);
        }
        let timeout = session
            .adapter_state
            .lock()
            .expect("agent adapter state lock poisoned")
            .init_timeout(Instant::now());
        let Some(timeout) = timeout else {
            return;
        };
        let stderr_tail = session
            .stderr_output
            .lock()
            .expect("agent stderr lock poisoned")
            .snapshot();
        session.fail_codex_initialization(&app, codex_init_timeout_message(&timeout, &stderr_tail));
    });
}

fn spawn_stdout_reader(app: AppHandle, session: Arc<ManagedAgentSession>, stdout: ChildStdout) {
    thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            match line {
                Ok(line) => {
                    let parsed = session.adapter.parse_line(
                        &mut session
                            .adapter_state
                            .lock()
                            .expect("agent adapter state lock poisoned"),
                        &line,
                    );
                    dispatch_parsed_turn(
                        parsed,
                        |request, event| session.register_pending_permission(&app, request, event),
                        |event| session.handle_event(&app, event),
                        |response| {
                            if let Err(error) = session.write_protocol_line(&response) {
                                session.handle_event(
                                    &app,
                                    AgentEvent::ProtocolError { message: error },
                                );
                            }
                        },
                    );
                }
                Err(error) => {
                    session.handle_event(
                        &app,
                        AgentEvent::ProtocolError {
                            message: format!("failed to read agent protocol output: {error}"),
                        },
                    );
                    break;
                }
            }
        }
    });
}

fn dispatch_parsed_turn(
    parsed: ParsedTurn,
    mut pending: impl FnMut(adapter::PendingPermission, AgentEvent),
    mut event: impl FnMut(AgentEvent),
    mut response: impl FnMut(String),
) {
    let mut pending_permissions = parsed.pending_permissions;
    for parsed_event in parsed.events {
        let pending_index = match &parsed_event {
            AgentEvent::PermissionRequest { request_id, .. } => pending_permissions
                .iter()
                .position(|request| request.request_id == *request_id),
            _ => None,
        };
        if let Some(index) = pending_index {
            pending(pending_permissions.remove(index), parsed_event);
        } else {
            event(parsed_event);
        }
    }
    debug_assert!(pending_permissions.is_empty());
    for encoded_response in parsed.responses {
        response(encoded_response);
    }
}

fn spawn_process_waiter(
    app: AppHandle,
    session: Arc<ManagedAgentSession>,
    mut stderr_reader: Option<thread::JoinHandle<()>>,
) {
    // The waiter thread owns the child reap from this point.
    session.process_waiter_spawned.store(true, Ordering::SeqCst);
    thread::spawn(move || loop {
        let result = {
            let mut process = session
                .process
                .lock()
                .expect("agent session process lock poisoned");
            let Some(child) = process.as_mut() else {
                return;
            };
            child.try_wait()
        };
        match result {
            Ok(Some(status)) => {
                if let Some(reader) = stderr_reader.take() {
                    let _ = reader.join();
                }
                if !status.success() && !session.closing.load(Ordering::SeqCst) {
                    let message = session
                        .stderr_output
                        .lock()
                        .expect("agent stderr lock poisoned")
                        .snapshot();
                    let message = message.trim();
                    if !message.is_empty() {
                        session.handle_event(
                            &app,
                            AgentEvent::ProtocolError {
                                message: message.to_string(),
                            },
                        );
                    }
                }
                session.finish(&app, status.code(), false);
                break;
            }
            Ok(None) => thread::sleep(Duration::from_millis(50)),
            Err(error) => {
                session.handle_event(
                    &app,
                    AgentEvent::ProtocolError {
                        message: format!("failed to wait for agent process: {error}"),
                    },
                );
                session.finish(&app, None, true);
                break;
            }
        }
    });
}

fn spawn_kimi_process_waiter(
    app: AppHandle,
    session: Arc<ManagedAgentSession>,
    mut stderr_reader: Option<thread::JoinHandle<()>>,
) {
    // The waiter thread owns the child reap from this point.
    session.process_waiter_spawned.store(true, Ordering::SeqCst);
    thread::spawn(move || loop {
        let result = {
            let mut process = session
                .process
                .lock()
                .expect("agent session process lock poisoned");
            let Some(child) = process.as_mut() else {
                return;
            };
            child.try_wait()
        };
        match result {
            Ok(Some(status)) => {
                if let Some(reader) = stderr_reader.take() {
                    let _ = reader.join();
                }
                session
                    .process
                    .lock()
                    .expect("agent session process lock poisoned")
                    .take();
                if session.closing.load(Ordering::SeqCst) || !status.success() {
                    if !status.success() && !session.closing.load(Ordering::SeqCst) {
                        let message = session
                            .stderr_output
                            .lock()
                            .expect("agent stderr lock poisoned")
                            .snapshot();
                        let message = message.trim();
                        if !message.is_empty() {
                            session.handle_event(
                                &app,
                                AgentEvent::ProtocolError {
                                    message: message.to_string(),
                                },
                            );
                        }
                    }
                    session.finish(&app, status.code(), false);
                } else {
                    session.set_status("waiting_input");
                    session.persist(&app);
                }
                break;
            }
            Ok(None) => thread::sleep(Duration::from_millis(50)),
            Err(error) => {
                session.handle_event(
                    &app,
                    AgentEvent::ProtocolError {
                        message: format!("failed to wait for Kimi process: {error}"),
                    },
                );
                session.finish(&app, None, true);
                break;
            }
        }
    });
}

fn stop_unmanaged_child(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

fn stop_managed_child(session: &ManagedAgentSession) {
    session
        .writer
        .lock()
        .expect("agent session writer lock poisoned")
        .take();
    let mut child = session
        .process
        .lock()
        .expect("agent session process lock poisoned");
    if let Some(child) = child.as_mut() {
        let _ = child.kill();
        let _ = child.wait();
    }
    child.take();
}

fn turn_usage_delta(after: TurnUsage, before: TurnUsage) -> TurnUsage {
    TurnUsage {
        input_tokens: after.input_tokens.saturating_sub(before.input_tokens),
        output_tokens: after.output_tokens.saturating_sub(before.output_tokens),
        cache_read_tokens: after
            .cache_read_tokens
            .saturating_sub(before.cache_read_tokens),
        cache_creation_tokens: after
            .cache_creation_tokens
            .saturating_sub(before.cache_creation_tokens),
        total_cost_usd: None,
    }
}

fn replace_usage_tokens(usage: &mut TurnUsage, replacement: &TurnUsage) {
    usage.input_tokens = replacement.input_tokens;
    usage.output_tokens = replacement.output_tokens;
    usage.cache_read_tokens = replacement.cache_read_tokens;
    usage.cache_creation_tokens = replacement.cache_creation_tokens;
}

fn add_usage_tokens(usage: &mut TurnUsage, additional: &TurnUsage) {
    usage.input_tokens = usage.input_tokens.saturating_add(additional.input_tokens);
    usage.output_tokens = usage.output_tokens.saturating_add(additional.output_tokens);
    usage.cache_read_tokens = usage
        .cache_read_tokens
        .saturating_add(additional.cache_read_tokens);
    usage.cache_creation_tokens = usage
        .cache_creation_tokens
        .saturating_add(additional.cache_creation_tokens);
}

fn u64_to_i64(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

fn usize_to_i64(value: usize) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentSessionProbes {
    antigravity: AgentProbe,
    claude: AgentProbe,
    codex: AgentProbe,
    gemini: AgentProbe,
    kimi: AgentProbe,
    ghostty: ghostty::GhosttyProbe,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentLaneOptions {
    agent_kind: AgentKind,
    lanes: Vec<AgentLaneOption>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct AgentLaneOption {
    kind: SessionLaneKind,
    logged_in: Option<bool>,
    credential_profiles: Vec<CredentialProfile>,
}

#[derive(Debug, Clone, Copy)]
struct AgentLoginPresence {
    antigravity: Option<bool>,
    claude: Option<bool>,
    codex: Option<bool>,
    gemini: Option<bool>,
    kimi: Option<bool>,
}

fn eligible_profiles(all: &[CredentialProfile], provider: &str) -> Vec<CredentialProfile> {
    all.iter()
        .filter(|profile| {
            profile.provider == provider && profile.kind == "api_key" && profile.status == "active"
        })
        .cloned()
        .collect()
}

fn lane_option(
    kind: SessionLaneKind,
    logged_in: Option<bool>,
    credential_profiles: Vec<CredentialProfile>,
) -> AgentLaneOption {
    AgentLaneOption {
        kind,
        logged_in,
        credential_profiles,
    }
}

fn build_agent_lane_options(
    profiles: &[CredentialProfile],
    presence: AgentLoginPresence,
) -> Vec<AgentLaneOptions> {
    vec![
        AgentLaneOptions {
            agent_kind: AgentKind::Gemini,
            lanes: vec![lane_option(
                SessionLaneKind::Custom,
                presence.gemini,
                Vec::new(),
            )],
        },
        AgentLaneOptions {
            agent_kind: AgentKind::Antigravity,
            lanes: vec![lane_option(
                SessionLaneKind::Custom,
                presence.antigravity,
                Vec::new(),
            )],
        },
        AgentLaneOptions {
            agent_kind: AgentKind::Claude,
            lanes: vec![
                lane_option(
                    SessionLaneKind::ClaudeSubscription,
                    presence.claude,
                    Vec::new(),
                ),
                lane_option(
                    SessionLaneKind::ClaudeApiKey,
                    None,
                    eligible_profiles(profiles, "anthropic"),
                ),
                lane_option(SessionLaneKind::Custom, None, Vec::new()),
            ],
        },
        AgentLaneOptions {
            agent_kind: AgentKind::Codex,
            lanes: vec![
                lane_option(SessionLaneKind::CodexChatGpt, presence.codex, Vec::new()),
                lane_option(SessionLaneKind::Custom, None, Vec::new()),
            ],
        },
        AgentLaneOptions {
            agent_kind: AgentKind::Kimi,
            lanes: vec![
                lane_option(SessionLaneKind::KimiSubscription, presence.kimi, Vec::new()),
                lane_option(
                    SessionLaneKind::KimiApiKey,
                    None,
                    eligible_profiles(profiles, "moonshot"),
                ),
                lane_option(SessionLaneKind::Custom, None, Vec::new()),
            ],
        },
        AgentLaneOptions {
            agent_kind: AgentKind::Zai,
            lanes: vec![
                lane_option(
                    SessionLaneKind::ZaiGlm,
                    None,
                    eligible_profiles(profiles, "zai"),
                ),
                lane_option(SessionLaneKind::Custom, None, Vec::new()),
            ],
        },
        AgentLaneOptions {
            agent_kind: AgentKind::Grok,
            lanes: vec![
                lane_option(
                    SessionLaneKind::GrokApiKey,
                    None,
                    eligible_profiles(profiles, "xai"),
                ),
                lane_option(SessionLaneKind::Custom, None, Vec::new()),
            ],
        },
    ]
}

fn auth_file_exists(candidates: &[&str]) -> Option<bool> {
    let home = std::env::var_os("HOME")?;
    let home = PathBuf::from(home);
    Some(
        candidates
            .iter()
            .any(|candidate| home.join(candidate).is_file()),
    )
}

/// A CLI `--version` result stays valid this long while the binary keeps its
/// fingerprint. A version shim, for example mise, can switch the version
/// without a change to the binary that Halv finds.
const CLI_PROBE_TTL: Duration = Duration::from_secs(60);
/// Halv searches for Ghostty with Spotlight when it is not in /Applications.
const GHOSTTY_PROBE_TTL: Duration = Duration::from_secs(60);

static ANTIGRAVITY_PROBE: SharedCheck<FileFingerprint, AgentProbe> =
    SharedCheck::new(CLI_PROBE_TTL);
static CLAUDE_PROBE: SharedCheck<FileFingerprint, AgentProbe> = SharedCheck::new(CLI_PROBE_TTL);
static CODEX_PROBE: SharedCheck<FileFingerprint, AgentProbe> = SharedCheck::new(CLI_PROBE_TTL);
static GEMINI_PROBE: SharedCheck<FileFingerprint, AgentProbe> = SharedCheck::new(CLI_PROBE_TTL);
static KIMI_PROBE: SharedCheck<FileFingerprint, AgentProbe> = SharedCheck::new(CLI_PROBE_TTL);
static GHOSTTY_PROBE: SharedCheck<Option<FileFingerprint>, ghostty::GhosttyProbe> =
    SharedCheck::new(GHOSTTY_PROBE_TTL);
// A login outside Halv can change the status at any time. A caller gets
// only the result of a run that started after its call.
static CLAUDE_LOGIN: SharedCheck<ClaudeLoginKey, Option<bool>> = SharedCheck::new(Duration::ZERO);

/// Runs the CLI probe of `kind`, or shares a fresh result for the same binary.
fn cached_probe(kind: AgentKind) -> AgentProbe {
    let adapter = adapter_for(kind);
    let Some(binary) = adapter.binary() else {
        // Without a binary the probe spawns nothing.
        return adapter.probe();
    };
    let check = match kind {
        AgentKind::Antigravity => &ANTIGRAVITY_PROBE,
        AgentKind::Claude | AgentKind::Grok | AgentKind::Zai => &CLAUDE_PROBE,
        AgentKind::Codex => &CODEX_PROBE,
        AgentKind::Gemini => &GEMINI_PROBE,
        AgentKind::Kimi => &KIMI_PROBE,
    };
    check.get(file_fingerprint(&binary), || adapter.probe())
}

/// The `--version` text of the Claude Code binary `program`. It shares the
/// Claude CLI probe, so a launch does not run `claude --version` again.
/// `None` when the probe finds another binary or no version.
fn cached_claude_version(program: &Path) -> Option<String> {
    let probe = CLAUDE_PROBE.get(file_fingerprint(program), || {
        adapter_for(AgentKind::Claude).probe()
    });
    let probed = probe.binary.as_deref().map(Path::new);
    (probed == Some(program)).then_some(probe.version).flatten()
}

fn cached_ghostty_probe() -> ghostty::GhosttyProbe {
    GHOSTTY_PROBE.get(ghostty::install_fingerprint(), ghostty::probe)
}

/// The inputs of `claude auth status`: the binary and the active account.
#[derive(PartialEq)]
struct ClaudeLoginKey {
    binary: Option<FileFingerprint>,
    account: String,
}

fn claude_login_present() -> Option<bool> {
    let provider = crate::wallet::WalletProvider::Claude;
    let key = ClaudeLoginKey {
        binary: claude::find_binary().map(|binary| file_fingerprint(&binary)),
        // An Orca switch keeps the folder, so the account key is part of it.
        account: format!(
            "{:?}|{}",
            crate::wallet::probe_environment(provider),
            crate::wallet::usage_cache_key(provider)
        ),
    };
    CLAUDE_LOGIN.get(key, claude::login_status)
}

fn codex_login_present() -> Option<bool> {
    // The check follows the active wallet account.
    let home = codex::codex_home()?;
    Some(home.join("auth.json").is_file())
}

fn kimi_login_present() -> Option<bool> {
    if !cached_probe(AgentKind::Kimi).found {
        return None;
    }
    auth_file_exists(&[
        ".kimi/auth.json",
        ".kimi/credentials.json",
        ".kimi-code/auth.json",
    ])
}

fn gemini_login_present() -> Option<bool> {
    google::gemini_login_status()
}

fn antigravity_login_present() -> Option<bool> {
    google::antigravity_login_status()
}

fn validate_conversation_agent(
    agent_kind: AgentKind,
    lane: &SessionLane,
    model: &str,
) -> AgentSessionResult<()> {
    if model.trim().is_empty() {
        return Err("the agent session model cannot be empty".to_string());
    }
    match (agent_kind, lane) {
        (AgentKind::Claude, SessionLane::ClaudeSubscription) => {
            if !is_claude_code_model(model) {
                return Err(format!("unsupported Claude Code model `{model}`"));
            }
            let claude_probe = adapter_for(agent_kind).probe();
            let claude_login = claude_login_present();
            if !claude_probe.found && claude_login == Some(true) {
                return Err(
                    "Claude account found, but the Claude Code CLI is not installed. Install it to run sessions."
                        .to_string(),
                );
            }
            if !claude_probe.found || claude_login != Some(true) {
                return Err(
                    "Open Agent Sessions and sign in to Claude Code before sending.".to_string(),
                );
            }
        }
        (AgentKind::Codex, SessionLane::CodexChatGpt { .. }) => {
            if !adapter_for(agent_kind).probe().found || codex_login_present() != Some(true) {
                return Err(
                    "Open Agent Sessions and sign in to Codex CLI before sending.".to_string(),
                );
            }
        }
        (AgentKind::Kimi, SessionLane::KimiSubscription | SessionLane::KimiApiKey { .. }) => {
            if !adapter_for(agent_kind).probe().found {
                return Err("Install Kimi Code before sending.".to_string());
            }
        }
        (AgentKind::Zai, SessionLane::ZaiGlm { .. }) => {
            if !adapter_for(agent_kind).probe().found {
                return Err("Install Claude Code before using GLM Coding Plan.".to_string());
            }
        }
        (AgentKind::Grok, SessionLane::GrokApiKey { .. }) => {
            if !adapter_for(agent_kind).probe().found {
                return Err("Install Claude Code before using Grok.".to_string());
            }
        }
        _ => {
            return Err(format!(
                "the {} conversation cannot use the {} lane",
                agent_kind.display_name(),
                lane.kind().storage_name()
            ));
        }
    }
    Ok(())
}

fn agent_login_spec(agent_kind: AgentKind) -> AgentSessionResult<LaunchSpec> {
    adapter_for(agent_kind).login_launch_spec()
}

#[tauri::command]
pub fn agent_login_launch(
    app: AppHandle,
    terminals: tauri::State<'_, TerminalManager>,
    agent_kind: AgentKind,
) -> AgentSessionResult<String> {
    // The login signs in the active wallet account: its own folder for a
    // managed account, else the user's own CLI folder (System default).
    let account = match agent_kind {
        AgentKind::Claude => {
            crate::wallet::launch_environment(crate::wallet::WalletProvider::Claude)?
        }
        AgentKind::Codex => {
            crate::wallet::launch_environment(crate::wallet::WalletProvider::Codex)?
        }
        _ => crate::wallet::AccountEnvironment::default(),
    };
    spawn_login_terminal(app, terminals.inner(), agent_kind, &account)
}

/// Opens the CLI login of `agent_kind` in a terminal. `account` points the
/// login at a wallet account folder.
pub(crate) fn spawn_login_terminal(
    app: AppHandle,
    terminals: &TerminalManager,
    agent_kind: AgentKind,
    account: &crate::wallet::AccountEnvironment,
) -> AgentSessionResult<String> {
    let mut spec = agent_login_spec(agent_kind)?;
    spec.env_remove.extend(account.remove.iter().cloned());
    spec.env.retain(|(name, _)| !account.remove.contains(name));
    spec.env.extend(account.set.iter().cloned());
    let cwd = host_env::user_home_cwd();
    let terminal = terminal::spawn_terminal(
        app,
        terminals,
        terminal::TerminalSpawnRequest {
            id: None,
            project_id: None,
            conversation_id: None,
            project_name: format!("{} login", agent_kind.storage_name()),
            cwd,
            cols: 100,
            rows: 24,
            execution_profile: "full_access".to_string(),
            agent_kind: None,
            program: Some(terminal::TerminalProgram {
                program: spec.program,
                args: spec.args,
                env: spec.env,
                env_remove: spec.env_remove,
            }),
            shell_tab_id: None,
        },
    )?;
    Ok(terminal.id)
}

// The login checks spawn CLIs. A sync command runs on the app main thread,
// which also delivers the keys to the webview, so this command is async and
// does its work on a blocking thread.
#[tauri::command]
pub async fn agent_session_lane_options(
    app: AppHandle,
) -> AgentSessionResult<Vec<AgentLaneOptions>> {
    tauri::async_runtime::spawn_blocking(move || {
        let profiles = app
            .state::<Storage>()
            .list_credential_profiles()
            .map_err(|error| error.to_string())?;
        Ok(build_agent_lane_options(
            &profiles,
            AgentLoginPresence {
                antigravity: antigravity_login_present(),
                claude: claude_login_present(),
                codex: codex_login_present(),
                gemini: gemini_login_present(),
                kimi: kimi_login_present(),
            },
        ))
    })
    .await
    .map_err(|error| format!("failed to read the agent lanes: {error}"))?
}

#[tauri::command]
pub async fn agent_claude_usage_windows() -> claude::ClaudeUsageWindowsResult {
    claude::fetch_usage_windows_live().await
}

#[tauri::command]
pub async fn agent_codex_usage_windows() -> codex::CodexUsageSummary {
    codex::fetch_usage_windows().await
}

/// Returns the quota that the Claude lane still has, in percent. The value is
/// `None` when no window reports a number.
pub(crate) async fn claude_lane_remaining_percent() -> Option<f64> {
    let result = claude::fetch_usage_windows_live().await;
    lowest_remaining_percent(result.windows.into_iter().map(|window| window.used_percent))
}

/// Returns the quota that the Codex lane still has, in percent.
pub(crate) async fn codex_lane_remaining_percent() -> Option<f64> {
    let summary = codex::fetch_usage_windows().await;
    lowest_remaining_percent(
        summary
            .windows
            .into_iter()
            .map(|window| window.used_percent),
    )
}

fn lowest_remaining_percent(used: impl Iterator<Item = Option<f64>>) -> Option<f64> {
    // The tightest window decides the lane quota.
    used.flatten()
        .fold(None, |highest: Option<f64>, value| {
            Some(highest.map_or(value, |current| current.max(value)))
        })
        .map(|highest| (100.0 - highest).clamp(0.0, 100.0))
}

// Each probe spawns a CLI, so the probes run on a blocking thread and never
// on the app main thread.
#[tauri::command]
pub async fn agent_session_probe() -> AgentSessionResult<AgentSessionProbes> {
    tauri::async_runtime::spawn_blocking(|| AgentSessionProbes {
        antigravity: cached_probe(AgentKind::Antigravity),
        claude: cached_probe(AgentKind::Claude),
        codex: cached_probe(AgentKind::Codex),
        gemini: cached_probe(AgentKind::Gemini),
        kimi: cached_probe(AgentKind::Kimi),
        ghostty: cached_ghostty_probe(),
    })
    .await
    .map_err(|error| format!("failed to probe the agent CLIs: {error}"))
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn agent_session_create(
    app: AppHandle,
    manager: tauri::State<'_, AgentSessionManager>,
    storage: tauri::State<'_, Storage>,
    preferences: tauri::State<'_, PreferencesManager>,
    sidecar: tauri::State<'_, SidecarManager>,
    agent_kind: AgentKind,
    lane: SessionLane,
    cwd: String,
    project_id: Option<i64>,
    conversation_id: Option<String>,
    conversation_title: Option<String>,
    model: Option<String>,
    effort: Option<String>,
    permission_mode: Option<String>,
    resume_cli_session_id: Option<String>,
    title_state: Option<AgentSessionTitleState>,
) -> AgentSessionResult<AgentSession> {
    validate_conversation_tag(&conversation_id, &conversation_title, &model)?;
    let lane = resolve_lane_credentials(&storage, lane)?;
    let ponytail_mode = preferences.ponytail()?;
    let session_proxy = session_proxy_context(agent_kind, &lane, sidecar.inner()).await?;
    let session = match resume_cli_session_id.clone() {
        Some(cli_session_id) => manager.resume(
            app.clone(),
            &storage,
            agent_kind,
            lane,
            cwd,
            project_id,
            permission_mode,
            ponytail_mode,
            session_proxy,
            cli_session_id,
            model.clone(),
            effort.clone(),
        ),
        None => manager.spawn(
            app.clone(),
            &storage,
            agent_kind,
            lane,
            cwd,
            project_id,
            permission_mode,
            ponytail_mode,
            session_proxy,
            model.clone(),
            effort.clone(),
        ),
    }?;
    let session = manager.apply_title_state(&storage, &session.id, title_state)?;
    tag_session_conversation(
        &app,
        manager.inner(),
        &session.id,
        conversation_id.as_deref(),
        conversation_title.as_deref(),
        model.as_deref(),
        resume_cli_session_id.as_deref(),
    )?;
    manager.snapshot(&session.id)
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentSessionSavedTotals {
    pub session_id: String,
    pub conversation_id: Option<String>,
    pub savings_tokens: u64,
    pub crux_tokens_saved: u64,
    pub crux_included_in_total: bool,
    pub cache_read_tokens: u64,
    pub pricing_source: Option<String>,
    /// Dollars saved with the methods of ADR 0015.
    pub dollars_usd: f64,
}

/// Persisted per-session savings aggregates. The frontend seeds its live
/// meter totals from these on startup, so an app restart never shows a
/// session back at zero.
#[tauri::command]
pub async fn agent_session_saved_totals(
    storage: tauri::State<'_, Storage>,
) -> AgentSessionResult<Vec<AgentSessionSavedTotals>> {
    storage
        .agent_session_turn_totals()
        .map(|rows| {
            rows.into_iter()
                .map(|row| AgentSessionSavedTotals {
                    session_id: row.session_id,
                    conversation_id: row.conversation_id,
                    savings_tokens: row.savings_tokens,
                    crux_tokens_saved: row.crux_tokens_saved,
                    crux_included_in_total: true,
                    cache_read_tokens: row.cache_read_tokens,
                    pricing_source: row.pricing_source,
                    dollars_usd: row.dollars_usd,
                })
                .collect()
        })
        .map_err(|error| error.to_string())
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum LaunchSurfaceMode {
    Embedded { cols: u16, rows: u16 },
    Ghostty,
}

#[derive(Clone, Copy)]
enum LaneCredentialMode {
    Resolve,
    Rehydrate { required: bool },
}

enum PermissionModeSource {
    Requested(Option<String>),
    Inherited(String),
}

#[derive(Clone, Copy)]
enum SessionDiscoveryMode {
    FreshOnly,
    Always,
}

#[derive(Clone, Copy)]
enum PostRegisterFailureMode {
    LeaveRunning,
    CloseEmbedded,
}

struct ConversationTag {
    id: Option<String>,
    title: Option<String>,
    model: Option<String>,
}

struct SessionRequest {
    agent_kind: AgentKind,
    lane: SessionLane,
    lane_credential_mode: LaneCredentialMode,
    permission_mode_source: PermissionModeSource,
    cwd: String,
    project_id: Option<i64>,
    conversation: ConversationTag,
    surface_mode: LaunchSurfaceMode,
    initial_prompt: Option<String>,
    resume_cli_session_id: Option<String>,
    previous_session_id: Option<String>,
    title_state: Option<AgentSessionTitleState>,
    validate_conversation: bool,
    apply_title_state: bool,
    touch_conversation_activity: bool,
    discovery_mode: SessionDiscoveryMode,
    post_register_failure_mode: PostRegisterFailureMode,
}

struct SlotAndProxyGuard {
    active_sessions: Arc<AtomicUsize>,
    session_proxy: Option<SessionProxyContext>,
    slot_reserved: bool,
    disarmed: bool,
}

impl SlotAndProxyGuard {
    fn new(active_sessions: Arc<AtomicUsize>, session_proxy: Option<SessionProxyContext>) -> Self {
        Self {
            active_sessions,
            session_proxy,
            slot_reserved: false,
            disarmed: false,
        }
    }

    fn session_proxy(&self) -> &Option<SessionProxyContext> {
        &self.session_proxy
    }

    fn session_proxy_mut(&mut self) -> &mut Option<SessionProxyContext> {
        &mut self.session_proxy
    }

    fn mark_slot_reserved(&mut self) {
        self.slot_reserved = true;
    }

    fn disarm(&mut self) {
        self.disarmed = true;
    }
}

impl Drop for SlotAndProxyGuard {
    fn drop(&mut self) {
        if self.disarmed {
            return;
        }
        shutdown_owned_proxy(&self.session_proxy);
        if self.slot_reserved {
            self.active_sessions.fetch_sub(1, Ordering::SeqCst);
        }
    }
}

struct PreparedLaunch {
    session_id: String,
    launch_spec: LaunchSpec,
    session_proxy: Option<SessionProxyContext>,
    project_name: Option<String>,
    guard: SlotAndProxyGuard,
    agent_kind: AgentKind,
    lane: SessionLane,
    permission_mode: String,
    ponytail_mode: PonytailMode,
    claude_subscription_proxy: bool,
    cwd: String,
    project_id: Option<i64>,
    conversation: ConversationTag,
    surface_mode: LaunchSurfaceMode,
    assigned_cli_session_id: Option<String>,
    resume_cli_session_id: Option<String>,
    previous_session_id: Option<String>,
    title_state: Option<AgentSessionTitleState>,
    apply_title_state: bool,
    initial_prompt_marks_activity: bool,
    discovery_mode: SessionDiscoveryMode,
    post_register_failure_mode: PostRegisterFailureMode,
}

struct SpawnedSurface {
    terminal_id: Option<String>,
    pid: u32,
}

struct SessionLauncher<'a> {
    app: &'a AppHandle,
    manager: &'a AgentSessionManager,
    terminals: Option<&'a TerminalManager>,
    storage: &'a Storage,
    preferences: &'a PreferencesManager,
    sidecar: &'a SidecarManager,
}

impl SessionLauncher<'_> {
    async fn prepare(&self, mut req: SessionRequest) -> AgentSessionResult<PreparedLaunch> {
        if req.validate_conversation {
            validate_conversation_tag(
                &req.conversation.id,
                &req.conversation.title,
                &req.conversation.model,
            )?;
        }
        if req.surface_mode == LaunchSurfaceMode::Ghostty {
            let ghostty_probe = ghostty::probe();
            if !ghostty_probe.found {
                return Err("Ghostty is not installed".to_string());
            }
            if !ghostty_probe.applescript_ready {
                return Err("Ghostty 1.3 or newer with AppleScript access is required".to_string());
            }
        }
        req.lane = match req.lane_credential_mode {
            LaneCredentialMode::Resolve => resolve_lane_credentials(self.storage, req.lane)?,
            LaneCredentialMode::Rehydrate { required: true } => {
                resolve_rehydrated_lane_credentials(self.storage, req.lane)
                    .map_err(|_| DUPLICATE_CREDENTIAL_REFRESH_ERROR.to_string())?
            }
            LaneCredentialMode::Rehydrate { required: false } => req.lane,
        };
        let ponytail_mode = self.preferences.ponytail()?;
        // The subscription lane uses the engine only when both switches are on.
        let claude_subscription_proxy = self.preferences.effective_claude_subscription_proxy()?;
        let task_routing = self.preferences.task_routing()?;
        let worker_lane = self.preferences.worker_lane()?;
        let tier_agents = self.preferences.routing_tier_agents()?;
        let workflow_routing = self.preferences.workflow_routing()?;
        let claude_ultracode = self.preferences.claude_ultracode()?;
        let permission_mode = match req.permission_mode_source {
            PermissionModeSource::Requested(permission_mode) => canonical_permission_mode(
                permission_mode
                    .as_deref()
                    .unwrap_or(DEFAULT_PERMISSION_MODE),
            )?
            .to_string(),
            PermissionModeSource::Inherited(permission_mode) => permission_mode,
        };
        let session_proxy = session_proxy_context(req.agent_kind, &req.lane, self.sidecar).await?;
        // The engine switch can change between the preference read and the
        // context. A lane without an engine port goes direct.
        let claude_subscription_proxy = claude_subscription_proxy
            && session_proxy
                .as_ref()
                .is_some_and(SessionProxyContext::routed);
        let mut guard = SlotAndProxyGuard::new(self.manager.active_sessions.clone(), session_proxy);
        let session_id = new_uuid_v4();
        prepare_nested_codex_home(req.agent_kind, guard.session_proxy_mut(), &session_id)?;
        let assigned_cli_session_id = assigned_interactive_cli_session_id(
            req.agent_kind,
            req.resume_cli_session_id.as_deref(),
        );
        let initial_prompt_marks_activity = req.touch_conversation_activity
            && initial_prompt_arg(
                req.agent_kind,
                req.resume_cli_session_id.as_deref(),
                req.initial_prompt.as_deref(),
            )
            .is_some();
        let cwd_path = PathBuf::from(&req.cwd);
        let extension_manager = self
            .app
            .state::<extensions::ExtensionManager>()
            .inner()
            .clone();
        let launch_spec = launch_spec(
            req.agent_kind,
            &req.lane,
            &cwd_path,
            &extension_manager,
            req.resume_cli_session_id.as_deref(),
            assigned_cli_session_id.as_deref(),
            Some(&permission_mode),
            req.initial_prompt.as_deref(),
            guard.session_proxy().as_ref().and_then(|proxy| proxy.port),
            &session_id,
            self.storage.database_path(),
            guard
                .session_proxy()
                .as_ref()
                .and_then(|proxy| proxy.nested_codex_home.as_deref()),
            ponytail_mode,
            claude_subscription_proxy,
            req.conversation.model.as_deref(),
            req.conversation.id.as_deref(),
            task_routing,
            worker_lane,
            tier_agents,
            workflow_routing,
            claude_ultracode,
            matches!(req.surface_mode, LaunchSurfaceMode::Embedded { .. }),
        )?;
        let project_name = project_name(self.storage, req.project_id, &cwd_path)?;
        let project_name = (req.surface_mode != LaunchSurfaceMode::Ghostty).then_some(project_name);
        reserve_session_slot(&self.manager.active_sessions)?;
        guard.mark_slot_reserved();
        let session_proxy = guard.session_proxy().clone();
        Ok(PreparedLaunch {
            session_id,
            launch_spec,
            session_proxy,
            project_name,
            guard,
            agent_kind: req.agent_kind,
            lane: req.lane,
            permission_mode,
            ponytail_mode,
            claude_subscription_proxy,
            cwd: req.cwd,
            project_id: req.project_id,
            conversation: req.conversation,
            surface_mode: req.surface_mode,
            assigned_cli_session_id,
            resume_cli_session_id: req.resume_cli_session_id,
            previous_session_id: req.previous_session_id,
            title_state: req.title_state,
            apply_title_state: req.apply_title_state,
            initial_prompt_marks_activity,
            discovery_mode: req.discovery_mode,
            post_register_failure_mode: req.post_register_failure_mode,
        })
    }

    fn post_register_failure<T>(
        &self,
        mode: PostRegisterFailureMode,
        terminal_id: Option<&str>,
        error: String,
    ) -> AgentSessionResult<T> {
        // TODO(launcher): unify post-register failure handling
        if matches!(mode, PostRegisterFailureMode::CloseEmbedded) {
            if let (Some(terminals), Some(terminal_id)) = (self.terminals, terminal_id) {
                let _ = terminals.close_session(terminal_id);
            }
        }
        Err(error)
    }

    fn launch(
        &self,
        mut prepared: PreparedLaunch,
        surface_mode: LaunchSurfaceMode,
    ) -> AgentSessionResult<AgentSession> {
        if surface_mode != prepared.surface_mode {
            return Err("prepared launch surface does not match the requested surface".to_string());
        }
        let spawned_at = SystemTime::now();
        let spawned_surface = match surface_mode {
            LaunchSurfaceMode::Embedded { cols, rows } => {
                let terminals = self
                    .terminals
                    .ok_or_else(|| "the embedded terminal manager is unavailable".to_string())?;
                let project_name = prepared.project_name.take().ok_or_else(|| {
                    "the embedded terminal project name is unavailable".to_string()
                })?;
                let spec = prepared.launch_spec;
                crate::wallet::record_launch(
                    &prepared.session_id,
                    prepared.agent_kind,
                    prepared.lane.kind(),
                    &spec.env,
                );
                crate::agent_status::register_launch(
                    self.app,
                    &prepared.session_id,
                    prepared.agent_kind.storage_name(),
                    &spec.env,
                    &spec.env_remove,
                );
                let terminal = terminal::spawn_terminal(
                    self.app.clone(),
                    terminals,
                    terminal::TerminalSpawnRequest {
                        id: Some(prepared.session_id.clone()),
                        project_id: prepared.project_id,
                        conversation_id: prepared.conversation.id.clone(),
                        project_name,
                        cwd: PathBuf::from(&prepared.cwd),
                        cols,
                        rows,
                        execution_profile: "full_access".to_string(),
                        agent_kind: Some(prepared.agent_kind.storage_name().to_string()),
                        program: Some(terminal::TerminalProgram {
                            program: spec.program,
                            args: spec.args,
                            env: spec.env,
                            env_remove: spec.env_remove,
                        }),
                        shell_tab_id: None,
                    },
                )?;
                SpawnedSurface {
                    terminal_id: Some(terminal.id),
                    pid: terminal.pid,
                }
            }
            LaunchSurfaceMode::Ghostty => {
                ghostty::launch(&prepared.launch_spec, Path::new(&prepared.cwd))?;
                SpawnedSurface {
                    terminal_id: None,
                    pid: 0,
                }
            }
        };
        let session_mode = match surface_mode {
            LaunchSurfaceMode::Embedded { .. } => SessionMode::TerminalEmbedded,
            LaunchSurfaceMode::Ghostty => SessionMode::TerminalGhostty,
        };
        let session = match self.manager.register_reserved_external(
            self.app.clone(),
            self.storage,
            prepared.session_id.clone(),
            prepared.agent_kind,
            prepared.lane,
            session_mode,
            prepared.ponytail_mode,
            prepared.claude_subscription_proxy,
            prepared.cwd,
            prepared.project_id,
            spawned_surface.pid,
            prepared.permission_mode,
            prepared.session_proxy.take(),
            prepared.previous_session_id.as_deref(),
        ) {
            Ok(session) => session,
            Err(error) => {
                if let (Some(terminals), Some(terminal_id)) =
                    (self.terminals, spawned_surface.terminal_id.as_deref())
                {
                    let _ = terminals.close_session(terminal_id);
                }
                return Err(error);
            }
        };
        prepared.guard.disarm();
        let session = if prepared.apply_title_state {
            match self
                .manager
                .apply_title_state(self.storage, &session.id, prepared.title_state)
            {
                Ok(session) => session,
                Err(error) => {
                    return self.post_register_failure(
                        prepared.post_register_failure_mode,
                        spawned_surface.terminal_id.as_deref(),
                        error,
                    );
                }
            }
        } else {
            session
        };
        if let Err(error) = tag_session_conversation(
            self.app,
            self.manager,
            &prepared.session_id,
            prepared.conversation.id.as_deref(),
            prepared.conversation.title.as_deref(),
            prepared.conversation.model.as_deref(),
            prepared
                .resume_cli_session_id
                .as_deref()
                .or(prepared.assigned_cli_session_id.as_deref()),
        ) {
            return self.post_register_failure(
                prepared.post_register_failure_mode,
                spawned_surface.terminal_id.as_deref(),
                error,
            );
        }
        if prepared.initial_prompt_marks_activity {
            if let Some(conversation_id) = prepared.conversation.id.as_deref() {
                if let Err(error) = self.storage.touch_conversation(conversation_id) {
                    eprintln!("failed to update conversation activity: {error}");
                }
                if let Err(error) = terminal::emit_conversation_activity(self.app, conversation_id)
                {
                    eprintln!("failed to emit conversation activity: {error}");
                }
            }
        }
        let discover_session = match prepared.discovery_mode {
            SessionDiscoveryMode::Always => true,
            SessionDiscoveryMode::FreshOnly => prepared.resume_cli_session_id.is_none(),
        };
        if adapter_for(prepared.agent_kind).requires_session_discovery() && discover_session {
            spawn_codex_session_discovery(
                self.app.clone(),
                self.manager.clone(),
                prepared.session_id.clone(),
                PathBuf::from(&session.cwd),
                spawned_at,
            );
        }
        match surface_mode {
            LaunchSurfaceMode::Embedded { .. } => spawn_embedded_lifecycle(
                self.app.clone(),
                self.manager.clone(),
                self.terminals
                    .expect("embedded launches require a terminal manager")
                    .clone(),
                prepared.session_id,
            ),
            LaunchSurfaceMode::Ghostty => {
                spawn_ghostty_lifecycle(self.app.clone(), self.manager.clone(), prepared.session_id)
            }
        }
        self.manager.snapshot(&session.id)
    }
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn agent_terminal_create(
    app: AppHandle,
    manager: tauri::State<'_, AgentSessionManager>,
    terminals: tauri::State<'_, TerminalManager>,
    storage: tauri::State<'_, Storage>,
    preferences: tauri::State<'_, PreferencesManager>,
    sidecar: tauri::State<'_, SidecarManager>,
    agent_kind: AgentKind,
    lane: SessionLane,
    cwd: String,
    project_id: Option<i64>,
    conversation_id: Option<String>,
    conversation_title: Option<String>,
    model: Option<String>,
    permission_mode: Option<String>,
    initial_prompt: Option<String>,
    resume_cli_session_id: Option<String>,
    previous_session_id: Option<String>,
    cols: Option<u16>,
    rows: Option<u16>,
    title_state: Option<AgentSessionTitleState>,
) -> AgentSessionResult<AgentSession> {
    let surface_mode = LaunchSurfaceMode::Embedded {
        cols: cols.unwrap_or(100),
        rows: rows.unwrap_or(24),
    };
    let launcher = SessionLauncher {
        app: &app,
        manager: manager.inner(),
        terminals: Some(terminals.inner()),
        storage: storage.inner(),
        preferences: preferences.inner(),
        sidecar: sidecar.inner(),
    };
    let prepared = launcher
        .prepare(SessionRequest {
            agent_kind,
            lane,
            lane_credential_mode: LaneCredentialMode::Resolve,
            permission_mode_source: PermissionModeSource::Requested(permission_mode),
            cwd,
            project_id,
            conversation: ConversationTag {
                id: conversation_id,
                title: conversation_title,
                model,
            },
            surface_mode,
            initial_prompt,
            resume_cli_session_id,
            previous_session_id,
            title_state,
            validate_conversation: true,
            apply_title_state: true,
            touch_conversation_activity: true,
            discovery_mode: SessionDiscoveryMode::FreshOnly,
            post_register_failure_mode: PostRegisterFailureMode::LeaveRunning,
        })
        .await?;
    launcher.launch(prepared, surface_mode)
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn agent_terminal_duplicate(
    app: AppHandle,
    manager: tauri::State<'_, AgentSessionManager>,
    terminals: tauri::State<'_, TerminalManager>,
    storage: tauri::State<'_, Storage>,
    preferences: tauri::State<'_, PreferencesManager>,
    sidecar: tauri::State<'_, SidecarManager>,
    session_id: String,
    cols: Option<u16>,
    rows: Option<u16>,
) -> AgentSessionResult<AgentSession> {
    let source = manager.get(&session_id)?;
    let snapshot = source.snapshot();
    if snapshot.mode != SessionMode::TerminalEmbedded {
        return Err("only built-in agent terminals can be split".to_string());
    }
    let surface_mode = LaunchSurfaceMode::Embedded {
        cols: cols.unwrap_or(100),
        rows: rows.unwrap_or(24),
    };
    let launcher = SessionLauncher {
        app: &app,
        manager: manager.inner(),
        terminals: Some(terminals.inner()),
        storage: storage.inner(),
        preferences: preferences.inner(),
        sidecar: sidecar.inner(),
    };
    let prepared = launcher
        .prepare(SessionRequest {
            agent_kind: snapshot.agent_kind,
            lane: source.lane.clone(),
            lane_credential_mode: LaneCredentialMode::Rehydrate {
                required: source.credentials_rehydrated,
            },
            permission_mode_source: PermissionModeSource::Inherited(source.permission_mode.clone()),
            cwd: snapshot.cwd,
            project_id: snapshot.project_id,
            conversation: ConversationTag {
                id: snapshot.conversation_id,
                title: snapshot.conversation_title,
                model: snapshot.model,
            },
            surface_mode,
            initial_prompt: None,
            resume_cli_session_id: None,
            previous_session_id: None,
            title_state: None,
            validate_conversation: false,
            apply_title_state: false,
            touch_conversation_activity: false,
            discovery_mode: SessionDiscoveryMode::Always,
            post_register_failure_mode: PostRegisterFailureMode::CloseEmbedded,
        })
        .await?;
    launcher.launch(prepared, surface_mode)
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn agent_ghostty_launch(
    app: AppHandle,
    manager: tauri::State<'_, AgentSessionManager>,
    storage: tauri::State<'_, Storage>,
    preferences: tauri::State<'_, PreferencesManager>,
    sidecar: tauri::State<'_, SidecarManager>,
    agent_kind: AgentKind,
    lane: SessionLane,
    cwd: String,
    project_id: Option<i64>,
    conversation_id: Option<String>,
    conversation_title: Option<String>,
    model: Option<String>,
    permission_mode: Option<String>,
    resume_cli_session_id: Option<String>,
    title_state: Option<AgentSessionTitleState>,
) -> AgentSessionResult<AgentSession> {
    let surface_mode = LaunchSurfaceMode::Ghostty;
    let launcher = SessionLauncher {
        app: &app,
        manager: manager.inner(),
        terminals: None,
        storage: storage.inner(),
        preferences: preferences.inner(),
        sidecar: sidecar.inner(),
    };
    let prepared = launcher
        .prepare(SessionRequest {
            agent_kind,
            lane,
            lane_credential_mode: LaneCredentialMode::Resolve,
            permission_mode_source: PermissionModeSource::Requested(permission_mode),
            cwd,
            project_id,
            conversation: ConversationTag {
                id: conversation_id,
                title: conversation_title,
                model,
            },
            surface_mode,
            initial_prompt: None,
            resume_cli_session_id,
            previous_session_id: None,
            title_state,
            validate_conversation: true,
            apply_title_state: true,
            touch_conversation_activity: false,
            discovery_mode: SessionDiscoveryMode::FreshOnly,
            post_register_failure_mode: PostRegisterFailureMode::LeaveRunning,
        })
        .await?;
    launcher.launch(prepared, surface_mode)
}

fn project_name(
    storage: &Storage,
    project_id: Option<i64>,
    cwd: &Path,
) -> AgentSessionResult<String> {
    if let Some(project_id) = project_id {
        return storage
            .get_project(project_id)
            .map(|project| project.name)
            .map_err(|error| error.to_string());
    }
    Ok(cwd
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or("Agent session")
        .to_string())
}

fn validate_conversation_tag(
    conversation_id: &Option<String>,
    conversation_title: &Option<String>,
    model: &Option<String>,
) -> AgentSessionResult<()> {
    let supplied = [
        conversation_id.is_some(),
        conversation_title.is_some(),
        model.is_some(),
    ];
    if supplied.iter().any(|value| *value) && !supplied.iter().all(|value| *value) {
        return Err("conversation id, title, and model must be supplied together".to_string());
    }
    Ok(())
}

fn tag_session_conversation(
    app: &AppHandle,
    manager: &AgentSessionManager,
    session_id: &str,
    conversation_id: Option<&str>,
    conversation_title: Option<&str>,
    model: Option<&str>,
    cli_session_id: Option<&str>,
) -> AgentSessionResult<()> {
    let managed = manager.get(session_id)?;
    if let Some(cli_session_id) = cli_session_id {
        managed.set_cli_session_id(cli_session_id);
    }
    let (Some(conversation_id), Some(conversation_title), Some(model)) =
        (conversation_id, conversation_title, model)
    else {
        managed.persist(app);
        return Ok(());
    };
    managed.tag_conversation(conversation_id, conversation_title, model);
    managed.persist(app);
    Ok(())
}

fn shutdown_owned_proxy(session_proxy: &Option<SessionProxyContext>) {
    if let Some(proxy) = session_proxy {
        if proxy.shutdown_on_exit {
            if let Some(sidecar) = &proxy.sidecar {
                sidecar.shutdown();
            }
        }
        if let Some(codex_home) = &proxy.nested_codex_home {
            codex::remove_nested_home(codex_home);
        }
    }
}

// Claude accepts a pre-assigned interactive session id, so fresh Claude and
// GLM and Grok terminals are resumable from the start. Codex has no such flag;
// its id comes from rollout-file discovery instead.
fn assigned_interactive_cli_session_id(
    agent_kind: AgentKind,
    resume_cli_session_id: Option<&str>,
) -> Option<String> {
    adapter_for(agent_kind)
        .assigned_interactive_session_id(resume_cli_session_id)
        .then(new_uuid_v4)
}

const CODEX_DISCOVERY_TIMEOUT: Duration = Duration::from_secs(120);
const CODEX_DISCOVERY_INTERVAL: Duration = Duration::from_millis(750);

fn spawn_codex_session_discovery(
    app: AppHandle,
    manager: AgentSessionManager,
    session_id: String,
    cwd: PathBuf,
    spawned_at: SystemTime,
) {
    thread::spawn(move || {
        let deadline = Instant::now() + CODEX_DISCOVERY_TIMEOUT;
        loop {
            let Ok(session) = manager.get(&session_id) else {
                return;
            };
            if session.snapshot().cli_session_id.is_some()
                || session.finished.load(Ordering::SeqCst)
            {
                return;
            }
            let candidates = codex::discover_session_ids(&cwd, spawned_at);
            if claim_cli_session_id(&app, &manager, &session_id, &candidates) {
                return;
            }
            if Instant::now() >= deadline {
                return;
            }
            thread::sleep(CODEX_DISCOVERY_INTERVAL);
        }
    });
}

// The claim runs under the sessions lock so two concurrent Codex spawns in
// the same folder never adopt the same discovered rollout id.
fn claim_cli_session_id(
    app: &AppHandle,
    manager: &AgentSessionManager,
    session_id: &str,
    candidates: &[String],
) -> bool {
    if candidates.is_empty() {
        return false;
    }
    let claimed = {
        let sessions = manager
            .sessions
            .lock()
            .expect("agent sessions lock poisoned");
        let used = sessions
            .values()
            .filter_map(|session| session.snapshot().cli_session_id)
            .collect::<std::collections::HashSet<_>>();
        let Some(candidate) = candidates
            .iter()
            .find(|candidate| !used.contains(*candidate))
        else {
            return false;
        };
        let Some(managed) = sessions.get(session_id).cloned() else {
            return false;
        };
        managed.set_cli_session_id(candidate);
        managed
    };
    claimed.persist(app);
    let model = claimed.snapshot().model.unwrap_or_default();
    claimed.emit(
        app,
        AgentEvent::SessionInit {
            cli_session_id: claimed.snapshot().cli_session_id.unwrap_or_default(),
            model,
        },
    );
    true
}

fn prepare_nested_codex_home(
    agent_kind: AgentKind,
    session_proxy: &mut Option<SessionProxyContext>,
    session_id: &str,
) -> AgentSessionResult<()> {
    let Some(proxy) = session_proxy.as_mut() else {
        return Ok(());
    };
    if !adapter_for(agent_kind).supports_nested_codex_home(proxy.shutdown_on_exit) {
        return Ok(());
    }
    let codex_home = codex::prepare_nested_home(proxy.port, session_id)?;
    proxy.nested_codex_home = Some(codex_home);
    Ok(())
}

fn spawn_embedded_lifecycle(
    app: AppHandle,
    manager: AgentSessionManager,
    terminals: TerminalManager,
    session_id: String,
) {
    thread::spawn(move || loop {
        match terminals.session(&session_id) {
            Ok(terminal) if terminal.status == "running" => {
                thread::sleep(Duration::from_millis(250));
            }
            Ok(terminal) => {
                let agent = manager.get(&session_id).ok();
                // A clean exit (`/exit`) continues the pane in a plain shell.
                // The shell starts before `finish`, so `terminal_continued`
                // reaches the webview before `SessionExited`.
                let halv_stopped = terminals.halv_stopped(&session_id)
                    || agent
                        .as_ref()
                        .is_some_and(|session| session.closing.load(Ordering::SeqCst));
                if terminal::clean_self_exit(&terminal, halv_stopped) {
                    if let Err(error) = terminal::continue_with_shell(&app, &terminals, &session_id)
                    {
                        eprintln!("agent pane could not continue in a shell: {error}");
                    }
                }
                if let Some(session) = agent {
                    session.finish(&app, terminal.exit_code, terminal.status == "failed");
                }
                break;
            }
            Err(_) => {
                if let Ok(session) = manager.get(&session_id) {
                    session.finish(&app, None, true);
                }
                break;
            }
        }
    });
}

fn spawn_ghostty_lifecycle(app: AppHandle, manager: AgentSessionManager, session_id: String) {
    thread::spawn(move || loop {
        thread::sleep(TERMINAL_SAVINGS_POLL_INTERVAL);
        let Ok(session) = manager.get(&session_id) else {
            break;
        };
        if session.finished.load(Ordering::SeqCst) {
            break;
        }
        if !ghostty::is_running() {
            session.finish(&app, Some(0), false);
            break;
        }
    });
}

async fn session_proxy_context(
    agent_kind: AgentKind,
    lane: &SessionLane,
    sidecar: &SidecarManager,
) -> AgentSessionResult<Option<SessionProxyContext>> {
    let custom_codex_api_key = matches!(
        (agent_kind, lane),
        (AgentKind::Codex, SessionLane::Custom { env })
            if env
                .iter()
                .rev()
                .find(|(name, _)| name == "OPENAI_API_KEY")
                .is_some_and(|(_, value)| !value.trim().is_empty())
    );
    // The Claude subscription lane stays in this list also while its opt-in
    // is off: its nested Codex home routes the JEV Codex workers through
    // the engine and measures them. Only its own Anthropic traffic follows
    // the opt-in (`claude::apply_lane_environment`).
    let requires_shared_proxy = custom_codex_api_key
        || matches!(
            (agent_kind, lane),
            (
                AgentKind::Codex,
                SessionLane::CodexChatGpt { proxied: true }
            ) | (
                AgentKind::Claude | AgentKind::Zai,
                SessionLane::ClaudeSubscription
                    | SessionLane::ClaudeApiKey { .. }
                    | SessionLane::ZaiGlm { .. }
                    | SessionLane::Custom { .. }
            ) | (AgentKind::Grok, SessionLane::GrokApiKey { .. })
        );
    if !sidecar.is_enabled() {
        // The Halv Engine is off: no engine starts, Kimi's isolated engine
        // included. A lane that used the shared engine still gets a direct
        // context, so the nested Codex home keeps measuring JEV workers.
        return Ok(requires_shared_proxy.then(SessionProxyContext::direct));
    }
    if let Some(upstream) = adapter_for(agent_kind).isolated_proxy_upstream(lane) {
        let isolated = sidecar
            .start_isolated_upstream(SidecarUpstream::OpenAi(upstream.to_string()))
            .await?;
        let port = isolated.port();
        return Ok(Some(SessionProxyContext {
            sidecar: Some(isolated),
            port: Some(port),
            shutdown_on_exit: true,
            nested_codex_home: None,
        }));
    }
    if !requires_shared_proxy {
        return Ok(None);
    }
    let port = shared_proxy_port_from_status(agent_kind, sidecar.ensure_ready().await)?;
    Ok(Some(SessionProxyContext {
        sidecar: Some(sidecar.clone()),
        port: Some(port),
        shutdown_on_exit: false,
        nested_codex_home: None,
    }))
}

#[cfg(test)]
fn kimi_isolated_upstream(lane: &SessionLane) -> Option<&'static str> {
    adapter_for(AgentKind::Kimi).isolated_proxy_upstream(lane)
}

fn shared_proxy_port_from_status(
    agent_kind: AgentKind,
    status: SidecarStatus,
) -> AgentSessionResult<u16> {
    if let Some(port) = halv_managed_listener_port(&status) {
        return Ok(port);
    }

    match status {
        SidecarStatus::Running { port, .. } => Err(format!(
            "refusing to route {} credentials through an unmanaged listener on port {port}",
            agent_kind.display_name()
        )),
        SidecarStatus::NotInstalled { install_hint, .. } => Err(install_hint),
        SidecarStatus::Error { message, .. } => Err(message),
        SidecarStatus::Installing { .. } | SidecarStatus::Starting { .. } => {
            Err("Headroom is not ready for the proxied agent lane".to_string())
        }
        SidecarStatus::Off { .. } => Err(crate::sidecar::ENGINE_OFF_MESSAGE.to_string()),
    }
}

#[tauri::command]
pub fn agent_session_list(
    manager: tauri::State<'_, AgentSessionManager>,
) -> AgentSessionResult<Vec<AgentSession>> {
    Ok(manager.list())
}

#[tauri::command]
pub fn agent_session_rename(
    manager: tauri::State<'_, AgentSessionManager>,
    storage: tauri::State<'_, Storage>,
    session_id: String,
    title: Option<String>,
) -> AgentSessionResult<AgentSession> {
    manager.rename(&storage, &session_id, title)
}

#[tauri::command]
pub async fn agent_session_send(
    app: AppHandle,
    manager: tauri::State<'_, AgentSessionManager>,
    storage: tauri::State<'_, Storage>,
    session_id: String,
    text: String,
) -> AgentSessionResult<()> {
    manager.set_generated_title(&storage, &session_id, &text)?;
    let conversation_id = manager.snapshot(&session_id)?.conversation_id;
    manager.send_user_message(&app, &session_id, &text).await?;

    if let Some(conversation_id) = conversation_id {
        if let Err(error) = storage.touch_conversation(&conversation_id) {
            eprintln!("failed to update conversation activity: {error}");
        }
        if let Err(error) = terminal::emit_conversation_activity(&app, &conversation_id) {
            eprintln!("failed to emit conversation activity: {error}");
        }
    }

    Ok(())
}

#[tauri::command]
pub fn agent_session_respond_permission(
    app: AppHandle,
    manager: tauri::State<'_, AgentSessionManager>,
    session_id: String,
    request_id: String,
    approve: bool,
) -> AgentSessionResult<()> {
    manager.respond_permission(&app, &session_id, &request_id, approve)
}

// A kill closes the session terminal, which can wait seconds for the process
// to exit. The wait runs on a blocking thread, not on the app main thread.
#[tauri::command]
pub async fn agent_session_kill(
    app: AppHandle,
    manager: tauri::State<'_, AgentSessionManager>,
    terminals: tauri::State<'_, TerminalManager>,
    session_id: String,
) -> AgentSessionResult<AgentSession> {
    let manager = manager.inner().clone();
    let terminals = terminals.inner().clone();
    tauri::async_runtime::spawn_blocking(move || manager.kill(&app, &terminals, &session_id))
        .await
        .map_err(|error| format!("failed to stop the agent session: {error}"))?
}

#[tauri::command]
pub fn agent_session_forget(
    manager: tauri::State<'_, AgentSessionManager>,
    session_id: String,
) -> AgentSessionResult<()> {
    manager.forget(&session_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FakeAdapter;
    static FAKE_ADAPTER: FakeAdapter = FakeAdapter;

    #[test]
    fn embedded_terminal_close_classifies_live_and_missing_sessions() {
        assert!(agent_session_is_live("starting"));
        assert!(agent_session_is_live("running"));
        assert!(agent_session_is_live("waiting_input"));
        assert!(!agent_session_is_live("exited"));
        assert!(!agent_session_is_live("failed"));
        assert!(terminal_session_is_missing(
            "terminal session `missing` was not found",
            "missing"
        ));
        assert!(!terminal_session_is_missing(
            "terminal session `other` was not found",
            "missing"
        ));
    }

    impl AgentAdapter for FakeAdapter {
        fn kind(&self) -> AgentKind {
            AgentKind::Codex
        }

        fn probe(&self) -> AgentProbe {
            AgentProbe {
                found: true,
                binary: Some("in-memory".to_string()),
                version: Some("test".to_string()),
                error: None,
            }
        }

        fn permission_flags(&self, _mode: CanonicalMode) -> AgentSessionResult<PermissionFlags> {
            Ok(PermissionFlags::empty())
        }

        fn login_launch_spec(&self) -> AgentSessionResult<LaunchSpec> {
            Err("the fake adapter does not launch a login process".to_string())
        }

        fn interactive_launch_spec(
            &self,
            _ctx: &InteractiveLaunchContext<'_>,
        ) -> AgentSessionResult<LaunchSpec> {
            Err("the fake adapter does not launch an interactive process".to_string())
        }

        fn start(
            &self,
            _ctx: &LaunchContext<'_>,
            _state: &mut AdapterState,
        ) -> AgentSessionResult<AdapterProcess> {
            Ok(AdapterProcess::in_memory())
        }

        fn new_state(
            &self,
            resumed: bool,
            permission_mode: CanonicalMode,
            started_at: Instant,
            _initializing: bool,
        ) -> AdapterState {
            codex::CODEX_ADAPTER.new_state(resumed, permission_mode, started_at, false)
        }

        fn parse_line(&self, _state: &mut AdapterState, line: &str) -> ParsedTurn {
            match line {
                "permission" => {
                    let request = adapter::PendingPermission {
                        request_id: "approval-7".to_string(),
                        rpc_id: serde_json::json!(7),
                        method: "item/commandExecution/requestApproval".to_string(),
                        params: serde_json::json!({ "command": "cargo test" }),
                    };
                    ParsedTurn {
                        events: vec![AgentEvent::PermissionRequest {
                            request_id: request.request_id.clone(),
                            description: "Run cargo test".to_string(),
                            payload_json: request.params.to_string(),
                        }],
                        pending_permissions: vec![request],
                        responses: Vec::new(),
                    }
                }
                "turn" => ParsedTurn {
                    events: vec![
                        AgentEvent::TextDelta {
                            text: "done".to_string(),
                        },
                        AgentEvent::TurnCompleted {
                            usage: TurnUsage {
                                input_tokens: 11,
                                output_tokens: 7,
                                cache_read_tokens: 3,
                                cache_creation_tokens: 2,
                                total_cost_usd: Some(0.04),
                            },
                        },
                    ],
                    ..ParsedTurn::default()
                },
                _ => ParsedTurn::default(),
            }
        }

        fn encode_user_message(
            &self,
            _state: &mut AdapterState,
            text: &str,
            _turn: TurnKind<'_>,
        ) -> AgentSessionResult<Option<String>> {
            Ok(Some(format!("fake:{text}")))
        }
    }

    #[derive(Clone, Copy, PartialEq, Eq)]
    enum RecordedLaunchFailure {
        ReserveSlot,
        Spawn,
        TagConversation,
    }

    #[derive(Clone, Copy)]
    struct RecordedLaunchPath {
        apply_title_state: bool,
        close_surface_on_tag_failure: bool,
    }

    impl RecordedLaunchPath {
        fn run(self, failure: Option<RecordedLaunchFailure>) -> Vec<&'static str> {
            let mut steps = vec![
                "credentials",
                "proxy_context",
                "nested_codex_home",
                "launch_spec",
                "project_name",
                "reserve_slot",
            ];
            if failure == Some(RecordedLaunchFailure::ReserveSlot) {
                steps.push("shutdown_proxy");
                return steps;
            }
            steps.push("spawn");
            if failure == Some(RecordedLaunchFailure::Spawn) {
                steps.extend(["shutdown_proxy", "release_slot"]);
                return steps;
            }
            steps.push("register");
            if self.apply_title_state {
                steps.push("title_state");
            }
            steps.push("tag_conversation");
            if failure == Some(RecordedLaunchFailure::TagConversation) {
                if self.close_surface_on_tag_failure {
                    steps.push("close_surface");
                }
                return steps;
            }
            steps.push("lifecycle");
            steps
        }
    }

    fn assert_recorded_launch_path(path: RecordedLaunchPath, expected: &[&str]) {
        assert_eq!(path.run(None), expected);
        assert_eq!(
            path.run(Some(RecordedLaunchFailure::ReserveSlot)),
            [
                "credentials",
                "proxy_context",
                "nested_codex_home",
                "launch_spec",
                "project_name",
                "reserve_slot",
                "shutdown_proxy",
            ]
        );
        assert_eq!(
            path.run(Some(RecordedLaunchFailure::Spawn)),
            [
                "credentials",
                "proxy_context",
                "nested_codex_home",
                "launch_spec",
                "project_name",
                "reserve_slot",
                "spawn",
                "shutdown_proxy",
                "release_slot",
            ]
        );
    }

    #[test]
    fn terminal_create_records_launch_sequence_and_unwind() {
        let path = RecordedLaunchPath {
            apply_title_state: true,
            close_surface_on_tag_failure: false,
        };
        assert_recorded_launch_path(
            path,
            &[
                "credentials",
                "proxy_context",
                "nested_codex_home",
                "launch_spec",
                "project_name",
                "reserve_slot",
                "spawn",
                "register",
                "title_state",
                "tag_conversation",
                "lifecycle",
            ],
        );
        assert_eq!(
            path.run(Some(RecordedLaunchFailure::TagConversation)),
            [
                "credentials",
                "proxy_context",
                "nested_codex_home",
                "launch_spec",
                "project_name",
                "reserve_slot",
                "spawn",
                "register",
                "title_state",
                "tag_conversation",
            ]
        );
    }

    #[test]
    fn terminal_duplicate_records_launch_sequence_and_unwind() {
        let path = RecordedLaunchPath {
            apply_title_state: false,
            close_surface_on_tag_failure: true,
        };
        assert_recorded_launch_path(
            path,
            &[
                "credentials",
                "proxy_context",
                "nested_codex_home",
                "launch_spec",
                "project_name",
                "reserve_slot",
                "spawn",
                "register",
                "tag_conversation",
                "lifecycle",
            ],
        );
        assert_eq!(
            path.run(Some(RecordedLaunchFailure::TagConversation)),
            [
                "credentials",
                "proxy_context",
                "nested_codex_home",
                "launch_spec",
                "project_name",
                "reserve_slot",
                "spawn",
                "register",
                "tag_conversation",
                "close_surface",
            ]
        );
    }

    #[test]
    fn ghostty_launch_records_launch_sequence_and_unwind() {
        let path = RecordedLaunchPath {
            apply_title_state: true,
            close_surface_on_tag_failure: false,
        };
        assert_recorded_launch_path(
            path,
            &[
                "credentials",
                "proxy_context",
                "nested_codex_home",
                "launch_spec",
                "project_name",
                "reserve_slot",
                "spawn",
                "register",
                "title_state",
                "tag_conversation",
                "lifecycle",
            ],
        );
        assert_eq!(
            path.run(Some(RecordedLaunchFailure::TagConversation)),
            [
                "credentials",
                "proxy_context",
                "nested_codex_home",
                "launch_spec",
                "project_name",
                "reserve_slot",
                "spawn",
                "register",
                "title_state",
                "tag_conversation",
            ]
        );
    }

    #[test]
    fn registry_drives_fake_adapter_events_permissions_and_settled_usage_in_memory() {
        let manager = AgentSessionManager::default();
        manager
            .register_in_memory_adapter("fake-session", &FAKE_ADAPTER)
            .expect("fake adapter should register");

        manager
            .drive_in_memory_adapter_line("fake-session", "permission")
            .expect("permission event should flow through the registry");
        manager
            .respond_in_memory_permission("fake-session", "approval-7", true)
            .expect("permission response should bridge through adapter state");
        manager
            .drive_in_memory_adapter_line("fake-session", "turn")
            .expect("turn events should flow through the registry");

        let sessions = manager
            .in_memory_sessions
            .lock()
            .expect("in-memory agent sessions lock poisoned");
        let session = sessions
            .get("fake-session")
            .expect("fake session should remain registered");
        assert_eq!(
            session.events,
            vec![
                AgentEvent::PermissionRequest {
                    request_id: "approval-7".to_string(),
                    description: "Run cargo test".to_string(),
                    payload_json: r#"{"command":"cargo test"}"#.to_string(),
                },
                AgentEvent::PermissionResolved {
                    request_id: "approval-7".to_string(),
                    approved: true,
                },
                AgentEvent::TextDelta {
                    text: "done".to_string(),
                },
                AgentEvent::TurnCompleted {
                    usage: TurnUsage {
                        input_tokens: 11,
                        output_tokens: 7,
                        cache_read_tokens: 3,
                        cache_creation_tokens: 2,
                        total_cost_usd: Some(0.04),
                    },
                },
            ]
        );
        assert_eq!(
            session.responses,
            [r#"{"id":7,"result":{"decision":"accept"}}"#.to_string()]
        );
        assert_eq!(
            session.settled_usage,
            Some(TurnUsage {
                input_tokens: 11,
                output_tokens: 7,
                cache_read_tokens: 3,
                cache_creation_tokens: 2,
                total_cost_usd: Some(0.04),
            })
        );
        assert!(session.state.pending_permissions_empty());
    }

    fn pending_permission(request_id: &str) -> adapter::PendingPermission {
        adapter::PendingPermission {
            request_id: request_id.to_string(),
            rpc_id: serde_json::json!(request_id),
            method: "item/commandExecution/requestApproval".to_string(),
            params: serde_json::json!({ "command": "git status" }),
        }
    }

    #[test]
    fn unknown_or_resolved_permission_request_id_errors() {
        let pending = HashMap::new();
        let error = find_pending_permission(&pending, "approval-1")
            .expect_err("an unknown request must fail");

        assert_eq!(
            error,
            "permission request `approval-1` is unknown or already resolved"
        );
    }

    #[test]
    fn child_exit_resolves_and_removes_pending_permissions() {
        let mut pending = HashMap::from([
            ("approval-2".to_string(), pending_permission("approval-2")),
            ("approval-1".to_string(), pending_permission("approval-1")),
        ]);

        let events = drain_pending_permission_events(&mut pending);

        assert!(pending.is_empty());
        assert_eq!(
            events,
            vec![
                AgentEvent::PermissionResolved {
                    request_id: "approval-1".to_string(),
                    approved: false,
                },
                AgentEvent::PermissionResolved {
                    request_id: "approval-2".to_string(),
                    approved: false,
                },
            ]
        );
    }

    #[test]
    fn memory_prefix_is_added_only_to_the_first_session_turn() {
        let memories_injected = AtomicBool::new(false);
        let original_text = "Implement the change.";
        let first = prefix_memories_for_first_turn(&memories_injected, original_text, || {
            Some("## Memories\n\n- Use pnpm.".to_string())
        });
        let second = prefix_memories_for_first_turn(&memories_injected, original_text, || {
            panic!("the second turn must not load memories")
        });

        assert_eq!(
            first,
            "<halv-memories>\n## Memories\n\n- Use pnpm.\n</halv-memories>\n\nImplement the change."
        );
        assert_eq!(second, original_text);
        assert!(memories_injected.load(Ordering::SeqCst));
    }

    #[test]
    fn memory_prefix_waits_until_memories_exist() {
        let memories_injected = AtomicBool::new(false);
        let original_text = "Implement the change.";
        let first = prefix_memories_for_first_turn(&memories_injected, original_text, || None);

        assert_eq!(first, original_text);
        assert!(!memories_injected.load(Ordering::SeqCst));

        let second = prefix_memories_for_first_turn(&memories_injected, original_text, || {
            Some("## Memories\n\n- Use pnpm.".to_string())
        });
        let third = prefix_memories_for_first_turn(&memories_injected, original_text, || {
            panic!("the third turn must not load memories")
        });

        assert_eq!(
            second,
            "<halv-memories>\n## Memories\n\n- Use pnpm.\n</halv-memories>\n\nImplement the change."
        );
        assert_eq!(third, original_text);
    }

    #[test]
    fn latest_user_message_for_memory_ignores_errors_and_returns_the_newest_user() {
        let path = std::env::temp_dir().join(format!(
            "halv-agent-memory-user-message-{}.sqlite3",
            new_uuid_v4()
        ));
        let storage = Storage::open(&path).expect("test storage should open");
        let conversation_id = storage
            .ensure_conversation(None, "Memory test", "gpt-5.5", None)
            .expect("conversation should create");
        for (content, error) in [
            ("Older successful user message", false),
            ("Newest successful user message", false),
            ("Errored user message", true),
        ] {
            storage
                .insert_message(NewMessage {
                    conversation_id: &conversation_id,
                    role: "user",
                    content,
                    model: Some("gpt-5.5"),
                    input_tokens: None,
                    output_tokens: None,
                    error,
                    mentions: None,
                })
                .expect("user message should insert");
        }

        assert_eq!(
            latest_user_message_for_memory(&storage, &conversation_id)
                .expect("latest user message should load"),
            "Newest successful user message"
        );
        drop(storage);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn protocol_writer_reports_a_full_queue_instead_of_blocking() {
        // The writer blocks until the test drops `unblock`.
        struct BlockedWriter(std::sync::mpsc::Receiver<()>);
        impl Write for BlockedWriter {
            fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
                let _ = self.0.recv();
                Ok(buffer.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }

        let (unblock, blocked) = std::sync::mpsc::channel();
        let writer =
            ProtocolWriter::spawn_with(BlockedWriter(blocked), 1, Duration::from_millis(50));
        writer
            .enqueue_line("first")
            .expect("the writer thread takes the first line");
        writer
            .enqueue_line("second")
            .expect("the queue holds one waiting line");
        let error = writer
            .enqueue_line("third")
            .expect_err("a full queue must fail instead of a hang");
        assert!(
            error.contains("failed to write agent protocol message"),
            "{error}"
        );
        drop(unblock);
    }

    #[test]
    fn protocol_writer_reports_the_stdin_failure_after_the_thread_stops() {
        struct FailingWriter;
        impl Write for FailingWriter {
            fn write(&mut self, _buffer: &[u8]) -> std::io::Result<usize> {
                Err(std::io::Error::new(
                    std::io::ErrorKind::BrokenPipe,
                    "stdin gone",
                ))
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }

        let writer = ProtocolWriter::spawn_with(FailingWriter, 4, Duration::from_millis(50));
        let deadline = Instant::now() + Duration::from_secs(5);
        let error = loop {
            match writer.enqueue_line("line") {
                Ok(()) => {
                    assert!(Instant::now() < deadline, "the writer thread must stop");
                    thread::sleep(Duration::from_millis(5));
                }
                Err(error) => break error,
            }
        };
        assert!(error.contains("stdin gone"), "{error}");
    }

    fn flag_text(values: &[OsString]) -> Vec<String> {
        values
            .iter()
            .map(|value| value.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn agent_session_forget_rejects_running_and_removes_exited_record() {
        let mut sessions = HashMap::from([
            ("running".to_string(), "running".to_string()),
            ("exited".to_string(), "exited".to_string()),
        ]);

        let error = remove_stopped_agent_session(&mut sessions, "running", Clone::clone)
            .expect_err("a running session must remain registered");
        assert_eq!(error, "agent session `running` is running");
        assert!(sessions.contains_key("running"));

        remove_stopped_agent_session(&mut sessions, "exited", Clone::clone)
            .expect("an exited session can be forgotten");
        assert!(!sessions.contains_key("exited"));
    }

    #[test]
    fn rehydrated_agent_record_restores_the_persisted_session() {
        let row = AgentSessionRow {
            id: "adopted-session".to_string(),
            agent_kind: "grok".to_string(),
            lane: "grok_api_key".to_string(),
            mode: "terminal_embedded".to_string(),
            ponytail_mode: Some("ultra".to_string()),
            project_id: Some(41),
            conversation_id: Some("conversation-9".to_string()),
            cwd: "/tmp/adopted".to_string(),
            cli_session_id: Some("cli-session-12".to_string()),
            custom_title: Some("Custom".to_string()),
            default_title: "Grok 3".to_string(),
            generated_title: Some("Generated".to_string()),
            ai_vault_title: Some("Vault".to_string()),
            ai_vault_session_id: Some("vault-session-2".to_string()),
            generation: 8,
            status: "running".to_string(),
            created_at: "2026-08-24T10:00:00Z".to_string(),
            last_active_at: "2026-08-24T10:05:00Z".to_string(),
            exit_code: None,
            rtk_db_path: None,
            crux_ledger_path: None,
            headroom_route: None,
        };
        let terminal = TerminalSession {
            id: row.id.clone(),
            title: Some("Adopted".to_string()),
            project_id: Some(99),
            conversation_id: None,
            project_name: "Project".to_string(),
            cwd: "/tmp/terminal-metadata".to_string(),
            shell: "zsh".to_string(),
            pid: 4242,
            execution_profile: "full_access".to_string(),
            agent_kind: Some("grok".to_string()),
            status: "running".to_string(),
            cols: 100,
            rows: 24,
            started_at: "2026-08-24T10:00:00Z".to_string(),
            exited_at: None,
            exit_code: None,
            signal: None,
            continued_from: None,
            shell_tab_id: None,
        };

        let savings_paths = SessionSavingsPaths::from_paths(
            &row.id,
            row.rtk_db_path.as_deref(),
            row.crux_ledger_path.as_deref(),
        );
        let (record, lane, route) =
            rehydrated_agent_session_record(&row, &terminal, false, &savings_paths)
                .expect("adopted session should rehydrate");
        // A row from 1.0.0 has no saved route, so the lane tells it.
        assert_eq!(route, HeadroomRoute::Engine);

        assert_eq!(record.id, row.id);
        assert_eq!(record.agent_kind, AgentKind::Grok);
        assert_eq!(record.lane_kind, SessionLaneKind::GrokApiKey);
        assert_eq!(record.mode, SessionMode::TerminalEmbedded);
        assert_eq!(record.ponytail_mode, Some(PonytailMode::Ultra));
        assert_eq!(record.project_id, Some(41));
        assert_eq!(record.conversation_id.as_deref(), Some("conversation-9"));
        assert_eq!(record.cwd, "/tmp/adopted");
        assert_eq!(record.pid, 4242);
        assert_eq!(record.cli_session_id.as_deref(), Some("cli-session-12"));
        assert_eq!(record.custom_title.as_deref(), Some("Custom"));
        assert_eq!(record.default_title, "Grok 3");
        assert_eq!(record.generated_title.as_deref(), Some("Generated"));
        assert_eq!(record.generation, 8);
        assert!(record.savings_sources.proxy);
        assert_eq!(
            record.ai_vault_title,
            Some(AiVaultSessionTitle {
                session_id: "vault-session-2".to_string(),
                title: "Vault".to_string(),
            })
        );
        assert!(matches!(&lane, SessionLane::GrokApiKey { .. }));

        let managed = build_rehydrated_managed_session(
            record,
            lane,
            savings_paths,
            Arc::new(AtomicUsize::new(0)),
            Some(SidecarManager::new(8787)),
        );
        assert!(managed.savings_meter.is_some());
        assert!(managed.terminal_savings.is_some());
        assert_eq!(
            managed.rtk_source_path,
            exec::rtk_session_database_path(&row.id)
        );
        assert_eq!(
            managed.crux_source_path,
            exec::crux_session_ledger_path(&row.id)
        );
    }

    #[test]
    fn legacy_rows_read_the_route_from_the_lane() {
        use HeadroomRoute::{Direct, Engine, Isolated, Workers};
        for (agent_kind, lane_kind, subscription, expected) in [
            (
                AgentKind::Claude,
                SessionLaneKind::ClaudeSubscription,
                true,
                Engine,
            ),
            // The opt-in was off, but the nested Codex home still routed the
            // JEV workers through the engine.
            (
                AgentKind::Claude,
                SessionLaneKind::ClaudeSubscription,
                false,
                Workers,
            ),
            (
                AgentKind::Claude,
                SessionLaneKind::ClaudeApiKey,
                false,
                Engine,
            ),
            (AgentKind::Zai, SessionLaneKind::ZaiGlm, false, Engine),
            (AgentKind::Claude, SessionLaneKind::Custom, false, Engine),
            (AgentKind::Grok, SessionLaneKind::GrokApiKey, false, Engine),
            (
                AgentKind::Codex,
                SessionLaneKind::CodexChatGpt,
                false,
                Engine,
            ),
            (
                AgentKind::Kimi,
                SessionLaneKind::KimiApiKey,
                false,
                Isolated,
            ),
            (
                AgentKind::Kimi,
                SessionLaneKind::KimiSubscription,
                false,
                Isolated,
            ),
            // These lanes had no engine context in 1.0.0.
            (AgentKind::Grok, SessionLaneKind::Custom, false, Direct),
            (AgentKind::Codex, SessionLaneKind::Custom, false, Direct),
        ] {
            assert_eq!(
                rehydrated_headroom_route(None, agent_kind, lane_kind, subscription),
                expected,
                "{agent_kind:?} {lane_kind:?} {subscription}"
            );
        }
    }

    #[test]
    fn a_saved_route_wins_over_the_lane_rule() {
        let route = |stored| {
            rehydrated_headroom_route(
                stored,
                AgentKind::Claude,
                SessionLaneKind::ClaudeApiKey,
                true,
            )
        };
        // A session launched with the engine off went direct.
        assert_eq!(route(Some("direct")), HeadroomRoute::Direct);
        assert_eq!(route(Some("workers")), HeadroomRoute::Workers);
        assert_eq!(route(Some("engine")), HeadroomRoute::Engine);
        // A damaged value falls back to the lane rule.
        assert_eq!(route(Some("bogus")), HeadroomRoute::Engine);
        for route in [
            HeadroomRoute::Direct,
            HeadroomRoute::Engine,
            HeadroomRoute::Workers,
            HeadroomRoute::Isolated,
        ] {
            assert_eq!(
                HeadroomRoute::from_storage_name(route.storage_name()),
                Some(route)
            );
        }
    }

    #[test]
    fn only_shared_engine_routes_need_the_engine_after_a_restart() {
        assert!(HeadroomRoute::Engine.needs_shared_engine());
        assert!(HeadroomRoute::Workers.needs_shared_engine());
        // Kimi's isolated engine stops with the app and its port is not
        // saved, so the shared engine cannot serve it.
        assert!(!HeadroomRoute::Isolated.needs_shared_engine());
        assert!(!HeadroomRoute::Direct.needs_shared_engine());

        assert!(HeadroomRoute::Engine.proxy_source());
        assert!(HeadroomRoute::Isolated.proxy_source());
        assert!(!HeadroomRoute::Workers.proxy_source());
        assert!(!HeadroomRoute::Direct.proxy_source());
    }

    #[test]
    fn a_launch_saves_the_route_of_its_context() {
        let meter = |shutdown_on_exit| SessionSavingsMeter {
            sidecar: SidecarManager::new(4330),
            project: "session-3".to_string(),
            before: Mutex::new(None),
            shutdown_on_exit,
        };
        assert_eq!(HeadroomRoute::of_launch(None, false), HeadroomRoute::Direct);
        assert_eq!(
            HeadroomRoute::of_launch(Some(&meter(false)), true),
            HeadroomRoute::Engine
        );
        assert_eq!(
            HeadroomRoute::of_launch(Some(&meter(false)), false),
            HeadroomRoute::Workers
        );
        assert_eq!(
            HeadroomRoute::of_launch(Some(&meter(true)), true),
            HeadroomRoute::Isolated
        );
        // An engine-off context has no meter: the session went direct.
        assert!(SessionProxyContext::direct()
            .savings_meter("session-3")
            .is_none());
    }

    #[test]
    fn permission_mapping_covers_each_agent_and_halv_policy() {
        let modes = [
            "read_only",
            "ask",
            "on_request",
            "on_failure",
            "never",
            "skip_all",
        ];
        let claude_args = [
            vec!["--permission-mode", "plan"],
            vec!["--permission-mode", "default"],
            vec!["--permission-mode", "auto"],
            vec!["--permission-mode", "acceptEdits"],
            vec!["--permission-mode", "dontAsk"],
            vec!["--dangerously-skip-permissions"],
        ];
        let claude_compatible_args = [
            vec!["--permission-mode", "plan"],
            vec!["--permission-mode", "default"],
            vec!["--permission-mode", "default"],
            vec!["--permission-mode", "acceptEdits"],
            vec!["--permission-mode", "dontAsk"],
            vec!["--dangerously-skip-permissions"],
        ];
        let codex_args = [
            vec!["--sandbox", "read-only", "--ask-for-approval", "never"],
            vec![
                "--sandbox",
                "workspace-write",
                "--ask-for-approval",
                "on-request",
            ],
            vec![
                "--sandbox",
                "workspace-write",
                "--ask-for-approval",
                "on-request",
                "-c",
                "approvals_reviewer=\"auto_review\"",
            ],
            vec![
                "-c",
                "approval_policy=\"on-failure\"",
                "-c",
                "sandbox_mode=\"workspace-write\"",
            ],
            vec![
                "--sandbox",
                "workspace-write",
                "--ask-for-approval",
                "never",
            ],
            vec!["--yolo"],
        ];
        let codex_settings = [
            ("never", "read-only"),
            ("on-request", "workspace-write"),
            ("on-request", "workspace-write"),
            ("on-failure", "workspace-write"),
            ("never", "workspace-write"),
            ("never", "danger-full-access"),
        ];

        for (index, mode) in modes.iter().enumerate() {
            let claude = permission_flags(AgentKind::Claude, mode)
                .expect("Claude permission mode should map");
            assert_eq!(flag_text(&claude.cli_args()), claude_args[index]);
            assert!(claude.config_overrides.is_empty());

            for agent_kind in [AgentKind::Grok, AgentKind::Zai] {
                let flags = permission_flags(agent_kind, mode)
                    .expect("Claude-compatible permission mode should map");
                assert_eq!(flag_text(&flags.cli_args()), claude_compatible_args[index]);
                assert!(flags.config_overrides.is_empty());
            }

            let codex = permission_flags(AgentKind::Codex, mode).expect("Codex mode should map");
            assert_eq!(flag_text(&codex.cli_args()), codex_args[index]);
            let settings = codex
                .codex_settings
                .expect("Codex mode should include app-server settings");
            assert_eq!(
                (settings.approval_policy, settings.sandbox),
                codex_settings[index]
            );
            match *mode {
                "on_request" => assert_eq!(
                    flag_text(&codex.config_overrides),
                    ["approvals_reviewer=\"auto_review\""]
                ),
                "on_failure" => assert_eq!(
                    flag_text(&codex.config_overrides),
                    [
                        "approval_policy=\"on-failure\"",
                        "sandbox_mode=\"workspace-write\"",
                    ]
                ),
                _ => assert!(codex.config_overrides.is_empty()),
            }

            let kimi = permission_flags(AgentKind::Kimi, mode).expect("Kimi mode should map");
            let expected = if *mode == "skip_all" {
                vec!["--yolo"]
            } else {
                Vec::new()
            };
            assert_eq!(flag_text(&kimi.cli_args()), expected);
            assert!(kimi.config_overrides.is_empty());
        }

        assert!(permission_flags(AgentKind::Claude, "unexpected").is_err());
    }

    #[test]
    fn initial_prompt_arg_rides_only_on_fresh_supported_launches() {
        assert_eq!(
            initial_prompt_arg(AgentKind::Claude, None, Some(" oi ")),
            Some(OsString::from("oi"))
        );
        assert_eq!(
            initial_prompt_arg(AgentKind::Codex, None, Some("oi")),
            Some(OsString::from("oi"))
        );
        assert_eq!(
            initial_prompt_arg(AgentKind::Zai, None, Some("oi")),
            Some(OsString::from("oi"))
        );
        assert_eq!(
            initial_prompt_arg(AgentKind::Grok, None, Some("oi")),
            Some(OsString::from("oi"))
        );
        // A resumed CLI already holds its history; no prompt argument.
        assert_eq!(
            initial_prompt_arg(AgentKind::Claude, Some("cli-id"), Some("oi")),
            None
        );
        // Kimi and Antigravity keep typed delivery.
        assert_eq!(initial_prompt_arg(AgentKind::Kimi, None, Some("oi")), None);
        assert_eq!(
            initial_prompt_arg(AgentKind::Antigravity, None, Some("oi")),
            None
        );
        assert_eq!(
            initial_prompt_arg(AgentKind::Gemini, None, Some("oi")),
            Some(OsString::from("oi"))
        );
        // Blank prompts never become arguments.
        assert_eq!(
            initial_prompt_arg(AgentKind::Claude, None, Some("   ")),
            None
        );
        assert_eq!(initial_prompt_arg(AgentKind::Claude, None, None), None);
    }

    #[test]
    fn custom_title_normalization_trims_and_resets() {
        assert_eq!(
            normalize_title(Some("  Release checks  ".to_string())).as_deref(),
            Some("Release checks")
        );
        assert_eq!(normalize_title(Some("   ".to_string())), None);
        assert_eq!(normalize_title(None), None);
    }

    #[test]
    fn session_shutdown_generation_increments_and_saturates() {
        assert_eq!(next_session_generation(0), 1);
        assert_eq!(next_session_generation(7), 8);
        assert_eq!(next_session_generation(u64::MAX), u64::MAX);
    }

    #[test]
    fn codex_messages_are_queued_then_flushed_in_order() {
        let mut init = CodexInitState::new(Instant::now());
        for message in ["first", "second", "third"] {
            assert_eq!(
                init.queue_message(message)
                    .expect("message should enter initialization queue"),
                CodexMessageDisposition::Queued
            );
        }

        assert!(init.begin_flush());
        let mut flushed = Vec::new();
        while let Some(message) = init.next_queued_message() {
            flushed.push(message);
        }

        assert_eq!(flushed, ["first", "second", "third"]);
        assert_eq!(
            init.queue_message("after init")
                .expect("initialized message should be ready"),
            CodexMessageDisposition::Ready
        );
    }

    #[test]
    fn codex_no_init_timeout_reports_protocol_error_and_stderr() {
        let started_at = Instant::now();
        let mut init = CodexInitState::new(started_at);
        init.queue_message("queued prompt")
            .expect("message should enter initialization queue");
        init.remember_protocol_error("thread/start rejected the request");

        let timeout = init
            .timeout(started_at + CODEX_INIT_TIMEOUT + Duration::from_millis(1))
            .expect("synthetic no-init state should time out");
        let message = codex_init_timeout_message(
            &timeout,
            "startup detail\nfatal app-server initialization failure",
        );

        assert_eq!(timeout.queued_messages, 1);
        assert!(message.contains("did not initialize its thread within 30 seconds"));
        assert!(message.contains("Last protocol error: thread/start rejected the request"));
        assert!(message.contains("App-server stderr tail:"));
        assert!(message.contains("fatal app-server initialization failure"));
        assert!(init.timeout(Instant::now() + CODEX_INIT_TIMEOUT).is_none());
    }

    #[test]
    fn stderr_tail_capture_is_bounded() {
        let mut tail = StderrTail::new(32);
        for line_number in 0..10 {
            tail.push_line(&format!("stderr-line-{line_number}"));
        }
        let output = tail.snapshot();

        assert!(output.len() <= 32);
        assert!(output.contains("stderr-line-9"));
        assert!(!output.contains("stderr-line-0"));
    }

    fn running_sidecar_status(
        spawned_by_us: bool,
        managed_by_halv: bool,
        port: u16,
    ) -> SidecarStatus {
        SidecarStatus::Running {
            spawned_by_us,
            managed_by_halv,
            optimization_paused: false,
            version: Some("v-test".to_string()),
            pinned_version: "v-pin".to_string(),
            port,
        }
    }

    #[test]
    fn shared_proxy_accepts_listener_spawned_this_run() {
        let port = shared_proxy_port_from_status(
            AgentKind::Codex,
            running_sidecar_status(true, false, 4201),
        )
        .expect("this process spawned the listener");

        assert_eq!(port, 4201);
    }

    #[test]
    fn shared_proxy_accepts_marker_detected_adopted_listener() {
        let port = shared_proxy_port_from_status(
            AgentKind::Codex,
            running_sidecar_status(false, true, 4202),
        )
        .expect("Halv detected the adopted listener");

        assert_eq!(port, 4202);
    }

    #[test]
    fn shared_proxy_refuses_foreign_listener() {
        let error = shared_proxy_port_from_status(
            AgentKind::Codex,
            running_sidecar_status(false, false, 4203),
        )
        .expect_err("agent credentials must never use a foreign listener");

        assert!(error.contains("unmanaged listener"));
    }

    fn spec_args(spec: &LaunchSpec) -> Vec<String> {
        spec.args
            .iter()
            .map(|argument| argument.to_string_lossy().into_owned())
            .collect()
    }

    fn spec_env<'a>(spec: &'a LaunchSpec, name: &str) -> Option<&'a str> {
        spec.env
            .iter()
            .find_map(|(key, value)| (key == name).then(|| value.to_str()).flatten())
    }

    fn spec_env_value(environment: &[(OsString, OsString)], name: &str) -> Option<String> {
        environment
            .iter()
            .find_map(|(key, value)| (key == name).then(|| value.to_string_lossy().into_owned()))
    }

    fn test_profile(id: &str, provider: &str, status: &str) -> CredentialProfile {
        CredentialProfile {
            id: id.to_string(),
            provider: provider.to_string(),
            name: format!("{provider} test"),
            kind: "api_key".to_string(),
            priority: 0,
            status: status.to_string(),
            last_validated_at: None,
            metadata_json: None,
            created_at: "2000-01-01T00:00:00Z".to_string(),
            updated_at: "2000-01-01T00:00:00Z".to_string(),
        }
    }

    #[test]
    fn lane_options_serialize_in_agent_and_lane_order() {
        let profiles = vec![
            test_profile("anthropic-active", "anthropic", "active"),
            test_profile("anthropic-disabled", "anthropic", "disabled"),
            test_profile("moonshot-active", "moonshot", "active"),
            test_profile("zai-active", "zai", "active"),
            test_profile("xai-active", "xai", "active"),
        ];
        let options = build_agent_lane_options(
            &profiles,
            AgentLoginPresence {
                antigravity: Some(false),
                claude: Some(true),
                codex: Some(false),
                gemini: Some(true),
                kimi: None,
            },
        );
        let value = serde_json::to_value(options).expect("lane options should serialize");

        assert_eq!(
            value
                .as_array()
                .expect("lane options should be an array")
                .iter()
                .map(|item| item["agentKind"].as_str().unwrap_or_default())
                .collect::<Vec<_>>(),
            [
                "gemini",
                "antigravity",
                "claude",
                "codex",
                "kimi",
                "zai",
                "grok",
            ]
        );
        assert_eq!(value[0]["lanes"][0]["kind"], "custom");
        assert_eq!(value[0]["lanes"][0]["loggedIn"], true);
        assert_eq!(value[1]["lanes"][0]["kind"], "custom");
        assert_eq!(value[1]["lanes"][0]["loggedIn"], false);
        assert_eq!(
            value[2]["lanes"]
                .as_array()
                .expect("Claude lanes should be an array")
                .iter()
                .map(|lane| lane["kind"].as_str().unwrap_or_default())
                .collect::<Vec<_>>(),
            ["claudeSubscription", "claudeApiKey", "custom"]
        );
        assert_eq!(value[2]["lanes"][0]["loggedIn"], true);
        assert_eq!(value[2]["lanes"][1]["loggedIn"], serde_json::Value::Null);
        assert_eq!(
            value[2]["lanes"][1]["credentialProfiles"][0]["id"],
            "anthropic-active"
        );
        assert_eq!(
            value[2]["lanes"][1]["credentialProfiles"]
                .as_array()
                .map(Vec::len),
            Some(1)
        );
        assert_eq!(
            value[3]["lanes"]
                .as_array()
                .expect("Codex lanes should be an array")
                .iter()
                .map(|lane| lane["kind"].as_str().unwrap_or_default())
                .collect::<Vec<_>>(),
            ["codexChatGpt", "custom"]
        );
        assert_eq!(value[3]["lanes"][0]["loggedIn"], false);
        assert_eq!(
            value[4]["lanes"]
                .as_array()
                .expect("Kimi lanes should be an array")
                .iter()
                .map(|lane| lane["kind"].as_str().unwrap_or_default())
                .collect::<Vec<_>>(),
            ["kimiSubscription", "kimiApiKey", "custom"]
        );
        assert_eq!(value[4]["lanes"][0]["loggedIn"], serde_json::Value::Null);
        assert_eq!(
            value[5]["lanes"]
                .as_array()
                .expect("Z.ai lanes should be an array")
                .iter()
                .map(|lane| lane["kind"].as_str().unwrap_or_default())
                .collect::<Vec<_>>(),
            ["zaiGlm", "custom"]
        );
        assert_eq!(
            value[6]["lanes"]
                .as_array()
                .expect("Grok lanes should be an array")
                .iter()
                .map(|lane| lane["kind"].as_str().unwrap_or_default())
                .collect::<Vec<_>>(),
            ["grokApiKey", "custom"]
        );
        assert_eq!(
            value[6]["lanes"][0]["credentialProfiles"][0]["id"],
            "xai-active"
        );
        assert_eq!(SessionLaneKind::GrokApiKey.storage_name(), "grok_api_key");
    }

    #[test]
    fn raw_and_profile_api_key_lanes_deserialize() {
        let raw = serde_json::from_value::<SessionLane>(serde_json::json!({
            "kind": "claudeApiKey",
            "apiKey": "legacy-secret"
        }))
        .expect("legacy raw-key lane should deserialize");
        assert!(matches!(
            raw,
            SessionLane::ClaudeApiKey {
                credential: SessionCredential::ApiKey { api_key },
                ..
            } if api_key == "legacy-secret"
        ));

        let profile = serde_json::from_value::<SessionLane>(serde_json::json!({
            "kind": "zaiGlm",
            "credentialProfileId": "zai-profile"
        }))
        .expect("profile lane should deserialize");
        assert!(matches!(
            profile,
            SessionLane::ZaiGlm {
                credential: SessionCredential::CredentialProfile { credential_profile_id },
                ..
            } if credential_profile_id == "zai-profile"
        ));

        let grok = serde_json::from_value::<SessionLane>(serde_json::json!({
            "kind": "grokApiKey",
            "credentialProfileId": "xai-profile",
            "modelOverrides": {
                "sonnet": "grok-sonnet-test"
            }
        }))
        .expect("Grok profile lane should deserialize");
        assert!(matches!(
            grok,
            SessionLane::GrokApiKey {
                credential: SessionCredential::CredentialProfile { credential_profile_id },
                model_overrides: Some(GrokModelOverrides { sonnet: Some(sonnet), .. }),
            } if credential_profile_id == "xai-profile" && sonnet == "grok-sonnet-test"
        ));
    }

    #[test]
    fn rehydrated_lanes_identify_api_key_providers() {
        for (kind, expected_provider) in [
            (SessionLaneKind::ClaudeSubscription, None),
            (SessionLaneKind::ClaudeApiKey, Some("anthropic")),
            (SessionLaneKind::CodexChatGpt, None),
            (SessionLaneKind::ZaiGlm, Some("zai")),
            (SessionLaneKind::GrokApiKey, Some("xai")),
            (SessionLaneKind::KimiApiKey, Some("moonshot")),
            (SessionLaneKind::KimiSubscription, None),
            (SessionLaneKind::Custom, None),
        ] {
            assert_eq!(
                rehydrated_lane_credential_provider(&kind.rehydrated_lane()),
                expected_provider
            );
        }
    }

    #[test]
    fn rehydrated_api_key_lane_resolves_the_default_profile() {
        let mut default_profile = test_profile("zai-default", "zai", "active");
        default_profile.priority = -5;
        let credential_profiles = vec![test_profile("zai-later", "zai", "active"), default_profile];
        let lane = resolve_rehydrated_lane_credentials_with(
            SessionLaneKind::ZaiGlm.rehydrated_lane(),
            &credential_profiles,
            |profile_id, provider| {
                assert_eq!(profile_id, "zai-default");
                assert_eq!(provider, "zai");
                Ok("refreshed-zai-secret".to_string())
            },
        )
        .expect("rehydrated Z.ai credentials should resolve");

        assert!(matches!(
            lane,
            SessionLane::ZaiGlm {
                credential: SessionCredential::ApiKey { api_key },
                ..
            } if api_key == "refreshed-zai-secret"
        ));
    }

    #[test]
    fn rehydrated_credential_free_lane_skips_profile_resolution() {
        let lane = resolve_rehydrated_lane_credentials_with(
            SessionLaneKind::Custom.rehydrated_lane(),
            &[],
            |_, _| panic!("custom lanes must not resolve credentials"),
        )
        .expect("rehydrated custom lane should remain usable");

        assert_eq!(lane, SessionLane::Custom { env: Vec::new() });
    }

    #[test]
    fn profile_lanes_resolve_secrets_into_adapter_environments() {
        let path = std::env::temp_dir().join(format!(
            "halv-agent-profile-env-{}.sqlite3",
            crate::storage::new_uuid_v4()
        ));
        let storage = Storage::open(&path).expect("profile test storage should open");
        for profile in [
            test_profile("anthropic-profile", "anthropic", "active"),
            test_profile("moonshot-profile", "moonshot", "active"),
            test_profile("zai-profile", "zai", "active"),
            test_profile("xai-profile", "xai", "active"),
        ] {
            storage
                .insert_credential_profile(&profile)
                .expect("profile should insert");
        }

        let resolve = |lane| {
            resolve_lane_credentials_with(lane, |profile_id, provider| {
                let (_, secret) = profiles::active_api_key_with(
                    &storage,
                    profile_id,
                    provider,
                    |service, account| {
                        assert_eq!(service, "Halv");
                        assert_eq!(account, format!("{provider}-profile-{profile_id}"));
                        Ok::<_, String>(Some(format!("{provider}-secret")))
                    },
                )?;
                Ok(secret)
            })
            .expect("profile lane should resolve")
        };

        let claude_lane = resolve(
            serde_json::from_value(serde_json::json!({
                "kind": "claudeApiKey",
                "credentialProfileId": "anthropic-profile"
            }))
            .expect("Claude profile lane should deserialize"),
        );
        let claude_spec = claude::interactive_launch_spec_with_program(
            PathBuf::from("/tmp/claude"),
            Path::new("/tmp"),
            &claude_lane,
            "default",
            None,
            None,
            Some(9123),
            "session-123",
            Path::new("/tmp/halv.sqlite3"),
            None,
            PonytailMode::Off,
            false,
            None,
        )
        .expect("Claude profile specification should build");
        assert_eq!(
            spec_env(&claude_spec, "ANTHROPIC_API_KEY"),
            Some("anthropic-secret")
        );
        assert_eq!(
            spec_env(&claude_spec, "ANTHROPIC_BASE_URL"),
            Some("http://127.0.0.1:9123/p/session-123")
        );
        assert_eq!(
            spec_env(&claude_spec, "ANTHROPIC_CUSTOM_HEADERS"),
            Some("X-Headroom-Base-Url: https://api.anthropic.com")
        );

        let kimi_lane = resolve(
            serde_json::from_value(serde_json::json!({
                "kind": "kimiApiKey",
                "credentialProfileId": "moonshot-profile"
            }))
            .expect("Kimi profile lane should deserialize"),
        );
        let kimi_spec = kimi::interactive_launch_spec_with_program(
            PathBuf::from("/tmp/kimi"),
            &kimi_lane,
            "ask",
            None,
            Some(9123),
            "session-123",
            None,
        )
        .expect("Kimi profile specification should build");
        assert_eq!(
            spec_env(&kimi_spec, "KIMI_MODEL_API_KEY"),
            Some("moonshot-secret")
        );

        let zai_lane = resolve(
            serde_json::from_value(serde_json::json!({
                "kind": "zaiGlm",
                "credentialProfileId": "zai-profile"
            }))
            .expect("Z.ai profile lane should deserialize"),
        );
        let zai_spec = claude::interactive_launch_spec_with_program(
            PathBuf::from("/tmp/claude"),
            Path::new("/tmp"),
            &zai_lane,
            "default",
            None,
            None,
            Some(9123),
            "session-123",
            Path::new("/tmp/halv.sqlite3"),
            None,
            PonytailMode::Off,
            false,
            None,
        )
        .expect("Z.ai profile specification should build");
        assert_eq!(
            spec_env(&zai_spec, "ANTHROPIC_AUTH_TOKEN"),
            Some("zai-secret")
        );

        let grok_lane = resolve(
            serde_json::from_value(serde_json::json!({
                "kind": "grokApiKey",
                "credentialProfileId": "xai-profile"
            }))
            .expect("Grok profile lane should deserialize"),
        );
        let grok_spec = claude::interactive_launch_spec_with_program(
            PathBuf::from("/tmp/claude"),
            Path::new("/tmp"),
            &grok_lane,
            "default",
            None,
            None,
            Some(9123),
            "session-123",
            Path::new("/tmp/halv.sqlite3"),
            None,
            PonytailMode::Off,
            false,
            None,
        )
        .expect("Grok profile specification should build");
        assert_eq!(
            spec_env(&grok_spec, "ANTHROPIC_AUTH_TOKEN"),
            Some("xai-secret")
        );
        assert_eq!(
            spec_env(&grok_spec, "ANTHROPIC_CUSTOM_HEADERS"),
            Some("X-Headroom-Base-Url: https://api.x.ai")
        );

        drop(storage);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn registry_enforces_session_cap() {
        let active_sessions = AtomicUsize::new(0);
        for _ in 0..MAX_AGENT_SESSIONS {
            reserve_session_slot(&active_sessions).expect("session slot should be available");
        }
        let error = reserve_session_slot(&active_sessions).expect_err("session cap should apply");
        assert!(error.contains(&format!("at most {MAX_AGENT_SESSIONS} agent sessions")));

        active_sessions.fetch_sub(1, Ordering::SeqCst);
        reserve_session_slot(&active_sessions).expect("released session slot should be available");
    }

    #[test]
    fn registry_enforces_session_cap_under_contention() {
        let active_sessions = AtomicUsize::new(0);
        let barrier = std::sync::Barrier::new(MAX_AGENT_SESSIONS * 2);
        let accepted = std::thread::scope(|scope| {
            let workers = (0..MAX_AGENT_SESSIONS * 2)
                .map(|_| {
                    scope.spawn(|| {
                        barrier.wait();
                        reserve_session_slot(&active_sessions).is_ok()
                    })
                })
                .collect::<Vec<_>>();
            workers
                .into_iter()
                .map(|worker| usize::from(worker.join().unwrap()))
                .sum::<usize>()
        });
        assert_eq!(accepted, MAX_AGENT_SESSIONS);
        assert_eq!(active_sessions.load(Ordering::SeqCst), MAX_AGENT_SESSIONS);
        assert!(reserve_session_slot(&AtomicUsize::new(usize::MAX)).is_err());
    }

    #[test]
    fn usage_deltas_add_nested_codex_counters() {
        let mut usage = turn_usage_delta(
            TurnUsage {
                input_tokens: 30,
                output_tokens: 12,
                cache_read_tokens: 20,
                cache_creation_tokens: 8,
                total_cost_usd: None,
            },
            TurnUsage {
                input_tokens: 10,
                output_tokens: 2,
                cache_read_tokens: 5,
                cache_creation_tokens: 3,
                total_cost_usd: None,
            },
        );
        add_usage_tokens(
            &mut usage,
            &TurnUsage {
                input_tokens: 400,
                output_tokens: 80,
                cache_read_tokens: 150,
                cache_creation_tokens: 25,
                total_cost_usd: None,
            },
        );
        assert_eq!(usage.input_tokens, 420);
        assert_eq!(usage.output_tokens, 90);
        assert_eq!(usage.cache_read_tokens, 165);
        assert_eq!(usage.cache_creation_tokens, 30);
    }

    #[test]
    fn agent_launches_carry_the_pane_variables_and_drop_orca_variables() {
        let spec = LaunchSpec {
            program: PathBuf::from("/bin/claude"),
            args: Vec::new(),
            env: vec![
                (
                    OsString::from("HALV_AGENT_SESSION_ID"),
                    OsString::from("stale"),
                ),
                (OsString::from("ORCA_PANE_KEY"), OsString::from("tab:leaf")),
                (OsString::from("PATH"), OsString::from("/bin")),
            ],
            env_remove: vec![OsString::from("ORCA_TAB_ID")],
        };
        let spec = add_agent_status_to_launch_spec(
            spec,
            vec![
                (
                    OsString::from("HALV_AGENT_SESSION_ID"),
                    OsString::from("session-123"),
                ),
                (
                    OsString::from("HALV_AGENT_STATUS_DIR"),
                    OsString::from("/data/agent-status/hooks"),
                ),
            ],
            vec![
                OsString::from("ORCA_PANE_KEY"),
                OsString::from("ORCA_TAB_ID"),
            ],
        );
        assert_eq!(
            spec.env,
            vec![
                (OsString::from("PATH"), OsString::from("/bin")),
                (
                    OsString::from("HALV_AGENT_SESSION_ID"),
                    OsString::from("session-123"),
                ),
                (
                    OsString::from("HALV_AGENT_STATUS_DIR"),
                    OsString::from("/data/agent-status/hooks"),
                ),
            ]
        );
        assert_eq!(
            spec.env_remove,
            vec![
                OsString::from("ORCA_TAB_ID"),
                OsString::from("ORCA_PANE_KEY")
            ]
        );
    }

    #[test]
    fn claude_interactive_spec_keeps_proxied_lane_environment() {
        let _guard = CLAUDE_CONFIG_DIR_TEST_LOCK
            .lock()
            .expect("Claude config test lock poisoned");
        let lane = SessionLane::ZaiGlm {
            credential: SessionCredential::ApiKey {
                api_key: "secret-value".to_string(),
            },
            model_overrides: None,
        };
        // --resume needs an existing Claude session file for this cwd.
        let config_dir = std::env::temp_dir().join(format!(
            "halv-claude-lane-config-{:016x}",
            std::process::id() as u64 ^ 0x1a4e
        ));
        let project_dir = config_dir.join("projects/-tmp");
        std::fs::create_dir_all(&project_dir).expect("test directory should exist");
        std::fs::write(project_dir.join("resume-123.jsonl"), "{}\n")
            .expect("session fixture should write");
        let previous = std::env::var_os("CLAUDE_CONFIG_DIR");
        std::env::set_var("CLAUDE_CONFIG_DIR", &config_dir);
        let spec = claude::interactive_launch_spec_with_program(
            PathBuf::from("/tmp/claude"),
            Path::new("/tmp"),
            &lane,
            "default",
            Some("resume-123"),
            None,
            Some(9123),
            "session-123",
            Path::new("/tmp/halv.sqlite3"),
            Some(Path::new("/tmp/halv-codex-session-123")),
            PonytailMode::Off,
            false,
            None,
        )
        .expect("Claude interactive specification should build");
        match previous {
            Some(value) => std::env::set_var("CLAUDE_CONFIG_DIR", value),
            None => std::env::remove_var("CLAUDE_CONFIG_DIR"),
        }
        let _ = std::fs::remove_dir_all(&config_dir);
        let spec = add_savings_stack_to_launch_spec(spec, "session-123");
        let args = spec_args(&spec);
        assert_eq!(args[0], "--mcp-config");
        assert!(args[1].starts_with('{'));
        assert_eq!(
            &args[args.len() - 4..],
            ["--permission-mode", "default", "--resume", "resume-123"]
        );
        assert!(!args.iter().any(|argument| argument == "--print"));
        assert_eq!(
            spec_env(&spec, "ANTHROPIC_BASE_URL"),
            Some("http://127.0.0.1:9123/p/session-123")
        );
        assert_eq!(
            spec_env(&spec, "ANTHROPIC_AUTH_TOKEN"),
            Some("secret-value")
        );
        assert_eq!(spec_env(&spec, "HEADROOM_PROJECT"), Some("session-123"));
        assert_eq!(
            spec_env(&spec, "CODEX_HOME"),
            Some("/tmp/halv-codex-session-123")
        );
        assert!(spec
            .env_remove
            .iter()
            .any(|name| name == "ANTHROPIC_API_KEY"));
        assert!(spec_env(&spec, "PATH").is_some());
    }

    #[test]
    fn codex_interactive_spec_removes_app_server_transport() {
        let spec = codex::interactive_launch_spec_with_program(
            PathBuf::from("/tmp/codex"),
            Path::new("/tmp"),
            &SessionLane::CodexChatGpt { proxied: true },
            "default",
            None,
            Some(9123),
            "session-123",
            Path::new("/tmp/halv.sqlite3"),
            PonytailMode::Off,
        )
        .expect("Codex interactive specification should build");
        let spec = add_savings_stack_to_launch_spec(spec, "session-123");
        let args = spec_args(&spec);
        assert!(!args.iter().any(|argument| argument == "app-server"));
        assert!(!args.iter().any(|argument| argument == "--stdio"));
        assert!(!args
            .iter()
            .any(|argument| argument.contains("chatgpt_base_url")));
        assert!(args.iter().any(|argument| {
            argument.contains("openai_base_url=\"http://127.0.0.1:9123/p/session-123/v1\"")
        }));
        assert!(spec_env(&spec, "PATH").is_some());
        assert_eq!(spec_env(&spec, "HEADROOM_PROJECT"), Some("session-123"));
    }

    #[test]
    fn kimi_interactive_spec_keeps_api_key_proxy_environment() {
        let spec = kimi::interactive_launch_spec_with_program(
            PathBuf::from("/tmp/kimi"),
            &SessionLane::KimiApiKey {
                credential: SessionCredential::ApiKey {
                    api_key: "secret-value".to_string(),
                },
            },
            "ask",
            None,
            Some(9123),
            "session-123",
            Some(Path::new("/tmp/halv-codex-session-123")),
        )
        .expect("Kimi interactive specification should build");
        let spec = add_savings_stack_to_launch_spec(spec, "session-123");
        let mut args = spec_args(&spec);
        let agent_index = args
            .iter()
            .position(|argument| argument == "--agent-file")
            .expect("Kimi interactive arguments include agent file");
        args.drain(agent_index..=agent_index + 1);
        assert!(args.is_empty());
        assert_eq!(
            spec_env(&spec, "KIMI_MODEL_BASE_URL"),
            Some("http://127.0.0.1:9123/p/session-123/v1")
        );
        assert_eq!(spec_env(&spec, "KIMI_MODEL_API_KEY"), Some("secret-value"));
        assert!(spec_env(&spec, "PATH").is_some());
        assert_eq!(spec_env(&spec, "HEADROOM_PROJECT"), Some("session-123"));
        assert_eq!(
            spec_env(&spec, "CODEX_HOME"),
            Some("/tmp/halv-codex-session-123")
        );
    }

    #[test]
    fn kimi_interactive_spec_routes_subscription_through_isolated_proxy() {
        let spec = kimi::interactive_launch_spec_with_program(
            PathBuf::from("/tmp/kimi"),
            &SessionLane::KimiSubscription,
            "ask",
            None,
            Some(9123),
            "session-123",
            Some(Path::new("/tmp/halv-codex-session-123")),
        )
        .expect("Kimi subscription specification should build");
        let spec = add_savings_stack_to_launch_spec(spec, "session-123");

        assert_eq!(
            spec_env(&spec, "KIMI_BASE_URL"),
            Some("http://127.0.0.1:9123/p/session-123/v1")
        );
        assert_eq!(spec_env(&spec, "KIMI_MODEL_NAME"), Some("kimi-for-coding"));
        assert_eq!(
            kimi_isolated_upstream(&SessionLane::KimiSubscription),
            Some(kimi::KIMI_SUBSCRIPTION_UPSTREAM)
        );
        assert_eq!(
            kimi::KIMI_SUBSCRIPTION_UPSTREAM,
            "https://api.kimi.com/coding/v1"
        );
        assert!(spec_env(&spec, "PATH").is_some());
        assert_eq!(spec_env(&spec, "HEADROOM_PROJECT"), Some("session-123"));
    }

    #[test]
    fn every_message_lands_a_point_even_without_savings() {
        let mut requests = 0;
        let mut tokens = 0;

        // First message: one request, no compression yet -> boundary zero.
        assert_eq!(
            proxy_savings_sample(&mut requests, 1, &mut tokens, 0),
            Some((0, true))
        );
        // Quiet poll: no new request, no point.
        assert_eq!(proxy_savings_sample(&mut requests, 1, &mut tokens, 0), None);
        // Second message saves tokens -> growth point on a new boundary.
        assert_eq!(
            proxy_savings_sample(&mut requests, 2, &mut tokens, 750),
            Some((750, true))
        );
        // Savings can grow within the same message: no boundary.
        assert_eq!(
            proxy_savings_sample(&mut requests, 2, &mut tokens, 900),
            Some((150, false))
        );
    }

    #[test]
    fn terminal_crux_ledger_prices_each_new_call_once() {
        let path = std::env::temp_dir().join(format!(
            "halv-terminal-crux-savings-{}.jsonl",
            new_uuid_v4()
        ));
        std::fs::write(
            &path,
            concat!(
                "{\"tool\":\"scip_map\",\"saved_tokens\":1}\n",
                "{\"tool\":\"custom_lookup\",\"saved_tokens\":75}\n",
                "{\"tool\":\"scip_index\",\"saved_tokens\":900}\n",
                "{broken\n"
            ),
        )
        .expect("Crux ledger should write");
        let mut previous = 0;
        let mut recorded = Vec::new();
        let first = terminal_crux_tokens_saved_at(&path).expect("Crux ledger should load");
        apply_project_savings_sample(&mut previous, first, |tokens| recorded.push(tokens));

        std::fs::write(
            &path,
            concat!(
                "{\"tool\":\"scip_map\",\"saved_tokens\":1}\n",
                "{\"tool\":\"custom_lookup\",\"saved_tokens\":75}\n",
                "{\"tool\":\"scip_index\",\"saved_tokens\":900}\n",
                "{\"tool\":\"scip_find\",\"saved_tokens\":2}\n"
            ),
        )
        .expect("Crux ledger should update");
        let second = terminal_crux_tokens_saved_at(&path).expect("Crux ledger should reload");
        apply_project_savings_sample(&mut previous, second, |tokens| recorded.push(tokens));
        apply_project_savings_sample(&mut previous, second, |tokens| recorded.push(tokens));

        assert_eq!(recorded, [1_975, 400]);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn terminal_crux_index_only_ledger_reports_zero() {
        let path =
            std::env::temp_dir().join(format!("halv-terminal-crux-index-{}.jsonl", new_uuid_v4()));
        std::fs::write(&path, "{\"tool\":\"scip_index\",\"saved_tokens\":900}\n")
            .expect("Crux ledger should write");

        assert_eq!(terminal_crux_tokens_saved_at(&path), Some(0));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn launch_spec_scopes_the_rtk_ledger_to_the_session() {
        let expected = exec::rtk_session_database_path("session-123");
        let spec = add_savings_stack_to_launch_spec(
            LaunchSpec {
                program: PathBuf::from("/tmp/agent"),
                args: vec![],
                env: vec![],
                env_remove: vec![],
            },
            "session-123",
        );
        let spec = add_savings_stack_to_launch_spec(spec, "session-123");
        assert_eq!(spec_env(&spec, exec::RTK_DB_PATH_ENV), expected.to_str(),);
        assert!(expected.ends_with("session-session-123.db"));
        assert_eq!(
            spec_env(&spec, "CRUX_SAVINGS_LEDGER"),
            exec::crux_session_ledger_path("session-123").to_str(),
        );
        for (name, _) in savings_stack_environment_with_rtk(expected) {
            assert_eq!(
                spec.env
                    .iter()
                    .filter(|(candidate, _)| candidate == &name)
                    .count(),
                1,
                "launch environment should contain one {name:?} entry"
            );
        }
    }

    #[test]
    fn savings_stack_environment_exposes_bundled_tools_and_session_ledgers() {
        let environment = savings_stack_environment("session-123");
        let path = environment
            .iter()
            .find_map(|(name, value)| (name == "PATH").then_some(value))
            .expect("agent PATH should exist");
        let paths = std::env::split_paths(path).collect::<Vec<_>>();
        let crux_name = if cfg!(windows) { "crux.exe" } else { "crux" };
        let rtk_name = if cfg!(windows) { "rtk.exe" } else { "rtk" };
        assert!(paths
            .iter()
            .any(|directory| directory.join(crux_name).is_file()));
        assert!(paths
            .iter()
            .any(|directory| directory.join(rtk_name).is_file()));
        assert_eq!(
            spec_env_value(&environment, exec::RTK_DB_PATH_ENV),
            Some(
                exec::rtk_session_database_path("session-123")
                    .to_string_lossy()
                    .into_owned()
            )
        );
        assert_eq!(
            spec_env_value(&environment, "CRUX_SAVINGS_LEDGER"),
            Some(
                exec::crux_session_ledger_path("session-123")
                    .to_string_lossy()
                    .into_owned()
            )
        );
        assert_eq!(
            spec_env_value(&environment, "RTK_TELEMETRY_DISABLED").as_deref(),
            Some("1")
        );
    }

    #[test]
    fn a_gui_turn_that_waits_for_a_permission_blocks_a_restart() {
        let manager = AgentSessionManager::default();
        let gui_record = |id: &str, mode: SessionMode, status: &str| AgentSession {
            id: id.to_string(),
            agent_kind: AgentKind::Codex,
            lane_kind: SessionLaneKind::CodexChatGpt,
            mode,
            ponytail_mode: None,
            project_id: None,
            conversation_id: None,
            conversation_title: None,
            default_title: "Codex 1".to_string(),
            generated_title: None,
            ai_vault_title: None,
            custom_title: None,
            generation: 0,
            model: None,
            cwd: "/tmp/project".to_string(),
            pid: 123,
            status: status.to_string(),
            cli_session_id: Some("codex-thread".to_string()),
            created_at: "2026-10-15T12:00:00Z".to_string(),
            last_active_at: "2026-10-15T12:01:00Z".to_string(),
            exit_code: None,
            savings_sources: SavingsSources {
                rtk: SavingsSourceState::Missing,
                crux: SavingsSourceState::Missing,
                proxy: false,
            },
        };
        let insert = |record: AgentSession| {
            let id = record.id.clone();
            let managed = build_rehydrated_managed_session(
                record,
                SessionLane::CodexChatGpt { proxied: false },
                SessionSavingsPaths::from_paths(&id, None, None),
                Arc::new(AtomicUsize::new(0)),
                None,
            );
            manager
                .sessions
                .lock()
                .expect("agent sessions lock poisoned")
                .insert(id, managed.clone());
            managed
        };
        let asking = insert(gui_record("gui-asking", SessionMode::Gui, "waiting_input"));
        let idle = insert(gui_record("gui-idle", SessionMode::Gui, "waiting_input"));
        let terminal = insert(gui_record(
            "terminal-asking",
            SessionMode::TerminalEmbedded,
            "waiting_input",
        ));
        assert_eq!(manager.busy_gui_session_count(), 0);

        // A PermissionRequest sets `waiting_input`, as a completed turn does.
        for session in [&asking, &terminal] {
            session
                .adapter_state
                .lock()
                .expect("agent adapter state lock poisoned")
                .register_pending_permission(pending_permission("approval-1"));
        }
        assert_eq!(manager.busy_gui_session_count(), 1);

        idle.set_status("running");
        assert_eq!(manager.busy_gui_session_count(), 2);
        asking.set_status("exited");
        assert_eq!(manager.busy_gui_session_count(), 1);
    }

    #[test]
    fn a_supervisor_protocol_mismatch_keeps_agent_rows_live() {
        let path = std::env::temp_dir().join(format!(
            "halv-supervisor-mismatch-rows-{}.sqlite3",
            new_uuid_v4()
        ));
        let storage = Storage::open(&path).expect("mismatch fixture storage should open");
        let now = Utc::now().to_rfc3339();
        for id in ["old-supervisor-agent", "orphan-agent"] {
            storage
                .upsert_agent_session(&AgentSessionRecord {
                    id,
                    agent_kind: "claude",
                    lane: "claude_subscription",
                    mode: "terminal_embedded",
                    ponytail_mode: None,
                    project_id: None,
                    conversation_id: None,
                    cwd: "/tmp",
                    cli_session_id: Some("cli-session"),
                    custom_title: None,
                    default_title: "Claude 1",
                    generated_title: None,
                    ai_vault_title: None,
                    ai_vault_session_id: None,
                    generation: 0,
                    status: "running",
                    created_at: &now,
                    last_active_at: &now,
                    exit_code: None,
                })
                .expect("mismatch fixture session should insert");
        }
        let status = |id: &str| {
            storage
                .get_agent_session(id)
                .expect("the session row should load")
                .expect("the session row should exist")
                .status
        };

        // The app cannot list the old supervisor, so no terminal is running.
        reconcile_supervised_rows(&storage, &HashSet::new(), true)
            .expect("a mismatch should skip the reconcile");
        assert_eq!(status("old-supervisor-agent"), "running");
        assert_eq!(status("orphan-agent"), "running");

        let live = HashSet::from(["old-supervisor-agent".to_string()]);
        reconcile_supervised_rows(&storage, &live, false).expect("the reconcile should run");
        assert_eq!(status("old-supervisor-agent"), "running");
        assert_eq!(status("orphan-agent"), "exited");
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn terminal_poll_records_initial_nonzero_headroom_project_savings() {
        let path = std::env::temp_dir().join(format!(
            "halv-terminal-poll-savings-{}.sqlite3",
            new_uuid_v4()
        ));
        let storage = Storage::open(&path).expect("savings fixture storage should open");
        let session_id = new_uuid_v4();
        let now = Utc::now().to_rfc3339();
        storage
            .upsert_agent_session(&AgentSessionRecord {
                id: &session_id,
                agent_kind: "codex",
                lane: "codex_chatgpt",
                mode: "terminal_embedded",
                ponytail_mode: Some(PonytailMode::Off),
                project_id: None,
                conversation_id: None,
                cwd: "/tmp",
                cli_session_id: None,
                custom_title: None,
                default_title: "Codex 1",
                generated_title: None,
                ai_vault_title: None,
                ai_vault_session_id: None,
                generation: 0,
                status: "running",
                created_at: &now,
                last_active_at: &now,
                exit_code: None,
            })
            .expect("savings fixture session should insert");
        let mut previous = 0;
        let mut recorded = Vec::new();
        apply_project_savings_sample(&mut previous, 55, |tokens| {
            persist_agent_turn(
                &storage,
                &session_id,
                AgentKind::Codex,
                Some("gpt-5"),
                SessionMode::TerminalEmbedded,
                Some(PonytailMode::Off),
                0,
                &TurnUsage::default(),
                Some(MeasuredSavings {
                    tokens_saved: tokens,
                    savings_usd: None,
                    once_tokens: 0,
                }),
                0,
                false,
                &now,
                &TurnPriceContext::default(),
            )
            .expect("poll savings should persist");
            recorded.push(tokens);
        });
        apply_project_savings_sample(&mut previous, 55, |tokens| recorded.push(tokens));
        apply_project_savings_sample(&mut previous, 40, |tokens| recorded.push(tokens));
        apply_project_savings_sample(&mut previous, 52, |tokens| recorded.push(tokens));

        assert_eq!(recorded, [55, 12]);
        assert_eq!(
            storage
                .total_savings()
                .expect("persisted poll savings should load")
                .tokens_saved,
            55
        );
        let event = serde_json::to_value(AgentEvent::SavingsSample {
            savings_tokens: recorded[0],
            savings_source: "measured".to_string(),
            cache_read_tokens: 0,
            turn_boundary: false,
            dollars_usd: 0.0,
        })
        .expect("savings event should serialize");
        assert_eq!(event["savingsTokens"], 55);
        assert_eq!(event["savingsSource"], "measured");
        drop(storage);
        let _ = std::fs::remove_file(path);
    }

    fn insert_terminal_test_session(storage: &Storage, session_id: &str, now: &str) {
        storage
            .upsert_agent_session(&AgentSessionRecord {
                id: session_id,
                agent_kind: "codex",
                lane: "codex_chatgpt",
                mode: "terminal_embedded",
                ponytail_mode: Some(PonytailMode::Off),
                project_id: None,
                conversation_id: None,
                cwd: "/tmp",
                cli_session_id: None,
                custom_title: None,
                default_title: "Codex 1",
                generated_title: None,
                ai_vault_title: None,
                ai_vault_session_id: None,
                generation: 0,
                status: "running",
                created_at: now,
                last_active_at: now,
                exit_code: None,
            })
            .expect("terminal test session should insert");
    }

    fn persist_terminal_test_sample(
        storage: &Storage,
        session_id: &str,
        turn_index: &AtomicUsize,
        now: &str,
        sample: TerminalSavingsSample,
    ) {
        persist_agent_turn(
            storage,
            session_id,
            AgentKind::Codex,
            Some("gpt-5"),
            SessionMode::TerminalEmbedded,
            Some(PonytailMode::Off),
            turn_index.fetch_add(1, Ordering::SeqCst),
            &TurnUsage {
                cache_read_tokens: sample.cache_read_tokens,
                ..TurnUsage::default()
            },
            Some(MeasuredSavings {
                tokens_saved: sample.savings_tokens,
                savings_usd: None,
                once_tokens: 0,
            }),
            sample.crux_tokens_saved,
            false,
            now,
            &TurnPriceContext::default(),
        )
        .expect("terminal savings sample should persist");
    }

    #[test]
    fn resumed_terminal_persists_immediate_headroom_then_rtk_and_crux() {
        let path = std::env::temp_dir().join(format!(
            "halv-terminal-resume-savings-{}.sqlite3",
            new_uuid_v4()
        ));
        let storage = Storage::open(&path).expect("resume fixture storage should open");
        let previous_session_id = new_uuid_v4();
        let session_id = new_uuid_v4();
        let now = Utc::now().to_rfc3339();
        insert_terminal_test_session(&storage, &previous_session_id, &now);
        insert_terminal_test_session(&storage, &session_id, &now);
        for index in 0..2 {
            persist_agent_turn(
                &storage,
                &previous_session_id,
                AgentKind::Codex,
                Some("gpt-5"),
                SessionMode::TerminalEmbedded,
                Some(PonytailMode::Off),
                index,
                &TurnUsage::default(),
                Some(MeasuredSavings {
                    tokens_saved: 10,
                    savings_usd: None,
                    once_tokens: 0,
                }),
                0,
                false,
                &now,
                &TurnPriceContext::default(),
            )
            .expect("previous terminal savings should persist");
        }

        let next_turn_index = storage
            .migrate_agent_session_turns(&previous_session_id, &session_id)
            .expect("resume migration should succeed");
        let turn_index = AtomicUsize::new(next_turn_index);
        let state = Mutex::new(TerminalSavingsState::default());
        let finished = AtomicBool::new(false);
        sample_terminal_savings_once(
            &state,
            &finished,
            false,
            || TerminalSavingsSnapshot {
                proxy: Some((1, 50, 5)),
                rtk: Some(0),
                crux: Some(0),
            },
            |sample| {
                persist_terminal_test_sample(&storage, &session_id, &turn_index, &now, sample);
            },
        );
        sample_terminal_savings_once(
            &state,
            &finished,
            false,
            || TerminalSavingsSnapshot {
                proxy: Some((1, 50, 5)),
                rtk: Some(7),
                crux: Some(9),
            },
            |sample| {
                persist_terminal_test_sample(&storage, &session_id, &turn_index, &now, sample);
            },
        );

        assert_eq!(next_turn_index, 2);
        assert_eq!(turn_index.load(Ordering::SeqCst), 5);
        let totals = storage
            .agent_session_turn_totals()
            .expect("resumed terminal totals should load")
            .into_iter()
            .find(|row| row.session_id == session_id)
            .expect("resumed terminal totals should exist");
        assert_eq!(totals.savings_tokens, 86);
        assert_eq!(totals.crux_tokens_saved, 9);
        assert_eq!(totals.cache_read_tokens, 5);
        let new_event_prefix = format!("agent-turn:{session_id}:");
        assert_eq!(
            storage
                .unreported_savings(500)
                .expect("resume savings queue should load")
                .iter()
                .filter(|event| event.id.starts_with(&new_event_prefix))
                .count(),
            3
        );

        drop(storage);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn final_terminal_sample_persists_once_when_the_poller_races() {
        let path = std::env::temp_dir().join(format!(
            "halv-terminal-final-savings-{}.sqlite3",
            new_uuid_v4()
        ));
        let storage = Storage::open(&path).expect("final fixture storage should open");
        let session_id = new_uuid_v4();
        let now = Utc::now().to_rfc3339();
        insert_terminal_test_session(&storage, &session_id, &now);
        let state = Mutex::new(TerminalSavingsState::default());
        let finished = AtomicBool::new(false);
        let turn_index = AtomicUsize::new(0);
        sample_terminal_savings_once(
            &state,
            &finished,
            false,
            || TerminalSavingsSnapshot {
                proxy: Some((0, 0, 0)),
                rtk: Some(0),
                crux: Some(0),
            },
            |_| panic!("zero baseline must not create a sample"),
        );
        let final_snapshot = TerminalSavingsSnapshot {
            proxy: Some((1, 11, 3)),
            rtk: Some(7),
            crux: Some(5),
        };

        thread::scope(|scope| {
            scope.spawn(|| {
                sample_terminal_savings_once(
                    &state,
                    &finished,
                    false,
                    || final_snapshot,
                    |sample| {
                        persist_terminal_test_sample(
                            &storage,
                            &session_id,
                            &turn_index,
                            &now,
                            sample,
                        );
                    },
                );
            });
            scope.spawn(|| {
                sample_terminal_savings_once(
                    &state,
                    &finished,
                    true,
                    || final_snapshot,
                    |sample| {
                        persist_terminal_test_sample(
                            &storage,
                            &session_id,
                            &turn_index,
                            &now,
                            sample,
                        );
                    },
                );
            });
        });

        assert!(finished.load(Ordering::SeqCst));
        assert_eq!(turn_index.load(Ordering::SeqCst), 3);
        let totals = storage
            .agent_session_turn_totals()
            .expect("final terminal totals should load")
            .into_iter()
            .find(|row| row.session_id == session_id)
            .expect("final terminal totals should exist");
        assert_eq!(totals.savings_tokens, 23);
        assert_eq!(totals.crux_tokens_saved, 5);
        assert_eq!(totals.cache_read_tokens, 3);
        assert_eq!(
            storage
                .unreported_savings(500)
                .expect("final savings queue should load")
                .len(),
            3
        );

        drop(storage);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn agent_savings_settle_requires_zero_grace_but_not_positive_grace() {
        let zero_before = ProjectSavingsTotals {
            tokens_saved: 100,
            savings_usd: Some(1.0),
        };
        let mut zero_tracker = AgentSavingsSettleTracker::default();
        assert!(!zero_tracker.observe(zero_before, zero_before, Duration::ZERO));
        assert!(!zero_tracker.observe(zero_before, zero_before, SAVINGS_SETTLE_DELAY));
        assert!(!zero_tracker.observe(zero_before, zero_before, SAVINGS_SETTLE_DELAY * 2));
        assert!(!zero_tracker.observe(zero_before, zero_before, Duration::from_millis(1_400)));
        assert!(zero_tracker.observe(zero_before, zero_before, Duration::from_millis(1_750)));

        let positive_after = ProjectSavingsTotals {
            tokens_saved: 125,
            savings_usd: Some(1.25),
        };
        let mut positive_tracker = AgentSavingsSettleTracker::default();
        assert!(!positive_tracker.observe(zero_before, positive_after, Duration::ZERO));
        assert!(!positive_tracker.observe(zero_before, positive_after, SAVINGS_SETTLE_DELAY));
        assert!(positive_tracker.observe(zero_before, positive_after, SAVINGS_SETTLE_DELAY * 2));
    }

    #[test]
    fn terminal_samples_use_the_dollar_methods() {
        let path =
            std::env::temp_dir().join(format!("halv-terminal-dollars-{}.sqlite3", new_uuid_v4()));
        let storage = Storage::open(&path).expect("dollar fixture storage should open");
        let session_id = new_uuid_v4();
        let now = Utc::now().to_rfc3339();
        storage
            .upsert_agent_session(&AgentSessionRecord {
                id: &session_id,
                agent_kind: "claude",
                lane: "claude_subscription",
                mode: "terminal_embedded",
                ponytail_mode: Some(PonytailMode::Off),
                project_id: None,
                conversation_id: None,
                cwd: "/tmp",
                cli_session_id: None,
                custom_title: None,
                default_title: "Claude 1",
                generated_title: None,
                ai_vault_title: None,
                ai_vault_session_id: None,
                generation: 0,
                status: "running",
                created_at: &now,
                last_active_at: &now,
                exit_code: None,
            })
            .expect("terminal session should insert");
        let context = TurnPriceContext {
            observed_model: Some("claude-opus-5-5".to_string()),
            window_rate: Some(0.30),
            running_rate: Some(0.40),
            cache_write_1h_tokens: 7,
        };
        // A Headroom sample, then an RTK sample. The launch hint is a Codex
        // id; the observed model wins.
        for (index, once_tokens) in [(0, 0), (1, 1_000_000)] {
            persist_agent_turn(
                &storage,
                &session_id,
                AgentKind::Claude,
                Some("gpt-5.6-sol"),
                SessionMode::TerminalEmbedded,
                Some(PonytailMode::Off),
                index,
                &TurnUsage::default(),
                Some(MeasuredSavings {
                    tokens_saved: 1_000_000,
                    savings_usd: None,
                    once_tokens,
                }),
                0,
                false,
                &now,
                &context,
            )
            .expect("terminal turn should persist");
        }
        let connection = rusqlite::Connection::open(&path).expect("fixture should reopen");
        let rows = connection
            .prepare(
                "SELECT savings_usd, usd_method, model, source_kind, price_version,
                        cache_write_1h_tokens
                 FROM agent_session_turns WHERE session_id = ?1 ORDER BY turn_index",
            )
            .expect("rows should prepare")
            .query_map([&session_id], |row| {
                Ok((
                    row.get::<_, f64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, i64>(5)?,
                ))
            })
            .expect("rows should load")
            .collect::<Result<Vec<_>, _>>()
            .expect("rows should decode");
        assert!((rows[0].0 - 0.30).abs() < 1e-12);
        assert_eq!(rows[0].1, "effective_rate");
        assert_eq!(rows[0].2, "claude-opus-5-5");
        assert_eq!(rows[0].3, "headroom");
        assert_eq!(rows[0].4, pricing::current().price_version);
        assert_eq!(rows[0].5, 7);
        assert!((rows[1].0 - 4.00).abs() < 1e-12);
        assert_eq!(rows[1].1, "input_once");
        assert_eq!(rows[1].3, "rtk");
        let totals = storage.total_savings().expect("totals should load");
        assert!((totals.dollars_usd - 4.30).abs() < 1e-9);
        drop(connection);
        drop(storage);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn standalone_gui_turn_enqueues_but_covered_gui_turn_does_not() {
        let path =
            std::env::temp_dir().join(format!("halv-agent-turn-savings-{}.sqlite3", new_uuid_v4()));
        let storage = Storage::open(&path).expect("savings fixture storage should open");
        let terminal_session_id = new_uuid_v4();
        let gui_session_id = new_uuid_v4();
        let now = Utc::now().to_rfc3339();
        let insert_session = |session_id: &str, mode: &str| {
            storage.upsert_agent_session(&AgentSessionRecord {
                id: session_id,
                agent_kind: "codex",
                lane: "codex_chatgpt",
                mode,
                ponytail_mode: Some(PonytailMode::Off),
                project_id: None,
                conversation_id: None,
                cwd: "/tmp",
                cli_session_id: None,
                custom_title: None,
                default_title: "Codex 1",
                generated_title: None,
                ai_vault_title: None,
                ai_vault_session_id: None,
                generation: 0,
                status: "running",
                created_at: &now,
                last_active_at: &now,
                exit_code: None,
            })
        };
        insert_session(&terminal_session_id, "terminal_embedded")
            .expect("terminal session should insert");
        insert_session(&gui_session_id, "gui").expect("GUI session should insert");

        persist_agent_turn(
            &storage,
            &terminal_session_id,
            AgentKind::Codex,
            Some("gpt-5"),
            SessionMode::TerminalEmbedded,
            Some(PonytailMode::Off),
            0,
            &TurnUsage::default(),
            Some(MeasuredSavings {
                tokens_saved: 64,
                savings_usd: None,
                once_tokens: 0,
            }),
            0,
            false,
            &now,
            &TurnPriceContext::default(),
        )
        .expect("terminal turn should persist");
        persist_agent_turn(
            &storage,
            &gui_session_id,
            AgentKind::Codex,
            Some("gpt-5"),
            SessionMode::Gui,
            Some(PonytailMode::Off),
            0,
            &TurnUsage::default(),
            Some(MeasuredSavings {
                tokens_saved: 32,
                savings_usd: None,
                once_tokens: 0,
            }),
            0,
            false,
            &now,
            &TurnPriceContext::default(),
        )
        .expect("GUI turn should persist");
        persist_agent_turn(
            &storage,
            &gui_session_id,
            AgentKind::Codex,
            Some("gpt-5"),
            SessionMode::Gui,
            Some(PonytailMode::Off),
            1,
            &TurnUsage::default(),
            Some(MeasuredSavings {
                tokens_saved: 16,
                savings_usd: None,
                once_tokens: 0,
            }),
            0,
            true,
            &now,
            &TurnPriceContext::default(),
        )
        .expect("covered GUI turn should persist");

        let events = storage
            .unreported_savings(500)
            .expect("savings queue should load");
        assert_eq!(events.len(), 2);
        let mut saved_tokens = events
            .iter()
            .map(|event| event.tokens_saved)
            .collect::<Vec<_>>();
        saved_tokens.sort_unstable();
        assert_eq!(saved_tokens, [32, 64]);
        let event_prefix = format!("agent-turn:{terminal_session_id}:");
        let terminal_event = events
            .iter()
            .find(|event| event.id.starts_with(&event_prefix))
            .expect("terminal event should use the session ID");
        let row_id = terminal_event
            .id
            .strip_prefix(&event_prefix)
            .expect("terminal event should use the session id")
            .parse::<i64>()
            .expect("terminal event should use a row id");
        assert!(row_id > 0);
        let gui_event_prefix = format!("agent-turn:{gui_session_id}:");
        assert!(events
            .iter()
            .any(|event| event.id.starts_with(&gui_event_prefix)));

        drop(storage);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn gui_agent_snapshot_prices_headroom_and_crux_with_the_v2_methods() {
        let path =
            std::env::temp_dir().join(format!("halv-gui-agent-savings-{}.sqlite3", new_uuid_v4()));
        let storage = Storage::open(&path).expect("GUI savings fixture should open");
        let conversation_id = storage
            .ensure_conversation(None, "GUI agent savings", "gpt-5.6-sol", None)
            .expect("conversation should insert");
        storage
            .set_conversation_agent_backend(
                &conversation_id,
                Some("codex_session"),
                Some("gpt-5.6-sol"),
            )
            .expect("agent backend should persist");
        let assistant = storage
            .insert_message(NewMessage {
                conversation_id: &conversation_id,
                role: "assistant",
                content: "Done",
                model: Some("gpt-5.6-sol"),
                input_tokens: Some(100),
                output_tokens: Some(20),
                error: false,
                mentions: None,
            })
            .expect("assistant should insert");
        // 750 Headroom tokens and 250 crux tokens; Headroom's own dollar
        // figure is ignored.
        let savings = MeasuredSavings {
            tokens_saved: 1_000,
            savings_usd: Some(9.99),
            once_tokens: 250,
        };
        let non_crux_savings = measured_savings_without_crux(savings, 250);
        assert_eq!(
            non_crux_savings,
            MeasuredSavings {
                tokens_saved: 750,
                savings_usd: Some(9.99),
                once_tokens: 0,
            }
        );
        let priced = price_turn_savings(
            AgentKind::Codex,
            SessionMode::Gui,
            Some("gpt-5.6-sol"),
            &TurnUsage::default(),
            &TurnPriceContext::default(),
            savings,
            250,
        );
        storage
            .insert_priced_savings_snapshot(
                &assistant.id,
                u64_to_i64(non_crux_savings.tokens_saved),
                250,
                &SnapshotDollars {
                    usd: priced.usd,
                    source: priced.source,
                    usd_method: priced.method,
                    price_version: &priced.price_version,
                },
            )
            .expect("GUI savings snapshot should insert");

        let messages = storage
            .get_messages(&conversation_id)
            .expect("GUI savings snapshot should load");
        assert_eq!(messages[0].tokens_saved, Some(1_000));
        // No usage: the default mix prices the Headroom share, and crux
        // tokens count once at the $4.00/M input price.
        let card = pricing::resolve(Some("codex"), Some("gpt-5.6-sol")).card;
        let mix_rate = card.mix_prompt_rate(&pricing::current().default_prompt_mix);
        let expected = (750.0 * mix_rate + 250.0 * 4.00) / 1_000_000.0;
        let saved = messages[0]
            .savings_usd
            .expect("snapshot should have dollars");
        assert!((saved - expected).abs() < 1e-12, "{saved} != {expected}");
        assert_eq!(priced.method, dollars::METHOD_MIXED);
        assert_eq!(messages[0].pricing_source.as_deref(), Some("model_table"));
        let totals = storage.total_savings().expect("GUI savings should total");
        assert_eq!(totals.tokens_saved, 1_000);
        assert_eq!(totals.crux_tokens_saved, 250);

        drop(storage);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn snapshots_and_events_use_the_frontend_contract() {
        let session = AgentSession {
            id: "f47ac10b-58cc-4372-a567-0e02b2c3d479".to_string(),
            agent_kind: AgentKind::Claude,
            lane_kind: SessionLaneKind::ClaudeApiKey,
            mode: SessionMode::Gui,
            ponytail_mode: Some(PonytailMode::Full),
            project_id: Some(42),
            conversation_id: None,
            conversation_title: None,
            default_title: "Claude 1".to_string(),
            generated_title: None,
            ai_vault_title: None,
            custom_title: Some("Release checks".to_string()),
            generation: 0,
            model: None,
            cwd: "/tmp/project".to_string(),
            pid: 123,
            status: "running".to_string(),
            cli_session_id: Some("claude-session".to_string()),
            created_at: "2026-08-16T12:00:00Z".to_string(),
            last_active_at: "2026-08-16T12:01:00Z".to_string(),
            exit_code: None,
            savings_sources: SavingsSources {
                rtk: SavingsSourceState::Present,
                crux: SavingsSourceState::Missing,
                proxy: true,
            },
        };
        let snapshot = serde_json::to_value(session).expect("snapshot should serialize");
        assert_eq!(snapshot["agentKind"], "claude");
        assert_eq!(snapshot["laneKind"], "claudeApiKey");
        assert_eq!(snapshot["ponytailMode"], "full");
        assert_eq!(snapshot["projectId"], 42);
        assert_eq!(snapshot["defaultTitle"], "Claude 1");
        assert_eq!(snapshot["customTitle"], "Release checks");
        assert_eq!(snapshot["generation"], 0);
        assert_eq!(snapshot["savingsSources"]["rtk"], "present");
        assert_eq!(snapshot["savingsSources"]["crux"], "missing");
        assert_eq!(snapshot["savingsSources"]["proxy"], true);
        assert!(snapshot.get("title").is_none());

        let event = serde_json::to_value(AgentEvent::ToolCallStarted {
            name: "Read".to_string(),
            summary_json: "{}".to_string(),
        })
        .expect("event should serialize");
        assert_eq!(event["type"], "toolCallStarted");
        assert_eq!(event["summaryJson"], "{}");

        let event = serde_json::to_value(AgentEvent::SavingsSourcesChanged {
            savings_sources: SavingsSources {
                rtk: SavingsSourceState::Present,
                crux: SavingsSourceState::Present,
                proxy: false,
            },
        })
        .expect("savings sources event should serialize");
        assert_eq!(event["type"], "savingsSourcesChanged");
        assert_eq!(event["savingsSources"]["rtk"], "present");
        assert_eq!(event["savingsSources"]["crux"], "present");
        assert_eq!(event["savingsSources"]["proxy"], false);
    }

    #[test]
    fn savings_sources_track_file_creation_and_only_change_once() {
        let root =
            std::env::temp_dir().join(format!("halv-savings-sources-{}", rand::random::<u64>()));
        std::fs::create_dir_all(&root).expect("source fixture directory should exist");
        let rtk = root.join("session.db");
        let crux = root.join("crux.jsonl");
        let mut current = SavingsSources::from_paths(&rtk, &crux, false, false);
        assert_eq!(current.rtk, SavingsSourceState::Missing);
        assert_eq!(current.crux, SavingsSourceState::Missing);
        assert!(!current.proxy);

        std::fs::write(&rtk, []).expect("RTK source fixture should write");
        let next = SavingsSources::from_paths(&rtk, &crux, false, false);
        assert!(replace_savings_sources(&mut current, next));
        assert_eq!(current.rtk, SavingsSourceState::Present);
        assert_eq!(current.crux, SavingsSourceState::Missing);
        assert!(!replace_savings_sources(&mut current, next));

        std::fs::write(&crux, []).expect("Crux source fixture should write");
        let next = SavingsSources::from_paths(&rtk, &crux, true, false);
        assert!(replace_savings_sources(&mut current, next));
        assert_eq!(current.crux, SavingsSourceState::Present);
        assert!(current.proxy);

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn session_savings_paths_prefer_stored_paths() {
        let paths = SessionSavingsPaths::from_paths(
            "session-123",
            Some("/tmp/stored/session.db"),
            Some("/tmp/stored/crux.jsonl"),
        );

        assert_eq!(paths.rtk, Path::new("/tmp/stored/session.db"));
        assert_eq!(paths.crux, Path::new("/tmp/stored/crux.jsonl"));

        let computed = SessionSavingsPaths::from_paths("session-123", None, None);
        assert_eq!(computed.rtk, exec::rtk_session_database_path("session-123"));
        assert_eq!(computed.crux, exec::crux_session_ledger_path("session-123"));
    }

    #[test]
    fn legacy_savings_path_discovery_updates_the_session_row() {
        let database_path = std::env::temp_dir().join(format!(
            "halv-legacy-path-storage-{}.sqlite3",
            new_uuid_v4()
        ));
        let storage = Storage::open(&database_path).expect("storage should open");
        let session_id = new_uuid_v4();
        let now = Utc::now().to_rfc3339();
        storage
            .upsert_agent_session(&AgentSessionRecord {
                id: &session_id,
                agent_kind: "claude",
                lane: "claude_subscription",
                mode: "terminal_embedded",
                ponytail_mode: Some(PonytailMode::Off),
                project_id: None,
                conversation_id: None,
                cwd: "/tmp",
                cli_session_id: None,
                custom_title: None,
                default_title: "Claude 1",
                generated_title: None,
                ai_vault_title: None,
                ai_vault_session_id: None,
                generation: 0,
                status: "running",
                created_at: &now,
                last_active_at: &now,
                exit_code: None,
            })
            .expect("legacy session row should insert");
        let legacy_directory =
            std::env::temp_dir().join(format!("halv-rtk-legacy-{}", new_uuid_v4()));
        std::fs::create_dir_all(&legacy_directory)
            .expect("legacy RTK state directory should create");
        let legacy_directory = legacy_directory
            .canonicalize()
            .expect("legacy RTK state directory should canonicalize");
        let legacy_rtk = legacy_directory.join(format!("session-{session_id}.db"));
        let legacy_crux = legacy_directory.join(format!("crux-session-{session_id}.jsonl"));
        std::fs::write(&legacy_rtk, []).expect("legacy RTK database should write");
        std::fs::write(&legacy_crux, []).expect("legacy Crux ledger should write");

        let row = storage
            .get_agent_session(&session_id)
            .expect("legacy session row should load")
            .expect("legacy session row should exist");
        let paths = resolve_session_savings_paths(&storage, &row);
        let updated = storage
            .get_agent_session(&session_id)
            .expect("updated session row should load")
            .expect("updated session row should exist");

        assert_eq!(paths.rtk, legacy_rtk);
        assert_eq!(paths.crux, legacy_crux);
        assert_eq!(updated.rtk_db_path.as_deref(), paths.rtk.to_str());
        assert_eq!(updated.crux_ledger_path.as_deref(), paths.crux.to_str());
        drop(storage);
        let _ = std::fs::remove_file(database_path);
        let _ = std::fs::remove_dir_all(legacy_directory);
    }

    #[test]
    fn legacy_savings_path_discovery_survives_closed_storage() {
        let database_path = std::env::temp_dir().join(format!(
            "halv-closed-path-storage-{}.sqlite3",
            new_uuid_v4()
        ));
        let storage = Storage::open(&database_path).expect("storage should open");
        let session_id = new_uuid_v4();
        let now = Utc::now().to_rfc3339();
        storage
            .upsert_agent_session(&AgentSessionRecord {
                id: &session_id,
                agent_kind: "claude",
                lane: "claude_subscription",
                mode: "terminal_embedded",
                ponytail_mode: Some(PonytailMode::Off),
                project_id: None,
                conversation_id: None,
                cwd: "/tmp",
                cli_session_id: None,
                custom_title: None,
                default_title: "Claude 1",
                generated_title: None,
                ai_vault_title: None,
                ai_vault_session_id: None,
                generation: 0,
                status: "running",
                created_at: &now,
                last_active_at: &now,
                exit_code: None,
            })
            .expect("legacy session row should insert");
        let row = storage
            .get_agent_session(&session_id)
            .expect("legacy session row should load")
            .expect("legacy session row should exist");
        drop(storage);
        let _ = std::fs::remove_file(&database_path);

        let legacy_directory =
            std::env::temp_dir().join(format!("halv-rtk-legacy-{}", new_uuid_v4()));
        std::fs::create_dir_all(&legacy_directory)
            .expect("legacy RTK state directory should create");
        let legacy_directory = legacy_directory
            .canonicalize()
            .expect("legacy RTK state directory should canonicalize");
        let legacy_rtk = legacy_directory.join(format!("session-{session_id}.db"));
        let legacy_crux = legacy_directory.join(format!("crux-session-{session_id}.jsonl"));
        std::fs::write(&legacy_rtk, []).expect("legacy RTK database should write");
        std::fs::write(&legacy_crux, []).expect("legacy Crux ledger should write");

        let paths = discover_session_savings_paths(&row);

        assert_eq!(paths.rtk, legacy_rtk);
        assert_eq!(paths.crux, legacy_crux);
        let _ = std::fs::remove_dir_all(legacy_directory);
    }

    #[test]
    fn engine_off_sessions_start_no_engine_and_keep_the_nested_codex_home_slot() {
        let sidecar = SidecarManager::new(4320);
        sidecar.set_enabled(false);
        let api_key = || SessionCredential::ApiKey {
            api_key: "test-key".to_string(),
        };
        let shared_lanes = [
            (AgentKind::Claude, SessionLane::ClaudeSubscription),
            (
                AgentKind::Claude,
                SessionLane::ClaudeApiKey {
                    credential: api_key(),
                    base_url: None,
                },
            ),
            (
                AgentKind::Zai,
                SessionLane::ZaiGlm {
                    credential: api_key(),
                    model_overrides: None,
                },
            ),
            (
                AgentKind::Grok,
                SessionLane::GrokApiKey {
                    credential: api_key(),
                    model_overrides: None,
                },
            ),
            (
                AgentKind::Codex,
                SessionLane::CodexChatGpt { proxied: true },
            ),
            (
                AgentKind::Codex,
                SessionLane::Custom {
                    env: vec![("OPENAI_API_KEY".to_string(), "test-key".to_string())],
                },
            ),
            (AgentKind::Claude, SessionLane::Custom { env: Vec::new() }),
        ];
        for (agent_kind, lane) in shared_lanes {
            let context =
                tauri::async_runtime::block_on(session_proxy_context(agent_kind, &lane, &sidecar))
                    .expect("an engine-off context should build")
                    .expect("a formerly proxied lane keeps a direct context");
            assert!(context.sidecar.is_none());
            assert!(context.port.is_none());
            assert!(!context.routed());
            assert!(!context.shutdown_on_exit);
            assert!(context.savings_meter("session-1").is_none());
        }
        // Kimi's isolated engine never starts.
        for lane in [
            SessionLane::KimiSubscription,
            SessionLane::KimiApiKey {
                credential: api_key(),
            },
        ] {
            assert!(tauri::async_runtime::block_on(session_proxy_context(
                AgentKind::Kimi,
                &lane,
                &sidecar,
            ))
            .expect("an engine-off Kimi context should build")
            .is_none());
        }
        assert!(matches!(
            tauri::async_runtime::block_on(sidecar.refresh_status()),
            SidecarStatus::Off { port: 4320 }
        ));
        assert!(shared_proxy_port_from_status(
            AgentKind::Claude,
            SidecarStatus::Off { port: 4320 }
        )
        .is_err());
    }

    #[test]
    fn routed_contexts_build_a_savings_meter() {
        let sidecar = SidecarManager::new(4321);
        let routed = SessionProxyContext {
            sidecar: Some(sidecar),
            port: Some(4321),
            shutdown_on_exit: false,
            nested_codex_home: None,
        };
        assert!(routed.routed());
        let meter = routed
            .savings_meter("session-2")
            .expect("a routed context builds a meter");
        assert_eq!(meter.project, "session-2");
        assert!(SessionProxyContext::direct()
            .savings_meter("session-2")
            .is_none());
    }

    #[test]
    fn lane_proxy_sources_match_managed_proxy_routes() {
        assert!(!SessionLane::ClaudeSubscription.uses_proxy(AgentKind::Claude, false));
        assert!(SessionLane::ClaudeSubscription.uses_proxy(AgentKind::Claude, true));
        assert!(SessionLane::KimiSubscription.uses_proxy(AgentKind::Kimi, false));
        assert!(SessionLane::ClaudeApiKey {
            credential: SessionCredential::ApiKey {
                api_key: "test-key".to_string(),
            },
            base_url: None,
        }
        .uses_proxy(AgentKind::Claude, false));
        assert!(SessionLane::Custom {
            env: vec![("OPENAI_API_KEY".to_string(), "test-key".to_string())],
        }
        .uses_proxy(AgentKind::Codex, false));
        assert!(!SessionLane::Custom { env: Vec::new() }.uses_proxy(AgentKind::Codex, false));
        assert!(!SessionLane::Custom { env: Vec::new() }.uses_proxy(AgentKind::Gemini, false));
        assert!(!SessionLane::Custom { env: Vec::new() }.uses_proxy(AgentKind::Antigravity, false));
    }

    #[test]
    fn instruction_state_describes_codex_after_rtk_writes() {
        let root = std::env::temp_dir().join(format!(
            "halv-instruction-sources-{}",
            rand::random::<u64>()
        ));
        std::fs::create_dir_all(&root).expect("source fixture directory should exist");
        let rtk = root.join("session.db");
        let crux = root.join("crux.jsonl");

        let instruction = SavingsSources::from_paths(&rtk, &crux, true, true);
        assert_eq!(instruction.rtk, SavingsSourceState::Instruction);
        assert_eq!(
            serde_json::to_value(instruction).expect("instruction state should serialize")["rtk"],
            "instruction"
        );
        std::fs::write(&rtk, []).expect("RTK source fixture should write");
        let after_write = SavingsSources::from_paths(&rtk, &crux, true, true);
        assert_eq!(after_write.rtk, SavingsSourceState::Instruction);

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn conversation_event_mapping_streams_text_tools_usage_and_errors() {
        assert_eq!(humanize_tool_name("commandExecution"), "Command Execution");

        assert_eq!(
            map_agent_event_to_conversation(
                AgentKind::Claude,
                &AgentEvent::TextDelta {
                    text: "ok".to_string(),
                }
            ),
            ConversationEventMapping::Delta("ok".to_string()),
        );
        assert_eq!(
            map_agent_event_to_conversation(
                AgentKind::Claude,
                &AgentEvent::ToolCallStarted {
                    name: "Bash".to_string(),
                    summary_json: "{}".to_string(),
                }
            ),
            ConversationEventMapping::Delta("\n\n_Bash._\n\n".to_string(),),
        );
        assert_eq!(
            map_agent_event_to_conversation(
                AgentKind::Codex,
                &AgentEvent::ToolCallStarted {
                    name: "commandExecution".to_string(),
                    summary_json: r#"{"type":"commandExecution","command":"rg TODO","cwd":"/tmp"}"#
                        .to_string(),
                }
            ),
            ConversationEventMapping::Delta("\n\n_Command Execution: rg TODO._\n\n".to_string()),
        );
        assert_eq!(
            map_agent_event_to_conversation(
                AgentKind::Claude,
                &AgentEvent::ToolCallStarted {
                    name: "Bash".to_string(),
                    summary_json: r#"{"command":"ls -la"}"#.to_string(),
                }
            ),
            ConversationEventMapping::Delta("\n\n_Bash: ls -la._\n\n".to_string()),
        );

        let long_summary = serde_json::json!({ "command": "x".repeat(97) }).to_string();
        assert_eq!(
            map_agent_event_to_conversation(
                AgentKind::Claude,
                &AgentEvent::ToolCallStarted {
                    name: "Bash".to_string(),
                    summary_json: long_summary,
                }
            ),
            ConversationEventMapping::Delta(format!("\n\n_Bash: {}…._\n\n", "x".repeat(96))),
        );
        assert_eq!(
            map_agent_event_to_conversation(
                AgentKind::Claude,
                &AgentEvent::ToolCallStarted {
                    name: "Bash".to_string(),
                    summary_json: serde_json::json!({
                        "command": "printf `hello`\n  world"
                    })
                    .to_string(),
                }
            ),
            ConversationEventMapping::Delta("\n\n_Bash: printf hello world._\n\n".to_string()),
        );
        let usage = TurnUsage {
            input_tokens: 10,
            output_tokens: 2,
            cache_read_tokens: 4,
            cache_creation_tokens: 1,
            total_cost_usd: Some(0.01),
        };
        assert_eq!(
            map_agent_event_to_conversation(
                AgentKind::Claude,
                &AgentEvent::TurnCompleted {
                    usage: usage.clone(),
                }
            ),
            ConversationEventMapping::Completed(usage),
        );
        assert_eq!(
            map_agent_event_to_conversation(
                AgentKind::Claude,
                &AgentEvent::ProtocolError {
                    message: "bad frame".to_string(),
                }
            ),
            ConversationEventMapping::Error("bad frame".to_string()),
        );
    }

    #[test]
    fn claude_code_model_aliases_match_the_frontend_wire_contract() {
        assert_eq!(
            CLAUDE_CODE_MODELS,
            [
                ("fable", "Fable 5"),
                ("opus", "Opus 5"),
                ("sonnet", "Sonnet 5"),
                ("haiku", "Haiku 4.5"),
            ],
        );
        assert!(is_claude_code_model("fable"));
        assert!(is_claude_code_model("sonnet"));
        assert!(is_claude_code_model("claude-fable-5"));
        assert!(is_claude_code_model("claude-sonnet-5-20260801"));
        assert!(!is_claude_code_model("gpt-5.5"));
        assert!(!is_claude_code_model("claude"));
        assert!(!is_claude_code_model("claude-"));
        assert_eq!(
            actionable_agent_error(AgentKind::Claude, "Unknown model: fable".to_string()),
            "Claude Code error: Unknown model: fable",
        );
    }

    #[test]
    fn probe_contract_contains_all_known_adapters() {
        let probes = AgentSessionProbes {
            antigravity: AgentProbe {
                found: false,
                binary: None,
                version: None,
                error: None,
            },
            claude: AgentProbe {
                found: false,
                binary: None,
                version: None,
                error: None,
            },
            codex: AgentProbe {
                found: true,
                binary: Some("/usr/local/bin/codex".to_string()),
                version: Some("codex-cli 0.147.0".to_string()),
                error: None,
            },
            gemini: AgentProbe {
                found: false,
                binary: None,
                version: None,
                error: None,
            },
            kimi: AgentProbe {
                found: false,
                binary: None,
                version: None,
                error: None,
            },
            ghostty: ghostty::GhosttyProbe {
                found: false,
                version: None,
                applescript_ready: false,
            },
        };
        let value = serde_json::to_value(probes).expect("probes should serialize");

        assert!(value.get("antigravity").is_some());
        assert!(value.get("claude").is_some());
        assert_eq!(value["codex"]["found"], true);
        assert_eq!(value["codex"]["version"], "codex-cli 0.147.0");
        assert!(value.get("gemini").is_some());
        assert!(value.get("kimi").is_some());
    }
}
