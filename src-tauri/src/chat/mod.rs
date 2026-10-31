mod turn_diff;

use crate::{
    agent_sessions::{self, AgentKind, AgentSessionManager, SessionCredential, SessionLane},
    app_data,
    auth::{self, AuthBearer},
    browser::{self, BrowserManager},
    exec::{
        self, apply_patch::PatchFileChange, FileSnapshot, FirstWriteAction, OutputCallback,
        ReviewDecision, SandboxMode, ShellCommandRequest, WorkspaceRoots,
    },
    extensions::{self, ExtensionManager},
    ponytail::{self, PonytailMode},
    preferences::{Personality, PreferencesManager},
    pricing::{self, PricingSource},
    projects,
    savings::dollars::{self, PricedSavings, PromptRates, SavedTokens},
    sidecar::{halv_managed_listener_port, ProjectSavingsTotals, SidecarManager, SidecarStatus},
    storage::{self, NewMessage, NewMessageAttachment, Storage, StoredMessage},
};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use chrono::{Local, TimeZone, Utc};
use reqwest::{
    header::{HeaderMap, HeaderName, HeaderValue, ACCEPT, AUTHORIZATION, CONTENT_TYPE, USER_AGENT},
    Client, StatusCode,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
#[cfg(windows)]
use std::os::windows::fs::OpenOptionsExt;
use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex, OnceLock,
    },
    time::{Duration, Instant},
};
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::{oneshot, Semaphore};
use turn_diff::{
    bounded_nonrevertible_diff, changed_file_snapshots, shell_turn_diff,
    snapshot_git_shell_preimages, GitShellPreimages,
};
#[cfg(test)]
use turn_diff::{init_git_repo, run_git};

const CHAT_DELTA_EVENT: &str = "chat_delta";
const DEFAULT_MODEL: &str = "gpt-5.5";
pub(crate) const DEFAULT_EFFORT: &str = "medium";
const DEFAULT_CLAUDE_EFFORT: &str = "high";
const ANTHROPIC_VERSION: &str = "2023-06-01";
const ANTHROPIC_API_URL: &str = "https://api.anthropic.com";
const ORIGINATOR: &str = "codex_cli_rs";
const GOAL_DONE_MARKER: &str = "[HALV_GOAL_DONE]";
const GOAL_CONTINUE_MARKER: &str = "[HALV_GOAL_CONTINUE]";
const GOAL_BLOCKED_MARKER: &str = "[HALV_GOAL_BLOCKED]";
const GOAL_PROTOCOL_INSTRUCTIONS: &str = "Treat the standing goal as your directive: keep working toward it autonomously and do not pause to ask the user what to do next. Turn-ends are gated — if you stop before the goal is fully accomplished, you will automatically be told to continue. When, and only when, the goal is fully accomplished, end your final message with the literal marker [HALV_GOAL_DONE].";
const GOAL_CHAT_MODE_INSTRUCTIONS: &str = "Treat the standing goal as your directive and continue working toward it when it can be advanced in Chat mode without machine actions. If it requires commands, filesystem changes, or other machine actions, state that Agent mode is required and end your response with the literal marker [HALV_GOAL_BLOCKED] to pause automatic continuation instead of looping. Otherwise, turn-ends are gated — if you stop before the goal is fully accomplished, you will automatically be told to continue. When, and only when, the goal is fully accomplished, end your final message with the literal marker [HALV_GOAL_DONE].";
const CHAT_MODE_ENVIRONMENT_INSTRUCTIONS: &str = "You are running inside Halv in Chat only mode without command-execution or filesystem tools. If the user asks you to run commands, create, modify, or delete files, or otherwise act on their machine, do not claim a generic inability and do not paste shell commands for them to run; tell them to bind a project and choose Sandbox or Ask in the chat bar. Full access is also available there but removes the sandbox and requires an explicit warning.";
const AGENT_MODE_ENVIRONMENT_INSTRUCTIONS: &str = "You are running inside Halv in Agent mode with a sandboxed shell in the bound project directory; apply_patch is also available when the current write policy allows it. A controlled Chromium browser is available through browser_open, browser_snapshot, browser_click, browser_type, browser_screenshot, and tab tools; use browser_snapshot refs for interaction and refresh the snapshot after page changes. For multi-step implementation work, use update_plan and keep its checklist current through completion. Native subagent tools are available for genuinely independent, bounded workstreams; follow the applicable AGENTS.md delegation policy.";
const PLAN_MODE_INSTRUCTIONS: &str = "You are in Plan mode. Investigate the request and produce a concrete, decision-complete implementation plan, but do not modify files or perform any other write action. Read-only project inspection and live web search are available when applicable. Use update_plan to keep the pinned checklist current as your understanding changes. Ask the user concise clarifying questions in chat only when a material choice cannot be resolved from the available context. Do not claim implementation is complete, and do not begin implementation until the user explicitly switches out of Plan mode.";
const CRUX_INDEX_GUIDANCE: &str =
    "This project has a crux code index (scip tools): go straight to scip_map for callers, references, impact, and dead-code questions; a plain text search is cheaper for a simple definition lookup.";
const WEB_SEARCH_ENVIRONMENT_INSTRUCTIONS: &str = "Live web search is available in every mode. Use it whenever the user explicitly asks you to search, browse, verify, look something up, or whenever current information is needed. Cite the sources returned by the search tool and never claim that web access is unavailable while the tool is present.";
const REQUEST_CANCEL_POLL: Duration = Duration::from_millis(50);
const CANCEL_AND_REVERT_WAIT_TIMEOUT: Duration = Duration::from_secs(10);
const RESPONSE_HEADERS_TIMEOUT: Duration = Duration::from_secs(120);
const RESPONSE_CHUNK_IDLE_TIMEOUT: Duration = Duration::from_secs(300);
const RESPONSE_RETRY_DELAYS: [Duration; 2] =
    [Duration::from_millis(500), Duration::from_millis(1_500)];
const TOOL_APPROVAL_TIMEOUT: Duration = Duration::from_secs(120);
const SAVINGS_SETTLE_MAX_RETRIES: usize = 6;
const SAVINGS_CANCEL_SETTLE_RETRIES: usize = 2;
const SAVINGS_SETTLE_STABLE_SAMPLES: usize = 3;
const SAVINGS_SETTLE_DELAY: Duration = Duration::from_millis(350);
const SAVINGS_SETTLE_ZERO_GRACE: Duration = Duration::from_millis(1_500);
const SAVINGS_CANCEL_SETTLE_GRACE: Duration = Duration::from_millis(700);
const HEADROOM_PROJECT_HEADER: &str = "x-headroom-project";
const HEADROOM_PROJECT_MAX_CHARS: usize = 128;
const MAX_SUBAGENT_TOOL_CALLS_PER_TURN: usize = 40;
const CRUX_SAVINGS_PER_MESSAGE_CAP: u64 = 20_000;
const REASONING_FRAGMENT_SEPARATOR: &str = "\n\n";
const MAX_SSE_FRAME_BYTES: usize = 2 * 1024 * 1024;
const MAX_STREAM_RAW_BYTES: usize = 32 * 1024 * 1024;
const MAX_STREAM_EVENTS: usize = 100_000;
const MAX_STREAM_ASSISTANT_BYTES: usize = 4 * 1024 * 1024;
const MAX_STREAM_REASONING_BYTES: usize = 1024 * 1024;
const MAX_STREAM_OUTPUT_ITEMS: usize = 128;
const MAX_ERROR_BODY_BYTES: usize = 64 * 1024;
const MAX_ERROR_FALLBACK_CHARS: usize = 200;
const UPSTREAM_ERROR_LOG_FILE_NAME: &str = "upstream-errors.log";
const UPSTREAM_ERROR_LOG_MAX_BYTES: u64 = 256 * 1024;
const UPSTREAM_ERROR_LOG_BODY_MAX_CHARS: usize = 8 * 1024;
const UPSTREAM_ERROR_LOG_TYPE_MAX_CHARS: usize = 160;
static UPSTREAM_ERROR_LOG_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
const MODEL_REGISTRY_CLIENT_VERSION: &str = "0.144.0";
const DEFAULT_FAST_SERVICE_TIER: &str = "priority";
const AGENTS_DOC_MAX_BYTES: usize = 32 * 1024;
const PROJECT_DOC_SEPARATOR: &str = "\n\n--- project-doc ---\n\n";
const FILE_MENTION_MAX_BYTES: usize = 24 * 1024;
const FILE_MENTION_TOTAL_MAX_BYTES: usize = 96 * 1024;
const FILE_MENTION_TRUNCATED_SUFFIX: &str = "\n[... truncated by Halv]\n";
const MAX_IMAGE_ATTACHMENTS_PER_MESSAGE: usize = 8;
const MAX_IMAGE_ATTACHMENT_BYTES: usize = 5 * 1024 * 1024;
const MAX_IMAGE_ATTACHMENTS_TOTAL_BYTES: usize = 20 * 1024 * 1024;
const MAX_IMAGE_ATTACHMENTS_PER_HISTORY: usize = 32;
const MAX_IMAGE_ATTACHMENT_NAME_CHARS: usize = 160;
const MAX_VIEW_IMAGE_BYTES: usize = 5 * 1024 * 1024;
const TITLE_GENERATION_INPUT_CHARS: usize = 480;
const TITLE_GENERATION_MAX_CHARS: usize = 72;
const TITLE_GENERATION_MAX_OUTPUT_TOKENS: i64 = 32;
const TITLE_GENERATION_INSTRUCTIONS: &str =
    "Write a 3-6 word title for this conversation. Reply with the title only.";
const COMPACTION_SUMMARY_HEADING: &str = "Conversation summary (context was compacted)";
const COMPACTION_CONTINUATION: &str = "Continue the conversation seamlessly from this state.";
const COMPACTION_REQUEST: &str = "Produce a thorough handoff summary of the conversation through the last completed turn. Include the key decisions, current task state, file paths touched, unresolved threads, and user preferences needed to continue seamlessly. Incorporate any earlier conversation summary already present. Return only the summary.";
const COMPACTION_INSTRUCTIONS: &str =
    "Write only the requested conversation handoff summary. Do not call tools.";
const MEMORY_GENERATION_INSTRUCTIONS: &str = "Extract at most 3 NEW durable, user-specific facts worth remembering across future sessions, such as stable preferences, project conventions, and explicit decisions. Do not extract temporary task state, generic facts, or anything already present in the supplied memories. Never include credentials, passwords, secrets, tokens, API keys, private keys, or other authentication material, even if the user asks you to remember them. Return only a JSON array of strings. When there is nothing durable to remember, return no facts as the empty JSON array []. Do not call tools.";
const MEMORY_CONTEXT_CAVEAT: &str =
    "Memories are background context that may be outdated; use them cautiously.";
const MEMORY_CONTEXT_LIMIT: usize = 30;
const MEMORY_CONTEXT_MAX_CHARS: usize = 4_000;
const MEMORY_DEDUPE_LIMIT: usize = 100;
const MEMORY_GENERATION_MAX_FACTS: usize = 3;
const MEMORY_GENERATION_MAX_FACT_CHARS: usize = 1_000;
const MEMORY_GENERATION_MAX_OUTPUT_TOKENS: i64 = 512;
const FALLBACK_MODELS: &[&str] = &[
    "gpt-6-astra",
    "gpt-6-sol",
    "gpt-6-luna",
    "gpt-5.6-sol",
    "gpt-5.6-terra",
    "gpt-5.6-luna",
    "gpt-5.6",
    "gpt-5.5",
    "gpt-5.4",
    "gpt-5.4-mini",
    "gpt-5.3-codex",
    "gpt-5.2",
];
const REASONING_EFFORTS: &[&str] = &["low", "medium", "high", "xhigh"];
const CLAUDE_EFFORTS: &[&str] = &["low", "medium", "high", "xhigh", "max"];
const FALLBACK_CLAUDE_MODELS: &[&str] = &["claude-sonnet-5", "claude-opus-4-8", "claude-fable-5"];
const KNOWN_REASONING_EFFORTS: &[&str] = &[
    "none", "minimal", "low", "medium", "high", "xhigh", "max", "ultra",
];
const HIDDEN_MODEL_IDS: &[&str] = &["codex-auto-review"];

type ChatResult<T> = Result<T, ChatError>;

#[derive(Debug)]
struct ChatError {
    message: String,
    partial_text: String,
    retryable: bool,
}

impl ChatError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            partial_text: String::new(),
            retryable: false,
        }
    }

    fn with_partial(message: impl Into<String>, partial_text: String) -> Self {
        Self {
            message: message.into(),
            partial_text,
            retryable: false,
        }
    }

    fn transient(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            partial_text: String::new(),
            retryable: true,
        }
    }

    fn transient_with_partial(message: impl Into<String>, partial_text: String) -> Self {
        Self {
            message: message.into(),
            partial_text,
            retryable: true,
        }
    }
}

impl std::fmt::Display for ChatError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for ChatError {}

#[derive(Debug, Clone)]
struct ModelReasoningMetadata {
    efforts: Vec<String>,
    default_effort: String,
    supports_fast_mode: bool,
    fast_service_tier: String,
}

pub struct ChatManager {
    client: Client,
    in_flight: Mutex<HashMap<String, Arc<InFlightRequest>>>,
    truncating_conversations: Mutex<HashSet<String>>,
    model_reasoning: Mutex<HashMap<String, ModelReasoningMetadata>>,
    pending_tool_decisions: Mutex<HashMap<String, oneshot::Sender<ReviewDecision>>>,
    pending_denied_actions: Mutex<HashMap<String, DeniedToolAction>>,
    last_denied_actions: Mutex<HashMap<String, DeniedToolAction>>,
    approval_cache: exec::ApprovalCache,
    write_guards: Mutex<HashMap<String, WriteGuardState>>,
    workspace_in_flight: Mutex<HashMap<String, String>>,
    subagent_registries: Mutex<HashMap<String, SubagentRegistry>>,
}

#[derive(Debug, Default)]
struct CruxSavingsMeter {
    credited_calls: HashSet<String>,
    tokens_saved: u64,
}

impl CruxSavingsMeter {
    fn record(&mut self, call_id: &str, estimate: u64) {
        if estimate == 0 || !self.credited_calls.insert(call_id.to_string()) {
            return;
        }
        self.tokens_saved = self
            .tokens_saved
            .saturating_add(estimate)
            .min(CRUX_SAVINGS_PER_MESSAGE_CAP);
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct TurnSavings {
    tokens_saved: Option<i64>,
    savings_usd: Option<f64>,
    pricing_source: Option<PricingSource>,
    crux_tokens_saved: i64,
    /// Headroom and RTK tokens inside `tokens_saved`, for the dollar
    /// methods of ADR 0015.
    split: Option<(u64, u64)>,
}

impl TurnSavings {
    fn new(
        savings: (Option<i64>, Option<f64>, Option<PricingSource>),
        crux_tokens_saved: i64,
    ) -> Self {
        Self {
            tokens_saved: savings.0,
            savings_usd: savings.1,
            pricing_source: savings.2,
            crux_tokens_saved,
            split: None,
        }
    }

    fn with_split(mut self, split: Option<(u64, u64)>) -> Self {
        self.split = split;
        self
    }

    /// The dollar value of the turn with the methods of ADR 0015. Headroom
    /// tokens use the prompt price of this request; RTK and crux tokens
    /// count once at the input price.
    fn priced(&self, model: &str, usage: Option<&TokenUsage>) -> Option<PricedSavings> {
        let crux = u64::try_from(self.crux_tokens_saved).unwrap_or_default();
        let tokens = self
            .tokens_saved
            .and_then(|tokens| u64::try_from(tokens).ok())
            .unwrap_or_default();
        if self.tokens_saved.is_none() && crux == 0 {
            return None;
        }
        let (context, rtk) = self.split.unwrap_or((tokens, 0));
        let window_rate = usage.and_then(|usage| {
            let cached = u64::try_from(usage.cached_tokens).unwrap_or_default();
            let input = u64::try_from(usage.input_tokens).unwrap_or_default();
            dollars::usage_prompt_rate(
                None,
                Some(model),
                &pricing::TokenUsage {
                    input: input.saturating_sub(cached),
                    cache_read: cached,
                    ..pricing::TokenUsage::default()
                },
            )
        });
        Some(dollars::price_savings(
            &PromptRates {
                agent_kind: None,
                model: Some(model),
                window_rate,
                running_rate: None,
            },
            SavedTokens { context, rtk, crux },
        ))
    }
}

fn insert_turn_savings_snapshot(
    storage: &Storage,
    message_id: &str,
    model: &str,
    usage: Option<&TokenUsage>,
    savings: &TurnSavings,
) -> Result<(), storage::StorageError> {
    match savings.priced(model, usage) {
        Some(priced) => storage.insert_priced_savings_snapshot(
            message_id,
            savings.tokens_saved.unwrap_or_default(),
            savings.crux_tokens_saved,
            &storage::savings_dollars::SnapshotDollars {
                usd: priced.usd,
                source: priced.source,
                usd_method: priced.method,
                price_version: &priced.price_version,
            },
        ),
        None => storage.insert_savings_snapshot_with_crux(
            message_id,
            savings.tokens_saved,
            savings.savings_usd,
            savings.pricing_source,
            savings.crux_tokens_saved,
        ),
    }
}

#[derive(Debug)]
struct InFlightRequest {
    cancel: Arc<AtomicBool>,
    turn_id: Mutex<Option<String>>,
    finalization_error: Mutex<Option<String>>,
    finished: AtomicBool,
    revert_reserved: AtomicBool,
    workspace_key: Option<String>,
    crux_savings: Mutex<CruxSavingsMeter>,
}

impl InFlightRequest {
    fn new(turn_id: Option<&str>, workspace_key: Option<String>) -> Self {
        Self {
            cancel: Arc::new(AtomicBool::new(false)),
            turn_id: Mutex::new(turn_id.map(ToString::to_string)),
            finalization_error: Mutex::new(None),
            finished: AtomicBool::new(false),
            revert_reserved: AtomicBool::new(false),
            workspace_key,
            crux_savings: Mutex::new(CruxSavingsMeter::default()),
        }
    }

    fn set_turn_id(&self, turn_id: &str) {
        *self.turn_id.lock().expect("turn id lock poisoned") = Some(turn_id.to_string());
    }

    fn turn_id(&self) -> Option<String> {
        self.turn_id.lock().expect("turn id lock poisoned").clone()
    }

    fn set_finalization_error(&self, error: &str) {
        *self
            .finalization_error
            .lock()
            .expect("finalization error lock poisoned") = Some(error.to_string());
    }

    fn finalization_error(&self) -> Option<String> {
        self.finalization_error
            .lock()
            .expect("finalization error lock poisoned")
            .clone()
    }

    fn crux_tokens_saved(&self) -> i64 {
        self.crux_savings
            .lock()
            .expect("Crux savings lock poisoned")
            .tokens_saved as i64
    }
}

impl ChatManager {
    pub fn new() -> Self {
        Self {
            client: Client::new(),
            in_flight: Mutex::new(HashMap::new()),
            truncating_conversations: Mutex::new(HashSet::new()),
            model_reasoning: Mutex::new(HashMap::new()),
            pending_tool_decisions: Mutex::new(HashMap::new()),
            pending_denied_actions: Mutex::new(HashMap::new()),
            last_denied_actions: Mutex::new(HashMap::new()),
            approval_cache: exec::ApprovalCache::default(),
            write_guards: Mutex::new(HashMap::new()),
            workspace_in_flight: Mutex::new(HashMap::new()),
            subagent_registries: Mutex::new(HashMap::new()),
        }
    }

    #[allow(dead_code)]
    fn try_register_request(
        &self,
        conversation_id: &str,
        turn_id: Option<&str>,
    ) -> Result<Arc<InFlightRequest>, String> {
        self.try_register_request_for_workspace(conversation_id, turn_id, None)
    }

    fn try_register_request_for_workspace(
        &self,
        conversation_id: &str,
        turn_id: Option<&str>,
        workspace_root: Option<&str>,
    ) -> Result<Arc<InFlightRequest>, String> {
        let truncating = self
            .truncating_conversations
            .lock()
            .expect("chat truncation lock poisoned");
        if truncating.contains(conversation_id) {
            return Err("Conversation messages are being changed; try again".to_string());
        }
        let mut in_flight = self.in_flight.lock().expect("chat lock poisoned");
        if in_flight.contains_key(conversation_id) {
            return Err("A chat request is already in flight for this conversation".to_string());
        }
        let workspace_key = workspace_root.map(normalize_workspace_lease_key);
        if let Some(workspace_key) = workspace_key.as_ref() {
            let mut workspaces = self
                .workspace_in_flight
                .lock()
                .expect("workspace lease lock poisoned");
            if let Some(owner) = workspaces.get(workspace_key) {
                return Err(format!(
                    "Workspace is busy in another conversation ({owner}). Use a separate worktree for parallel turns."
                ));
            }
            workspaces.insert(workspace_key.clone(), conversation_id.to_string());
        }
        let request = Arc::new(InFlightRequest::new(turn_id, workspace_key));
        in_flight.insert(conversation_id.to_string(), request.clone());
        drop(truncating);
        Ok(request)
    }

    pub(crate) fn is_in_flight(&self, conversation_id: &str) -> bool {
        self.in_flight
            .lock()
            .expect("chat lock poisoned")
            .contains_key(conversation_id)
    }

    pub(crate) fn in_flight_count(&self) -> usize {
        self.in_flight.lock().expect("chat lock poisoned").len()
    }

    fn try_register_revert(&self, conversation_id: &str) -> Result<Arc<InFlightRequest>, String> {
        let mut in_flight = self.in_flight.lock().expect("chat lock poisoned");
        if in_flight.contains_key(conversation_id) {
            return Err("A turn is still running — stop it before reverting".to_string());
        }
        let request = Arc::new(InFlightRequest::new(None, None));
        in_flight.insert(conversation_id.to_string(), request.clone());
        Ok(request)
    }

    fn cancel(&self, conversation_id: &str) -> Option<Arc<InFlightRequest>> {
        let request = self
            .in_flight
            .lock()
            .expect("chat lock poisoned")
            .get(conversation_id)
            .cloned();
        if let Some(request) = request.as_ref() {
            request.cancel.store(true, Ordering::SeqCst);
        }
        request
    }

    fn reserve_message_truncation(
        &self,
        conversation_id: &str,
    ) -> Result<Option<Arc<InFlightRequest>>, String> {
        let mut truncating = self
            .truncating_conversations
            .lock()
            .expect("chat truncation lock poisoned");
        if !truncating.insert(conversation_id.to_string()) {
            return Err("Conversation messages are already being changed".to_string());
        }
        let request = self
            .in_flight
            .lock()
            .expect("chat lock poisoned")
            .get(conversation_id)
            .cloned();
        Ok(request)
    }

    fn release_message_truncation(&self, conversation_id: &str) {
        self.truncating_conversations
            .lock()
            .expect("chat truncation lock poisoned")
            .remove(conversation_id);
    }

    fn record_finalization_error(&self, conversation_id: &str, error: &str) {
        let request = self
            .in_flight
            .lock()
            .expect("chat lock poisoned")
            .get(conversation_id)
            .cloned();
        if let Some(request) = request {
            request.set_finalization_error(error);
        }
    }

    fn reserve_cancel_and_revert(
        &self,
        conversation_id: &str,
    ) -> Result<Option<Arc<InFlightRequest>>, String> {
        let in_flight = self.in_flight.lock().expect("chat lock poisoned");
        let Some(request) = in_flight.get(conversation_id).cloned() else {
            return Ok(None);
        };
        request
            .revert_reserved
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .map_err(|_| "Stop & revert is already in progress for this turn".to_string())?;
        request.cancel.store(true, Ordering::SeqCst);
        Ok(Some(request))
    }

    fn clear_request(&self, conversation_id: &str, request: &Arc<InFlightRequest>) {
        request.finished.store(true, Ordering::SeqCst);
        let mut in_flight = self.in_flight.lock().expect("chat lock poisoned");
        if in_flight
            .get(conversation_id)
            .is_some_and(|current| Arc::ptr_eq(current, request))
            && !request.revert_reserved.load(Ordering::SeqCst)
        {
            in_flight.remove(conversation_id);
            self.release_workspace_lease(conversation_id, request);
        }
    }

    fn release_request(&self, conversation_id: &str, request: &Arc<InFlightRequest>) {
        let mut in_flight = self.in_flight.lock().expect("chat lock poisoned");
        request.revert_reserved.store(false, Ordering::SeqCst);
        if in_flight
            .get(conversation_id)
            .is_some_and(|current| Arc::ptr_eq(current, request))
            && request.finished.load(Ordering::SeqCst)
        {
            in_flight.remove(conversation_id);
            self.release_workspace_lease(conversation_id, request);
        }
    }

    fn set_subagent_registry(&self, conversation_id: &str, registry: SubagentRegistry) {
        self.subagent_registries
            .lock()
            .expect("subagent registry lock poisoned")
            .insert(conversation_id.to_string(), registry);
    }

    fn subagent_registry(&self, conversation_id: &str) -> Option<SubagentRegistry> {
        self.subagent_registries
            .lock()
            .expect("subagent registry lock poisoned")
            .get(conversation_id)
            .cloned()
    }

    fn clear_subagent_registry(&self, conversation_id: &str) {
        self.subagent_registries
            .lock()
            .expect("subagent registry lock poisoned")
            .remove(conversation_id);
    }

    fn record_crux_savings(
        &self,
        conversation_id: &str,
        call_id: &str,
        tool: &extensions::McpToolDescriptor,
    ) {
        let Some(estimate) = extensions::crux_savings_estimate(&tool.server_id, &tool.name) else {
            return;
        };
        let request = self
            .in_flight
            .lock()
            .expect("chat lock poisoned")
            .get(conversation_id)
            .cloned();
        if let Some(request) = request {
            request
                .crux_savings
                .lock()
                .expect("Crux savings lock poisoned")
                .record(call_id, estimate);
        }
    }

    fn release_workspace_lease(&self, conversation_id: &str, request: &InFlightRequest) {
        let Some(workspace_key) = request.workspace_key.as_ref() else {
            return;
        };
        let mut workspaces = self
            .workspace_in_flight
            .lock()
            .expect("workspace lease lock poisoned");
        if workspaces
            .get(workspace_key)
            .is_some_and(|owner| owner == conversation_id)
        {
            workspaces.remove(workspace_key);
        }
    }

    fn register_tool_decision(
        &self,
        call_id: &str,
    ) -> Result<oneshot::Receiver<ReviewDecision>, String> {
        let (sender, receiver) = oneshot::channel();
        let replaced = self
            .pending_tool_decisions
            .lock()
            .expect("tool decisions lock poisoned")
            .insert(call_id.to_string(), sender)
            .is_some();
        if replaced {
            Err(format!("duplicate pending tool call id: {call_id}"))
        } else {
            Ok(receiver)
        }
    }

    fn resolve_tool_decision(&self, call_id: &str, decision: ReviewDecision) -> bool {
        let denied = matches!(&decision, ReviewDecision::Denied);
        let delivered = self
            .pending_tool_decisions
            .lock()
            .expect("tool decisions lock poisoned")
            .remove(call_id)
            .map(|sender| sender.send(decision).is_ok())
            .unwrap_or(false);
        let denied_action = self
            .pending_denied_actions
            .lock()
            .expect("pending denied actions lock poisoned")
            .remove(call_id);
        if delivered {
            if let Some(action) = denied_action {
                if denied {
                    self.last_denied_actions
                        .lock()
                        .expect("last denied actions lock poisoned")
                        .insert(action.conversation_id().to_string(), action);
                }
            }
        }
        delivered
    }

    fn register_denied_action(&self, call_id: &str, action: DeniedToolAction) {
        self.pending_denied_actions
            .lock()
            .expect("pending denied actions lock poisoned")
            .insert(call_id.to_string(), action);
    }

    fn approve_last_denied(&self, conversation_id: &str) -> bool {
        let action = self
            .last_denied_actions
            .lock()
            .expect("last denied actions lock poisoned")
            .remove(conversation_id);
        let Some(action) = action else {
            return false;
        };
        match action {
            DeniedToolAction::Shell {
                conversation_id,
                command,
                cwd,
                approval_key,
                ..
            } => {
                debug_assert_eq!(conversation_id, approval_key.conversation_id);
                debug_assert_eq!(command, approval_key.command);
                debug_assert_eq!(cwd, approval_key.cwd);
                self.approval_cache.approve_for_session(approval_key);
            }
            DeniedToolAction::Patch {
                conversation_id,
                patch,
                cwd,
                approval_keys,
                ..
            } => {
                debug_assert!(!conversation_id.is_empty());
                debug_assert!(!patch.trim().is_empty());
                debug_assert!(cwd.is_absolute());
                self.approval_cache.approve_patch_paths(approval_keys);
            }
        }
        true
    }

    fn clear_tool_decision(&self, call_id: &str) {
        self.pending_tool_decisions
            .lock()
            .expect("tool decisions lock poisoned")
            .remove(call_id);
        self.pending_denied_actions
            .lock()
            .expect("pending denied actions lock poisoned")
            .remove(call_id);
    }
}

impl Default for ChatManager {
    fn default() -> Self {
        Self::new()
    }
}

fn normalize_workspace_lease_key(root: &str) -> String {
    let path = PathBuf::from(root);
    path.canonicalize()
        .unwrap_or(path)
        .to_string_lossy()
        .into_owned()
}

#[derive(Debug, Clone)]
struct WriteGuardState {
    confirmed: bool,
}

#[derive(Debug, Clone)]
enum DeniedToolAction {
    Shell {
        conversation_id: String,
        command: String,
        cwd: String,
        approval_key: exec::ApprovalKey,
    },
    Patch {
        conversation_id: String,
        patch: String,
        cwd: PathBuf,
        approval_keys: Vec<exec::PatchApprovalKey>,
    },
}

impl DeniedToolAction {
    fn conversation_id(&self) -> &str {
        match self {
            Self::Shell {
                conversation_id, ..
            }
            | Self::Patch {
                conversation_id, ..
            } => conversation_id,
        }
    }
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatSendResponse {
    pub conversation_id: String,
    pub assistant_message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<TokenUsage>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub skipped_mentions: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service_tier: Option<String>,
    #[serde(skip_serializing_if = "bool_is_false")]
    pub fast_mode_fallback: bool,
}

fn bool_is_false(value: &bool) -> bool {
    !*value
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageAttachmentInput {
    name: String,
    media_type: String,
    size_bytes: u64,
    data: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ComposerDroppedImage {
    pub name: String,
    pub media_type: String,
    pub size_bytes: u64,
    pub data: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatModel {
    pub id: String,
    pub label: String,
    pub reasoning_efforts: Vec<String>,
    pub default_effort: String,
    pub provider: String,
    pub supports_fast_mode: bool,
    #[serde(skip_serializing)]
    fast_service_tier: String,
}

impl ChatManager {
    fn cache_model_reasoning(&self, models: &[ChatModel]) {
        let mut cached = self
            .model_reasoning
            .lock()
            .expect("model reasoning lock poisoned");
        cached.clear();
        cached.extend(models.iter().map(|model| {
            (
                model.id.clone(),
                ModelReasoningMetadata {
                    efforts: model.reasoning_efforts.clone(),
                    default_effort: model.default_effort.clone(),
                    supports_fast_mode: model.supports_fast_mode,
                    fast_service_tier: model.fast_service_tier.clone(),
                },
            )
        }));
    }

    fn clear_model_reasoning(&self) {
        self.model_reasoning
            .lock()
            .expect("model reasoning lock poisoned")
            .clear();
    }

    fn reasoning_for_model(&self, model: &str) -> Option<ModelReasoningMetadata> {
        self.model_reasoning
            .lock()
            .expect("model reasoning lock poisoned")
            .get(model)
            .cloned()
    }

    fn model_supports_fast_mode(&self, model: &str) -> bool {
        self.reasoning_for_model(model)
            .is_some_and(|metadata| metadata.supports_fast_mode)
    }

    fn fast_service_tier_for_model(&self, model: &str) -> Option<String> {
        self.reasoning_for_model(model)
            .map(|metadata| metadata.fast_service_tier)
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TokenUsage {
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub total_tokens: i64,
    pub cached_tokens: i64,
    pub reasoning_tokens: i64,
}

fn model_context_window_tokens(model: &str) -> i64 {
    match model {
        id if id.starts_with("gpt-5") => 272_000,
        id if id.starts_with("claude-") => 200_000,
        _ => 200_000,
    }
}

fn should_compact(usage: &TokenUsage, model: &str) -> bool {
    usage.input_tokens > model_context_window_tokens(model) * 4 / 5
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CompletedTurnStats {
    count: usize,
    last_message_sequence: Option<i64>,
}

fn completed_turn_stats(messages: &[StoredMessage]) -> CompletedTurnStats {
    let mut pending_user = false;
    let mut count = 0_usize;
    let mut last_message_sequence = None;

    for message in messages {
        match message.role.as_str() {
            "user" if !message.error => pending_user = true,
            "assistant" => {
                if pending_user && !message.error {
                    count += 1;
                    last_message_sequence = Some(message.sequence);
                }
                pending_user = false;
            }
            _ => {}
        }
    }

    CompletedTurnStats {
        count,
        last_message_sequence,
    }
}

fn compacted_summary_message(summary: &str) -> String {
    format!(
        "# {COMPACTION_SUMMARY_HEADING}\n\n{}\n\n{COMPACTION_CONTINUATION}",
        summary.trim()
    )
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct UpdatePlanArgs {
    #[serde(default)]
    explanation: Option<String>,
    plan: Vec<storage::TaskPlanItem>,
}

#[derive(Debug, Deserialize)]
struct ViewImageArgs {
    path: String,
}

#[derive(Debug)]
struct LocalImage {
    path: PathBuf,
    media_type: &'static str,
    data: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ChatDeltaPayload {
    conversation_id: String,
    kind: ChatDeltaKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    usage: Option<TokenUsage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    goal_signal: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    call_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    command: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cwd: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    justification: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tail: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    exit_code: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    wall_time_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    truncated_output: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    rtk_route: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    file_changes: Option<Vec<PatchFileChange>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    diff: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    turn_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    revertible: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    branch: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    warning: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    elicitation_mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    elicitation_schema: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    elicitation_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    approval_policy: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    explanation: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    plan: Option<Vec<storage::TaskPlanItem>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    compaction: Option<storage::ConversationCompaction>,
}

impl ChatDeltaPayload {
    fn new(conversation_id: &str, kind: ChatDeltaKind) -> Self {
        Self {
            conversation_id: conversation_id.to_string(),
            kind,
            text: None,
            usage: None,
            goal_signal: None,
            call_id: None,
            command: None,
            tool_kind: None,
            cwd: None,
            status: None,
            justification: None,
            tail: None,
            exit_code: None,
            wall_time_ms: None,
            truncated_output: None,
            rtk_route: None,
            file_changes: None,
            diff: None,
            turn_id: None,
            revertible: None,
            branch: None,
            warning: None,
            elicitation_mode: None,
            elicitation_schema: None,
            elicitation_url: None,
            approval_policy: None,
            explanation: None,
            plan: None,
            compaction: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum ChatDeltaKind {
    Delta,
    Reasoning,
    Done,
    Error,
    ToolCall,
    ToolOutput,
    ToolResult,
    TurnDiff,
    BranchStatus,
    ConversationUpdated,
    TaskList,
    ContextCompacted,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct SavingsPoint {
    project: ProjectSavingsTotals,
    rtk_tokens_saved: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GuiHeadroomProjectScope {
    Turn,
    Background,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct GuiHeadroomProject {
    key: String,
    scope: GuiHeadroomProjectScope,
}

#[derive(Debug)]
struct StreamSuccess {
    assistant_text: String,
    usage: Option<TokenUsage>,
    service_tier: Option<String>,
    fast_mode_fallback: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ProjectPromptContext {
    environment_context: String,
    agents_instructions: Option<String>,
}

#[derive(Debug, Default)]
struct ReferencedFilesContext {
    context: Option<String>,
    skipped_mentions: Vec<String>,
}

struct ConversationRuntimeContext {
    goal: Option<String>,
    project: Option<storage::Project>,
    agent_mode_enabled: bool,
    plan_mode: bool,
    effective_policy: exec::ApprovalPolicy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct LiveTurnPermissions {
    agent_mode_enabled: bool,
    effective_policy: exec::ApprovalPolicy,
}

struct PreparedChatTurn {
    conversation_id: String,
    turn_id: String,
    messages: Vec<StoredMessage>,
    model: String,
    effort: String,
    goal: Option<String>,
    project_root: Option<String>,
    agent_mode_enabled: bool,
    plan_mode: bool,
    ponytail_mode: PonytailMode,
    project_context: Option<ProjectPromptContext>,
    referenced_files_context: Option<String>,
    skipped_mentions: Vec<String>,
}

trait ChatDeltaEmitter {
    fn emit_chat_delta(&self, payload: ChatDeltaPayload) -> Result<(), String>;
}

impl<R: tauri::Runtime> ChatDeltaEmitter for AppHandle<R> {
    fn emit_chat_delta(&self, payload: ChatDeltaPayload) -> Result<(), String> {
        self.emit(CHAT_DELTA_EVENT, payload)
            .map_err(|error| format!("failed to emit chat_delta: {error}"))
    }
}

#[derive(Debug, Deserialize)]
struct UsageWire {
    #[serde(default)]
    input_tokens: i64,
    #[serde(default)]
    input_tokens_details: Option<InputTokensDetailsWire>,
    #[serde(default)]
    output_tokens: i64,
    #[serde(default)]
    output_tokens_details: Option<OutputTokensDetailsWire>,
    #[serde(default)]
    total_tokens: i64,
}

#[derive(Debug, Deserialize)]
struct InputTokensDetailsWire {
    #[serde(default)]
    cached_tokens: i64,
}

#[derive(Debug, Deserialize)]
struct OutputTokensDetailsWire {
    #[serde(default)]
    reasoning_tokens: i64,
}

impl From<UsageWire> for TokenUsage {
    fn from(usage: UsageWire) -> Self {
        Self {
            input_tokens: usage.input_tokens,
            output_tokens: usage.output_tokens,
            total_tokens: usage.total_tokens,
            cached_tokens: usage
                .input_tokens_details
                .map(|details| details.cached_tokens)
                .unwrap_or_default(),
            reasoning_tokens: usage
                .output_tokens_details
                .map(|details| details.reasoning_tokens)
                .unwrap_or_default(),
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
enum ParsedSseEvent {
    Delta(String),
    Reasoning(String),
    ReasoningBoundary,
    FinalMessage {
        text: String,
        item: Value,
    },
    OutputItem(Value),
    WebSearchCall {
        call_id: String,
        command: String,
        status: String,
        item: Option<Value>,
        tail: Option<String>,
    },
    ToolCall(ToolCallEvent),
    Completed {
        usage: Option<TokenUsage>,
        service_tier: Option<String>,
    },
    Failed(String),
    Ignored,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ToolCallEvent {
    call_id: String,
    name: String,
    input: String,
    kind: ToolCallKind,
    item: Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ToolCallKind {
    Function,
    Custom,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ToolInvocation {
    Model,
    DirectUser,
}

#[derive(Clone)]
enum SubagentBackend {
    Responses { headers: HeaderMap },
    Claude { api_key: String },
}

#[derive(Clone)]
struct SubagentSpawnContext {
    app: AppHandle,
    client: Client,
    sidecar: SidecarManager,
    extensions: ExtensionManager,
    conversation_id: String,
    turn_id: String,
    headroom_project: String,
    project_root: String,
    model: String,
    credential_profile_id: Option<String>,
    effort: String,
    write_slug_basis: String,
    backend: SubagentBackend,
    mcp_tools: Vec<extensions::McpToolDescriptor>,
    parent_cancel: Arc<AtomicBool>,
    base_context: Vec<Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SubagentStatus {
    Pending,
    Running,
    Completed,
    Errored,
    Interrupted,
}

impl SubagentStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Errored => "errored",
            Self::Interrupted => "interrupted",
        }
    }

    fn is_final(self) -> bool {
        matches!(self, Self::Completed | Self::Errored | Self::Interrupted)
    }

    fn from_storage(value: &str) -> Self {
        match value {
            "pending" => Self::Pending,
            "running" => Self::Running,
            "completed" => Self::Completed,
            "errored" => Self::Errored,
            "interrupted" => Self::Interrupted,
            _ => Self::Errored,
        }
    }
}

#[derive(Debug, Clone)]
struct SubagentRecord {
    id: String,
    task_name: String,
    last_message: String,
    status: SubagentStatus,
    result: Option<String>,
    error: Option<String>,
    mailbox: Vec<String>,
    delivered: bool,
    cancel: Arc<AtomicBool>,
    generation: u64,
}

#[derive(Clone)]
struct SubagentRegistry {
    context: SubagentSpawnContext,
    records: Arc<Mutex<HashMap<String, SubagentRecord>>>,
    parent_context: Arc<Mutex<Vec<Value>>>,
    semaphore: Option<Arc<Semaphore>>,
    tool_semaphore: Arc<Semaphore>,
    usage: Arc<Mutex<Option<TokenUsage>>>,
}

#[derive(Debug, Deserialize)]
struct SpawnAgentArgs {
    task_name: String,
    message: String,
    #[serde(default)]
    fork_turns: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AgentMessageArgs {
    target: String,
    message: String,
}

#[derive(Debug, Deserialize)]
struct AgentTargetArgs {
    target: String,
}

#[derive(Debug, Default, Deserialize)]
struct WaitAgentArgs {
    #[serde(default)]
    timeout_ms: Option<u64>,
}

const DEFAULT_AGENT_WAIT_MS: u64 = 3_600_000;

impl WaitAgentArgs {
    fn timeout(&self) -> ChatResult<Duration> {
        let timeout_ms = self.timeout_ms.unwrap_or(DEFAULT_AGENT_WAIT_MS);
        if !(10_000..=DEFAULT_AGENT_WAIT_MS).contains(&timeout_ms) {
            return Err(ChatError::new(
                "wait_agent timeout_ms must be between 10000 and 3600000",
            ));
        }
        Ok(Duration::from_millis(timeout_ms))
    }
}

#[derive(Debug, Default, Deserialize)]
struct ListAgentsArgs {
    #[serde(default)]
    path_prefix: Option<String>,
}

impl ToolInvocation {
    fn requires_agent_mode(self) -> bool {
        matches!(self, Self::Model)
    }
}

#[derive(Debug, Default)]
struct SseParser {
    buffer: Vec<u8>,
}

impl SseParser {
    fn push(&mut self, chunk: &[u8], max_events: usize) -> ChatResult<Vec<ParsedSseEvent>> {
        self.buffer.extend_from_slice(chunk);
        let mut events = Vec::new();
        let mut consumed = 0_usize;

        while let Some((relative_index, width)) = find_sse_delimiter(&self.buffer[consumed..]) {
            let frame_end = consumed.saturating_add(relative_index);
            if frame_end.saturating_sub(consumed) > MAX_SSE_FRAME_BYTES {
                return Err(ChatError::new(format!(
                    "SSE frame exceeds the {} MiB safety limit",
                    MAX_SSE_FRAME_BYTES / (1024 * 1024)
                )));
            }
            let frame = &self.buffer[consumed..frame_end];
            consumed = frame_end.saturating_add(width);

            if frame.is_empty() {
                continue;
            }

            let frame = std::str::from_utf8(frame)
                .map_err(|error| ChatError::new(format!("invalid UTF-8 in SSE frame: {error}")))?;
            if let Some(event) = parse_sse_frame(frame)? {
                if events.len() >= max_events {
                    return Err(ChatError::new("SSE event count exceeded the safety limit"));
                }
                events.push(event);
            }
        }

        if consumed > 0 {
            self.buffer.drain(..consumed);
        }
        if self.buffer.len() > MAX_SSE_FRAME_BYTES {
            return Err(ChatError::new(format!(
                "incomplete SSE frame exceeds the {} MiB safety limit",
                MAX_SSE_FRAME_BYTES / (1024 * 1024)
            )));
        }

        Ok(events)
    }

    fn is_empty(&self) -> bool {
        self.buffer.is_empty()
    }
}

#[derive(Debug, Default)]
struct StreamBudget {
    raw_bytes: usize,
    events: usize,
    assistant_bytes: usize,
    reasoning_bytes: usize,
    output_items: usize,
}

impl StreamBudget {
    fn charge_raw(&mut self, bytes: usize) -> ChatResult<()> {
        charge_stream_limit(
            &mut self.raw_bytes,
            bytes,
            MAX_STREAM_RAW_BYTES,
            "raw SSE bytes",
        )
    }

    fn remaining_events(&self) -> usize {
        MAX_STREAM_EVENTS.saturating_sub(self.events)
    }

    fn charge_events(&mut self, count: usize) -> ChatResult<()> {
        charge_stream_limit(&mut self.events, count, MAX_STREAM_EVENTS, "SSE events")
    }

    fn charge_assistant(&mut self, text: &str) -> ChatResult<()> {
        charge_stream_limit(
            &mut self.assistant_bytes,
            text.len(),
            MAX_STREAM_ASSISTANT_BYTES,
            "assistant text",
        )
    }

    fn charge_reasoning(&mut self, text: &str) -> ChatResult<()> {
        charge_stream_limit(
            &mut self.reasoning_bytes,
            text.len(),
            MAX_STREAM_REASONING_BYTES,
            "reasoning text",
        )
    }

    fn charge_output_item(&mut self) -> ChatResult<()> {
        charge_stream_limit(
            &mut self.output_items,
            1,
            MAX_STREAM_OUTPUT_ITEMS,
            "response output items",
        )
    }
}

fn request_stream_budget() -> StreamBudget {
    StreamBudget::default()
}

#[derive(Debug, Default)]
struct HardToolBudget {
    tool_calls: usize,
}

impl HardToolBudget {
    fn charge_tool_call(&mut self) -> ChatResult<()> {
        charge_limit(
            &mut self.tool_calls,
            1,
            MAX_SUBAGENT_TOOL_CALLS_PER_TURN,
            "tool calls",
            "per-turn",
        )
    }
}

enum ToolBudget<'a> {
    Unlimited,
    Hard(&'a mut HardToolBudget),
}

impl ToolBudget<'_> {
    fn charge_tool_call(&mut self) -> ChatResult<()> {
        match self {
            Self::Unlimited => Ok(()),
            Self::Hard(budget) => budget.charge_tool_call(),
        }
    }
}

fn charge_stream_limit(
    current: &mut usize,
    amount: usize,
    limit: usize,
    label: &str,
) -> ChatResult<()> {
    charge_limit(current, amount, limit, label, "per-request")
}

fn charge_limit(
    current: &mut usize,
    amount: usize,
    limit: usize,
    label: &str,
    scope: &str,
) -> ChatResult<()> {
    let next = current
        .checked_add(amount)
        .ok_or_else(|| ChatError::new(format!("{label} counter overflowed")))?;
    if next > limit {
        return Err(ChatError::new(format!(
            "{label} exceeded the {scope} safety limit"
        )));
    }
    *current = next;
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ConversationBackendRoute {
    Native,
    ClaudeSession {
        model: String,
    },
    AgentSession {
        backend: ConversationAgentBackend,
        model: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConversationAgentBackend {
    Claude,
    Codex,
    Kimi,
    Zai,
    Grok,
}

fn bare_model_id(model: &str) -> &str {
    model
        .rsplit_once('/')
        .map(|(_, model)| model)
        .unwrap_or(model)
}

impl ConversationAgentBackend {
    fn from_storage(value: &str) -> Option<Self> {
        match value {
            "claude_session" => Some(Self::Claude),
            "codex_session" => Some(Self::Codex),
            "kimi_session" => Some(Self::Kimi),
            "zai_session" => Some(Self::Zai),
            "grok_session" => Some(Self::Grok),
            _ => None,
        }
    }

    fn storage_name(self) -> &'static str {
        match self {
            Self::Claude => "claude_session",
            Self::Codex => "codex_session",
            Self::Kimi => "kimi_session",
            Self::Zai => "zai_session",
            Self::Grok => "grok_session",
        }
    }

    fn accepts_model(self, model: &str) -> bool {
        let model = bare_model_id(model);
        match self {
            Self::Claude => agent_sessions::is_claude_code_model(model),
            Self::Codex => model.starts_with("gpt-"),
            Self::Kimi => model == "kimi-for-coding",
            Self::Zai => matches!(model, "sonnet" | "opus" | "haiku"),
            Self::Grok => matches!(model, "sonnet" | "opus" | "haiku"),
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Claude => "Claude Code",
            Self::Codex => "Codex CLI",
            Self::Kimi => "Kimi Code",
            Self::Zai => "GLM Coding Plan",
            Self::Grok => "Grok",
        }
    }
}

fn conversation_backend_route(
    stored_backend: Option<&str>,
    stored_model: Option<&str>,
    requested_model: &str,
    requested_backend: Option<&str>,
) -> Result<ConversationBackendRoute, String> {
    if let Some(backend) = stored_backend {
        ConversationAgentBackend::from_storage(backend)
            .ok_or_else(|| format!("unsupported conversation agent backend `{backend}`"))?;
    }
    let requested = requested_backend
        .map(|backend| {
            ConversationAgentBackend::from_storage(backend)
                .ok_or_else(|| format!("unsupported conversation agent backend `{backend}`"))
        })
        .transpose()?
        .or_else(|| {
            agent_sessions::is_claude_code_model(requested_model)
                .then_some(ConversationAgentBackend::Claude)
        });
    if let Some(backend) = requested.filter(|backend| *backend != ConversationAgentBackend::Claude)
    {
        let model = if backend.accepts_model(requested_model) {
            bare_model_id(requested_model)
        } else {
            return Err(format!(
                "unsupported {} model `{requested_model}`",
                backend.label()
            ));
        };
        return Ok(ConversationBackendRoute::AgentSession {
            backend,
            model: model.to_string(),
        });
    }
    let requested_claude = requested == Some(ConversationAgentBackend::Claude);
    match stored_backend {
        Some("claude_session") if requested_claude => {
            let model = if agent_sessions::is_claude_code_model(requested_model) {
                requested_model
            } else {
                stored_model.unwrap_or_default()
            };
            if !agent_sessions::is_claude_code_model(model) {
                return Err(format!("unsupported Claude Code model `{model}`"));
            }
            Ok(ConversationBackendRoute::ClaudeSession {
                model: model.to_string(),
            })
        }
        Some("claude_session") => Ok(ConversationBackendRoute::Native),
        Some("codex_session" | "kimi_session" | "zai_session" | "grok_session") => {
            Ok(ConversationBackendRoute::Native)
        }
        Some(backend) => Err(format!(
            "unsupported conversation agent backend `{backend}`"
        )),
        None if requested_claude => {
            if !agent_sessions::is_claude_code_model(requested_model) {
                return Err(format!("unsupported Claude Code model `{requested_model}`"));
            }
            Ok(ConversationBackendRoute::ClaudeSession {
                model: requested_model.to_string(),
            })
        }
        None => Ok(ConversationBackendRoute::Native),
    }
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn chat_send(
    app: AppHandle,
    chat: tauri::State<'_, ChatManager>,
    storage: tauri::State<'_, Storage>,
    sidecar: tauri::State<'_, SidecarManager>,
    extensions: tauri::State<'_, ExtensionManager>,
    agent_sessions: tauri::State<'_, AgentSessionManager>,
    conversation_id: Option<String>,
    text: String,
    model: String,
    effort: String,
    agent_session_effort: Option<String>,
    project_id: Option<i64>,
    workspace_id: Option<String>,
    agent_mode: Option<bool>,
    approval_policy: Option<String>,
    workflow_mode: Option<String>,
    fast_mode: Option<bool>,
    credential_profile_id: Option<String>,
    user_initiated: Option<bool>,
    mentions: Option<Vec<String>>,
    attachments: Option<Vec<ImageAttachmentInput>>,
    agent_backend: Option<String>,
    conversation_mode: Option<String>,
) -> Result<ChatSendResponse, String> {
    // Keep the IPC field for compatibility. Sends no longer use a local counter.
    let _ = user_initiated;
    let text = text.trim().to_string();
    let attachments = normalize_image_attachments(attachments.unwrap_or_default())?;
    if text.is_empty() && attachments.is_empty() {
        return Err("Message needs text or at least one image".to_string());
    }

    let mentions = normalize_mentions(mentions.unwrap_or_default());
    let model = normalize_model(&model);
    let model_reasoning = chat.reasoning_for_model(&model);
    let effort = normalize_effort(&effort, model_reasoning.as_ref())?;
    let creating_conversation = conversation_id.is_none();
    let conversation_id = storage
        .ensure_conversation_in_workspace(
            conversation_id,
            &text,
            &model,
            project_id,
            workspace_id.as_deref(),
        )
        .map_err(|error| error.to_string())?;
    if creating_conversation {
        storage
            .set_conversation_mode(
                &conversation_id,
                conversation_mode.as_deref().unwrap_or("chat"),
            )
            .map_err(|error| error.to_string())?;
    }
    let _ = emit_delta(
        &app,
        ChatDeltaPayload::new(&conversation_id, ChatDeltaKind::ConversationUpdated),
    );
    apply_requested_agent_mode(storage.inner(), &conversation_id, agent_mode)?;
    apply_requested_approval_policy(
        storage.inner(),
        &conversation_id,
        approval_policy.as_deref(),
    )?;
    apply_requested_workflow_mode(storage.inner(), &conversation_id, workflow_mode.as_deref())?;
    apply_requested_fast_mode(storage.inner(), &conversation_id, fast_mode)?;
    if let Some(credential_profile_id) = credential_profile_id {
        storage
            .set_conversation_credential_profile(&conversation_id, Some(credential_profile_id))
            .map_err(|error| error.to_string())?;
    }
    let runtime = load_conversation_runtime(
        storage.inner(),
        app.state::<PreferencesManager>().inner(),
        &conversation_id,
    )?;
    let (stored_backend, stored_agent_model) = storage
        .conversation_agent_backend(&conversation_id)
        .map_err(|error| error.to_string())?;
    let backend_route = conversation_backend_route(
        stored_backend.as_deref(),
        stored_agent_model.as_deref(),
        &model,
        agent_backend.as_deref(),
    )?;
    match backend_route {
        ConversationBackendRoute::ClaudeSession {
            model: claude_model,
        } => {
            storage
                .set_conversation_agent_backend(
                    &conversation_id,
                    Some("claude_session"),
                    Some(&claude_model),
                )
                .map_err(|error| error.to_string())?;
            let _ = emit_delta(
                &app,
                ChatDeltaPayload::new(&conversation_id, ChatDeltaKind::ConversationUpdated),
            );
            return run_agent_session_chat_turn(
                &app,
                chat.inner(),
                agent_sessions.inner(),
                storage.inner(),
                sidecar.inner(),
                AgentKind::Claude,
                SessionLane::ClaudeSubscription,
                &conversation_id,
                &text,
                &claude_model,
                agent_session_effort.as_deref(),
                runtime.effective_policy,
                true,
                &mentions,
                !attachments.is_empty(),
                runtime.project.as_ref(),
            )
            .await;
        }
        ConversationBackendRoute::AgentSession { backend, model } => {
            let (agent_kind, lane) =
                conversation_agent_lane(storage.inner(), &conversation_id, backend)?;
            storage
                .set_conversation_agent_backend(
                    &conversation_id,
                    Some(backend.storage_name()),
                    Some(&model),
                )
                .map_err(|error| error.to_string())?;
            let _ = emit_delta(
                &app,
                ChatDeltaPayload::new(&conversation_id, ChatDeltaKind::ConversationUpdated),
            );
            return run_agent_session_chat_turn(
                &app,
                chat.inner(),
                agent_sessions.inner(),
                storage.inner(),
                sidecar.inner(),
                agent_kind,
                lane,
                &conversation_id,
                &text,
                &model,
                match agent_kind {
                    AgentKind::Claude => agent_session_effort.as_deref(),
                    AgentKind::Antigravity
                    | AgentKind::Codex
                    | AgentKind::Gemini
                    | AgentKind::Grok
                    | AgentKind::Kimi
                    | AgentKind::Zai => None,
                },
                runtime.effective_policy,
                true,
                &mentions,
                !attachments.is_empty(),
                runtime.project.as_ref(),
            )
            .await;
        }
        ConversationBackendRoute::Native if stored_backend.is_some() => {
            storage
                .set_conversation_agent_backend(&conversation_id, None, None)
                .map_err(|error| error.to_string())?;
        }
        ConversationBackendRoute::Native => {}
    }
    let request = chat.try_register_request_for_workspace(
        &conversation_id,
        None,
        runtime
            .project
            .as_ref()
            .map(|project| project.root_path.as_str()),
    )?;
    let prior_messages = match storage.get_messages(&conversation_id) {
        Ok(messages) => messages,
        Err(error) => {
            chat.clear_request(&conversation_id, &request);
            return Err(error.to_string());
        }
    };
    if let Err(error) = validate_image_attachment_history(&prior_messages)
        .and_then(|()| validate_new_attachments_against_history(&prior_messages, &attachments))
    {
        chat.clear_request(&conversation_id, &request);
        return Err(error);
    }
    let user_message = match storage.insert_message_with_attachments(
        NewMessage {
            conversation_id: &conversation_id,
            role: "user",
            content: &text,
            model: Some(&model),
            input_tokens: None,
            output_tokens: None,
            error: false,
            mentions: (!mentions.is_empty()).then_some(mentions.as_slice()),
        },
        &attachments,
    ) {
        Ok(message) => message,
        Err(error) => {
            chat.clear_request(&conversation_id, &request);
            return Err(error.to_string());
        }
    };
    let turn_id = user_message.id.clone();
    request.set_turn_id(&turn_id);
    let messages = match storage.get_messages(&conversation_id) {
        Ok(messages) => messages,
        Err(error) => {
            chat.clear_request(&conversation_id, &request);
            return Err(error.to_string());
        }
    };
    if let Err(error) = validate_image_attachment_history(&messages) {
        chat.clear_request(&conversation_id, &request);
        return Err(error);
    }
    let project_context = match project_prompt_context_for_turn(&runtime, &messages) {
        Ok(context) => context,
        Err(error) => {
            chat.clear_request(&conversation_id, &request);
            return Err(error);
        }
    };
    let referenced_files_context = build_referenced_files_context(
        runtime
            .project
            .as_ref()
            .map(|project| Path::new(&project.root_path)),
        Some(&mentions),
    );
    let selected_skill_context = extensions::selected_skill_instructions(
        extensions.inner(),
        runtime
            .project
            .as_ref()
            .map(|project| Path::new(&project.root_path)),
        runtime
            .project
            .as_ref()
            .and_then(|project| project.trust_level.as_deref())
            == Some("trusted"),
        &text,
    )?;
    run_registered_chat_turn(
        &app,
        chat.inner(),
        storage.inner(),
        &sidecar,
        extensions.inner(),
        PreparedChatTurn {
            conversation_id,
            turn_id,
            messages,
            model,
            effort,
            goal: runtime.goal,
            project_root: runtime.project.map(|project| project.root_path),
            agent_mode_enabled: runtime.agent_mode_enabled,
            plan_mode: runtime.plan_mode,
            ponytail_mode: app.state::<PreferencesManager>().ponytail()?,
            project_context,
            referenced_files_context: merge_turn_contexts(
                referenced_files_context.context,
                selected_skill_context,
            ),
            skipped_mentions: referenced_files_context.skipped_mentions,
        },
        request,
    )
    .await
}

fn conversation_agent_lane(
    storage: &Storage,
    conversation_id: &str,
    backend: ConversationAgentBackend,
) -> Result<(AgentKind, SessionLane), String> {
    match backend {
        ConversationAgentBackend::Claude => {
            Ok((AgentKind::Claude, SessionLane::ClaudeSubscription))
        }
        ConversationAgentBackend::Codex => Ok((
            AgentKind::Codex,
            SessionLane::CodexChatGpt { proxied: true },
        )),
        ConversationAgentBackend::Kimi => {
            let Some(profile_id) =
                conversation_profile_id(storage, conversation_id, "moonshot", false)?
            else {
                return Ok((AgentKind::Kimi, SessionLane::KimiSubscription));
            };
            Ok((
                AgentKind::Kimi,
                SessionLane::KimiApiKey {
                    credential: SessionCredential::CredentialProfile {
                        credential_profile_id: profile_id,
                    },
                },
            ))
        }
        ConversationAgentBackend::Zai => {
            let profile_id = conversation_profile_id(storage, conversation_id, "zai", true)?
                .ok_or_else(|| {
                    "Add an active GLM Coding Plan profile before sending.".to_string()
                })?;
            storage
                .set_conversation_credential_profile(conversation_id, Some(profile_id.clone()))
                .map_err(|error| error.to_string())?;
            Ok((
                AgentKind::Zai,
                SessionLane::ZaiGlm {
                    credential: SessionCredential::CredentialProfile {
                        credential_profile_id: profile_id,
                    },
                    model_overrides: None,
                },
            ))
        }
        ConversationAgentBackend::Grok => {
            let profile_id = conversation_profile_id(storage, conversation_id, "xai", true)?
                .ok_or_else(|| "Add an active Grok (xAI) profile before sending.".to_string())?;
            storage
                .set_conversation_credential_profile(conversation_id, Some(profile_id.clone()))
                .map_err(|error| error.to_string())?;
            Ok((
                AgentKind::Grok,
                SessionLane::GrokApiKey {
                    credential: SessionCredential::CredentialProfile {
                        credential_profile_id: profile_id,
                    },
                    model_overrides: None,
                },
            ))
        }
    }
}

fn conversation_profile_id(
    storage: &Storage,
    conversation_id: &str,
    provider: &str,
    required: bool,
) -> Result<Option<String>, String> {
    if let Some(profile_id) = storage
        .conversation_credential_profile(conversation_id)
        .map_err(|error| error.to_string())?
    {
        let profile = storage
            .get_credential_profile(&profile_id)
            .map_err(|error| error.to_string())?
            .ok_or_else(|| format!("credential profile `{profile_id}` was not found"))?;
        if profile.provider != provider || profile.status != "active" {
            return Err(format!(
                "credential profile `{profile_id}` is not an active {provider} profile"
            ));
        }
        return Ok(Some(profile_id));
    }
    if !required {
        return Ok(None);
    }
    Ok(storage
        .list_credential_profiles()
        .map_err(|error| error.to_string())?
        .into_iter()
        .find(|profile| profile.provider == provider && profile.status == "active")
        .map(|profile| profile.id))
}

#[allow(clippy::too_many_arguments)]
async fn run_agent_session_chat_turn(
    app: &AppHandle,
    chat: &ChatManager,
    agent_sessions: &AgentSessionManager,
    storage: &Storage,
    sidecar: &SidecarManager,
    agent_kind: AgentKind,
    lane: SessionLane,
    conversation_id: &str,
    text: &str,
    model: &str,
    effort: Option<&str>,
    permission_mode: exec::ApprovalPolicy,
    persist_user_message: bool,
    mentions: &[String],
    has_attachments: bool,
    project: Option<&storage::Project>,
) -> Result<ChatSendResponse, String> {
    if has_attachments {
        return Err(
            "Agent session conversations do not support image attachments yet.".to_string(),
        );
    }
    let project = project.ok_or_else(|| {
        "Choose a project before sending with an agent session model.".to_string()
    })?;
    let summary = storage
        .conversation_summary(conversation_id)
        .map_err(|error| error.to_string())?;
    let ponytail_mode = app.state::<PreferencesManager>().ponytail()?;
    if persist_user_message {
        storage
            .insert_message(NewMessage {
                conversation_id,
                role: "user",
                content: text,
                model: Some(model),
                input_tokens: None,
                output_tokens: None,
                error: false,
                mentions: (!mentions.is_empty()).then_some(mentions),
            })
            .map_err(|error| error.to_string())?;
    }

    let result = agent_sessions
        .send_conversation_message(
            app.clone(),
            storage,
            sidecar,
            agent_kind,
            lane,
            conversation_id,
            &summary.title,
            model,
            effort,
            &project.root_path,
            Some(project.id),
            approval_policy_storage_name(permission_mode),
            ponytail_mode,
            text,
        )
        .await;
    let result = match result {
        Ok(result) => result,
        Err(error) => {
            persist_agent_backend_error(storage, conversation_id, model, &error);
            let mut payload = ChatDeltaPayload::new(conversation_id, ChatDeltaKind::Error);
            payload.text = Some(error.clone());
            let _ = emit_delta(app, payload);
            return Err(error);
        }
    };
    let usage = TokenUsage {
        input_tokens: agent_usage_i64(result.usage.input_tokens),
        output_tokens: agent_usage_i64(result.usage.output_tokens),
        total_tokens: agent_usage_i64(
            result
                .usage
                .input_tokens
                .saturating_add(result.usage.output_tokens),
        ),
        cached_tokens: agent_usage_i64(result.usage.cache_read_tokens),
        reasoning_tokens: 0,
    };
    schedule_title_generation(
        app,
        chat,
        storage,
        sidecar,
        conversation_id,
        model,
        text,
        Some(agent_sessions),
    );
    Ok(ChatSendResponse {
        conversation_id: conversation_id.to_string(),
        assistant_message: result.assistant_message,
        usage: Some(usage),
        skipped_mentions: Vec::new(),
        service_tier: None,
        fast_mode_fallback: false,
    })
}

fn approval_policy_storage_name(policy: exec::ApprovalPolicy) -> &'static str {
    match policy {
        exec::ApprovalPolicy::ReadOnly => "read_only",
        exec::ApprovalPolicy::Ask => "ask",
        exec::ApprovalPolicy::OnRequest => "on_request",
        exec::ApprovalPolicy::OnFailure => "on_failure",
        exec::ApprovalPolicy::Never => "never",
        exec::ApprovalPolicy::SkipAll => "skip_all",
    }
}

fn persist_agent_backend_error(storage: &Storage, conversation_id: &str, model: &str, error: &str) {
    let already_persisted = storage
        .get_messages(conversation_id)
        .ok()
        .and_then(|messages| messages.last().cloned())
        .is_some_and(|message| {
            message.role == "assistant" && message.error && message.content == error
        });
    if already_persisted {
        return;
    }
    let _ = storage.insert_message(NewMessage {
        conversation_id,
        role: "assistant",
        content: error,
        model: Some(model),
        input_tokens: None,
        output_tokens: None,
        error: true,
        mentions: None,
    });
}

fn agent_usage_i64(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn chat_run_shell(
    app: AppHandle,
    chat: tauri::State<'_, ChatManager>,
    storage: tauri::State<'_, Storage>,
    conversation_id: Option<String>,
    command: String,
    model: String,
    project_id: Option<i64>,
    workspace_id: Option<String>,
    agent_mode: Option<bool>,
    approval_policy: Option<String>,
    fast_mode: Option<bool>,
) -> Result<ChatSendResponse, String> {
    let command = command.trim().to_string();
    if command.is_empty() {
        return Err("Shell command cannot be empty".to_string());
    }
    if conversation_id.is_none() && project_id.is_none() {
        return Err("Shell mode requires a selected project".to_string());
    }

    let model = normalize_model(&model);
    let user_content = format!("!{command}");
    let conversation_id = storage
        .ensure_conversation_in_workspace(
            conversation_id,
            &user_content,
            &model,
            project_id,
            workspace_id.as_deref(),
        )
        .map_err(|error| error.to_string())?;
    let _ = emit_delta(
        &app,
        ChatDeltaPayload::new(&conversation_id, ChatDeltaKind::ConversationUpdated),
    );
    apply_requested_agent_mode(storage.inner(), &conversation_id, agent_mode)?;
    apply_requested_approval_policy(
        storage.inner(),
        &conversation_id,
        approval_policy.as_deref(),
    )?;
    apply_requested_fast_mode(storage.inner(), &conversation_id, fast_mode)?;
    let runtime = load_conversation_runtime(
        storage.inner(),
        app.state::<PreferencesManager>().inner(),
        &conversation_id,
    )?;
    if runtime.plan_mode {
        return Err("Direct shell mode is unavailable while Plan mode is active".to_string());
    }
    let project = runtime
        .project
        .ok_or_else(|| "Shell mode requires a project-bound conversation".to_string())?;
    let (turn_id, request) = register_direct_shell_request_in_workspace(
        chat.inner(),
        storage.inner(),
        &conversation_id,
        &user_content,
        &model,
        Some(&project.root_path),
    )?;

    run_registered_shell_turn(
        &app,
        chat.inner(),
        storage.inner(),
        app.state::<PreferencesManager>().inner(),
        &conversation_id,
        &turn_id,
        &project.root_path,
        runtime.effective_policy,
        &user_content,
        command,
        model,
        request,
    )
    .await
}

#[allow(dead_code)]
fn register_direct_shell_request(
    chat: &ChatManager,
    storage: &Storage,
    conversation_id: &str,
    user_content: &str,
    model: &str,
) -> Result<(String, Arc<InFlightRequest>), String> {
    register_direct_shell_request_in_workspace(
        chat,
        storage,
        conversation_id,
        user_content,
        model,
        None,
    )
}

fn register_direct_shell_request_in_workspace(
    chat: &ChatManager,
    storage: &Storage,
    conversation_id: &str,
    user_content: &str,
    model: &str,
    workspace_root: Option<&str>,
) -> Result<(String, Arc<InFlightRequest>), String> {
    let request = chat.try_register_request_for_workspace(conversation_id, None, workspace_root)?;
    let user_message = match storage.insert_message(NewMessage {
        conversation_id,
        role: "user",
        content: user_content,
        model: Some(model),
        input_tokens: None,
        output_tokens: None,
        error: false,
        mentions: None,
    }) {
        Ok(message) => message,
        Err(error) => {
            chat.clear_request(conversation_id, &request);
            return Err(error.to_string());
        }
    };
    request.set_turn_id(&user_message.id);
    Ok((user_message.id, request))
}

fn apply_requested_agent_mode(
    storage: &Storage,
    conversation_id: &str,
    agent_mode: Option<bool>,
) -> Result<(), String> {
    let Some(enabled) = agent_mode else {
        return Ok(());
    };
    if enabled {
        match storage.set_conversation_agent_mode(conversation_id, true) {
            Ok(_) => Ok(()),
            Err(error)
                if error
                    .to_string()
                    .contains("Agent mode requires a project-bound conversation") =>
            {
                storage
                    .set_conversation_agent_mode(conversation_id, false)
                    .map(|_| ())
                    .map_err(|error| error.to_string())
            }
            Err(error) => Err(error.to_string()),
        }
    } else {
        storage
            .set_conversation_agent_mode(conversation_id, false)
            .map(|_| ())
            .map_err(|error| error.to_string())
    }
}

fn apply_requested_workflow_mode(
    storage: &Storage,
    conversation_id: &str,
    workflow_mode: Option<&str>,
) -> Result<(), String> {
    let Some(mode) = workflow_mode else {
        return Ok(());
    };
    storage
        .set_conversation_workflow_mode(conversation_id, mode)
        .map(|_| ())
        .map_err(|error| error.to_string())
}

fn apply_requested_approval_policy(
    storage: &Storage,
    conversation_id: &str,
    approval_policy: Option<&str>,
) -> Result<(), String> {
    let Some(policy) = approval_policy else {
        return Ok(());
    };
    storage
        .set_conversation_approval_policy(conversation_id, Some(policy))
        .map(|_| ())
        .map_err(|error| error.to_string())
}

fn apply_requested_fast_mode(
    storage: &Storage,
    conversation_id: &str,
    fast_mode: Option<bool>,
) -> Result<(), String> {
    let Some(enabled) = fast_mode else {
        return Ok(());
    };
    storage
        .set_conversation_fast_mode(conversation_id, enabled)
        .map(|_| ())
        .map_err(|error| error.to_string())
}

#[allow(clippy::too_many_arguments)]
async fn run_registered_shell_turn<E>(
    app: &E,
    chat: &ChatManager,
    storage: &Storage,
    preferences: &PreferencesManager,
    conversation_id: &str,
    turn_id: &str,
    project_root: &str,
    effective_policy: exec::ApprovalPolicy,
    write_slug_basis: &str,
    command: String,
    model: String,
    request: Arc<InFlightRequest>,
) -> Result<ChatSendResponse, String>
where
    E: ChatDeltaEmitter + Clone + Send + Sync + 'static,
{
    let call = ToolCallEvent {
        call_id: storage::new_uuid_v4(),
        name: "shell_command".to_string(),
        input: json!({ "command": command }).to_string(),
        kind: ToolCallKind::Function,
        item: Value::Null,
    };
    let result = execute_shell_command_call(
        app,
        chat,
        storage,
        preferences,
        conversation_id,
        turn_id,
        project_root,
        effective_policy,
        write_slug_basis,
        call,
        ToolInvocation::DirectUser,
        request.cancel.clone(),
    )
    .await;

    let response = match result {
        Ok(output_item) => match output_item.get("output").and_then(Value::as_str) {
            Some(output) => {
                let assistant_message = direct_shell_message(output);
                let success = StreamSuccess {
                    assistant_text: assistant_message.clone(),
                    usage: None,
                    service_tier: None,
                    fast_mode_fallback: false,
                };
                match persist_success_before_terminal_delta(
                    app,
                    storage,
                    conversation_id,
                    &model,
                    &success,
                    None,
                    None,
                    TurnSavings::default(),
                ) {
                    Ok(()) => Ok(ChatSendResponse {
                        conversation_id: conversation_id.to_string(),
                        assistant_message,
                        usage: None,
                        skipped_mentions: Vec::new(),
                        service_tier: None,
                        fast_mode_fallback: false,
                    }),
                    Err(error) => Err(error),
                }
            }
            None => Err("Shell executor returned an invalid output item".to_string()),
        },
        Err(error) => {
            let message = error.message.clone();
            match persist_error_before_terminal_delta(
                app,
                storage,
                conversation_id,
                &model,
                &error,
                None,
                TurnSavings::default(),
            ) {
                Ok(()) => Err(message),
                Err(error) => Err(error),
            }
        }
    };
    chat.clear_request(conversation_id, &request);
    response
}

fn direct_shell_message(output: &str) -> String {
    let mut current_run = 0_usize;
    let mut longest_run = 0_usize;
    for character in output.chars() {
        if character == '`' {
            current_run += 1;
            longest_run = longest_run.max(current_run);
        } else {
            current_run = 0;
        }
    }
    let fence = "`".repeat(longest_run.saturating_add(1).max(3));
    format!("{fence}text\n{}\n{fence}", output.trim_end())
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn chat_regenerate(
    app: AppHandle,
    chat: tauri::State<'_, ChatManager>,
    storage: tauri::State<'_, Storage>,
    sidecar: tauri::State<'_, SidecarManager>,
    extensions: tauri::State<'_, ExtensionManager>,
    agent_sessions: tauri::State<'_, AgentSessionManager>,
    conversation_id: String,
    model: String,
    effort: String,
    agent_session_effort: Option<String>,
) -> Result<ChatSendResponse, String> {
    let model = normalize_model(&model);
    let model_reasoning = chat.reasoning_for_model(&model);
    let effort = normalize_effort(&effort, model_reasoning.as_ref())?;
    let runtime = load_conversation_runtime(
        storage.inner(),
        app.state::<PreferencesManager>().inner(),
        &conversation_id,
    )?;
    let (stored_backend, stored_agent_model) = storage
        .conversation_agent_backend(&conversation_id)
        .map_err(|error| error.to_string())?;
    if let Some(backend) = stored_backend
        .as_deref()
        .and_then(ConversationAgentBackend::from_storage)
    {
        let session_model = bare_model_id(stored_agent_model.as_deref().unwrap_or(&model));
        if !backend.accepts_model(session_model) {
            return Err(format!(
                "unsupported {} model `{session_model}`",
                backend.label()
            ));
        }
        let messages = storage
            .get_messages(&conversation_id)
            .map_err(|error| error.to_string())?;
        let last_user_message = messages
            .iter()
            .rev()
            .find(|message| message.role == "user")
            .cloned()
            .ok_or_else(|| "Cannot regenerate: no prior user message found".to_string())?;
        if !last_user_message.attachments.is_empty() {
            return Err(
                "Agent session conversations do not support image attachments yet.".to_string(),
            );
        }
        if storage
            .delete_trailing_assistant(&conversation_id)
            .map_err(|error| error.to_string())?
            .is_none()
        {
            return Err(
                "Cannot regenerate: conversation has no trailing assistant response".to_string(),
            );
        }
        let (agent_kind, lane) =
            conversation_agent_lane(storage.inner(), &conversation_id, backend)?;
        return run_agent_session_chat_turn(
            &app,
            chat.inner(),
            agent_sessions.inner(),
            storage.inner(),
            sidecar.inner(),
            agent_kind,
            lane,
            &conversation_id,
            &last_user_message.content,
            session_model,
            match agent_kind {
                AgentKind::Claude => agent_session_effort.as_deref(),
                AgentKind::Antigravity
                | AgentKind::Codex
                | AgentKind::Gemini
                | AgentKind::Grok
                | AgentKind::Kimi
                | AgentKind::Zai => None,
            },
            runtime.effective_policy,
            false,
            last_user_message.mentions.as_deref().unwrap_or(&[]),
            false,
            runtime.project.as_ref(),
        )
        .await;
    }
    let request = chat.try_register_request_for_workspace(
        &conversation_id,
        None,
        runtime
            .project
            .as_ref()
            .map(|project| project.root_path.as_str()),
    )?;
    let user_content = match storage.delete_trailing_assistant(&conversation_id) {
        Ok(Some(content)) => content,
        Ok(None) => {
            chat.clear_request(&conversation_id, &request);
            return Err(
                "Cannot regenerate because the conversation has no trailing assistant response"
                    .to_string(),
            );
        }
        Err(error) => {
            chat.clear_request(&conversation_id, &request);
            return Err(error.to_string());
        }
    };
    let messages = match storage.get_messages(&conversation_id) {
        Ok(messages) => messages,
        Err(error) => {
            chat.clear_request(&conversation_id, &request);
            return Err(error.to_string());
        }
    };
    let Some(last_user_message) = messages.iter().rev().find(|message| message.role == "user")
    else {
        chat.clear_request(&conversation_id, &request);
        return Err("Cannot regenerate because no prior user message was found".to_string());
    };
    if user_content.trim().is_empty() && last_user_message.attachments.is_empty() {
        chat.clear_request(&conversation_id, &request);
        return Err("Cannot regenerate because no prior user message was found".to_string());
    }
    if let Err(error) = validate_image_attachment_history(&messages) {
        chat.clear_request(&conversation_id, &request);
        return Err(error);
    }
    let turn_id = storage::new_uuid_v4();
    request.set_turn_id(&turn_id);
    let referenced_files_context = build_referenced_files_context(
        runtime
            .project
            .as_ref()
            .map(|project| Path::new(&project.root_path)),
        last_user_message.mentions.as_deref(),
    );
    let project_context = match project_prompt_context_for_turn(&runtime, &messages) {
        Ok(context) => context,
        Err(error) => {
            chat.clear_request(&conversation_id, &request);
            return Err(error);
        }
    };
    let selected_skill_context = extensions::selected_skill_instructions(
        extensions.inner(),
        runtime
            .project
            .as_ref()
            .map(|project| Path::new(&project.root_path)),
        runtime
            .project
            .as_ref()
            .and_then(|project| project.trust_level.as_deref())
            == Some("trusted"),
        &user_content,
    )?;

    run_registered_chat_turn(
        &app,
        chat.inner(),
        storage.inner(),
        &sidecar,
        extensions.inner(),
        PreparedChatTurn {
            conversation_id,
            turn_id,
            messages,
            model,
            effort,
            goal: runtime.goal,
            project_root: runtime.project.map(|project| project.root_path),
            agent_mode_enabled: runtime.agent_mode_enabled,
            plan_mode: runtime.plan_mode,
            ponytail_mode: app.state::<PreferencesManager>().ponytail()?,
            project_context,
            referenced_files_context: merge_turn_contexts(
                referenced_files_context.context,
                selected_skill_context,
            ),
            skipped_mentions: referenced_files_context.skipped_mentions,
        },
        request,
    )
    .await
}

fn load_conversation_runtime(
    storage: &Storage,
    preferences: &PreferencesManager,
    conversation_id: &str,
) -> Result<ConversationRuntimeContext, String> {
    let mut agent_mode_enabled = storage
        .conversation_agent_mode(conversation_id)
        .map_err(|error| error.to_string())?;
    let goal = storage
        .conversation_goal(conversation_id)
        .map_err(|error| error.to_string())?;
    let plan_mode = storage
        .conversation_workflow_mode(conversation_id)
        .map_err(|error| error.to_string())?
        == "plan";
    let project = storage
        .project_for_conversation(conversation_id)
        .map_err(|error| error.to_string())?;
    if let Some(project) = project.as_ref() {
        projects::validate_registered_project(project)?;
    }
    let approval_policy_override = storage
        .conversation_approval_policy_override(conversation_id)
        .map_err(|error| error.to_string())?;
    if agent_mode_enabled && project.is_none() {
        storage
            .set_conversation_agent_mode(conversation_id, false)
            .map_err(|error| error.to_string())?;
        agent_mode_enabled = false;
    }
    let effective_policy = match project.as_ref() {
        Some(project) => {
            let stored_policy = approval_policy_override
                .as_deref()
                .or(project.approval_policy.as_deref());
            match stored_policy {
                Some(stored_policy) => exec::ApprovalPolicy::from_storage(
                    Some(stored_policy),
                    project.trust_level.as_deref(),
                ),
                None if project.trust_level.as_deref() == Some("trusted") => preferences
                    .get()?
                    .default_approval_policy
                    .unwrap_or_else(|| {
                        exec::ApprovalPolicy::from_storage(None, project.trust_level.as_deref())
                    }),
                None => exec::ApprovalPolicy::from_storage(None, project.trust_level.as_deref()),
            }
        }
        None => exec::ApprovalPolicy::ReadOnly,
    };

    Ok(ConversationRuntimeContext {
        goal,
        project,
        agent_mode_enabled,
        plan_mode,
        effective_policy,
    })
}

fn load_live_turn_permissions(
    storage: &Storage,
    preferences: &PreferencesManager,
    conversation_id: &str,
    expected_project_root: Option<&str>,
) -> ChatResult<LiveTurnPermissions> {
    let runtime =
        load_conversation_runtime(storage, preferences, conversation_id).map_err(ChatError::new)?;
    let live_project_root = runtime
        .project
        .as_ref()
        .map(|project| project.root_path.as_str());
    if live_project_root != expected_project_root {
        return Err(ChatError::new(
            "The conversation project changed while this turn was running; start a new turn",
        ));
    }

    Ok(LiveTurnPermissions {
        agent_mode_enabled: runtime.agent_mode_enabled,
        effective_policy: runtime.effective_policy,
    })
}

fn live_tool_permissions_match(
    storage: &Storage,
    preferences: &PreferencesManager,
    conversation_id: &str,
    project_root: &str,
    expected_policy: exec::ApprovalPolicy,
    invocation: ToolInvocation,
) -> ChatResult<bool> {
    let permissions =
        load_live_turn_permissions(storage, preferences, conversation_id, Some(project_root))?;
    Ok(
        (permissions.agent_mode_enabled || !invocation.requires_agent_mode())
            && permissions.effective_policy == expected_policy,
    )
}

fn project_prompt_context_for_turn(
    runtime: &ConversationRuntimeContext,
    messages: &[StoredMessage],
) -> Result<Option<ProjectPromptContext>, String> {
    // Inject project context until the conversation has a successful assistant
    // reply — so a first turn that errored still gets context on resend, rather
    // than being suppressed by the count of prior (failed) user turns.
    let first_turn = !messages
        .iter()
        .any(|message| message.role == "assistant" && !message.error);
    if !first_turn {
        return Ok(None);
    }
    runtime
        .project
        .as_ref()
        .map(|project| {
            build_project_prompt_context(
                project,
                runtime.agent_mode_enabled,
                if runtime.plan_mode {
                    exec::ApprovalPolicy::ReadOnly
                } else {
                    runtime.effective_policy
                },
            )
        })
        .transpose()
        .map_err(|error| error.to_string())
}

async fn run_registered_chat_turn(
    app: &AppHandle,
    chat: &ChatManager,
    storage: &Storage,
    sidecar: &SidecarManager,
    extensions: &ExtensionManager,
    turn: PreparedChatTurn,
    request: Arc<InFlightRequest>,
) -> Result<ChatSendResponse, String> {
    let headroom_project = gui_turn_headroom_project(&turn.conversation_id, &turn.turn_id);
    let rtk_database_path = exec::rtk_gui_turn_database_path(&turn.turn_id);
    let before_savings =
        restart_safe_savings_point(sidecar, &headroom_project.key, &rtk_database_path).await;
    let result = stream_chat_response(
        app,
        chat,
        storage,
        sidecar,
        extensions,
        &turn.conversation_id,
        &turn.turn_id,
        &headroom_project.key,
        &turn.messages,
        &turn.model,
        &turn.effort,
        turn.goal.as_deref(),
        turn.project_context.as_ref(),
        turn.referenced_files_context.as_deref(),
        turn.project_root.as_deref(),
        turn.agent_mode_enabled,
        turn.plan_mode,
        turn.ponytail_mode,
        request.cancel.clone(),
    )
    .await;
    chat.clear_subagent_registry(&turn.conversation_id);

    let after_savings = settled_after_savings(
        sidecar,
        &headroom_project.key,
        &rtk_database_path,
        &before_savings,
        request.cancel.clone(),
    )
    .await;

    let response = match result {
        Ok(success) => {
            let goal_protocol = (!turn.plan_mode).then_some(turn.goal.as_deref()).flatten();
            let (assistant_message, _) =
                assistant_text_without_goal_marker(&success.assistant_text, goal_protocol);
            let persist_result = persist_success_before_terminal_delta(
                app,
                storage,
                &turn.conversation_id,
                &turn.model,
                &success,
                goal_protocol,
                Some(turn.ponytail_mode),
                TurnSavings::new(
                    savings_delta_for_model(
                        &headroom_project,
                        before_savings,
                        after_savings,
                        &turn.model,
                    ),
                    request.crux_tokens_saved(),
                )
                .with_split(savings_split(
                    &headroom_project,
                    before_savings,
                    after_savings,
                )),
            );
            match persist_result {
                Ok(()) => {
                    schedule_agent_memory_generation(app, &turn, &assistant_message);
                    if success
                        .usage
                        .as_ref()
                        .is_some_and(|usage| should_compact(usage, &turn.model))
                    {
                        auto_compact_after_completed_turn(
                            app,
                            chat,
                            storage,
                            sidecar,
                            &turn.conversation_id,
                            &turn.model,
                            &turn.effort,
                            request.cancel.clone(),
                        )
                        .await;
                    }
                    schedule_title_generation_if_needed(app, chat, storage, sidecar, &turn);
                    Ok(ChatSendResponse {
                        conversation_id: turn.conversation_id.clone(),
                        assistant_message,
                        usage: success.usage,
                        skipped_mentions: turn.skipped_mentions.clone(),
                        service_tier: success.service_tier,
                        fast_mode_fallback: success.fast_mode_fallback,
                    })
                }
                Err(error) => Err(error),
            }
        }
        Err(error) => {
            let persist_result = persist_error_before_terminal_delta(
                app,
                storage,
                &turn.conversation_id,
                &turn.model,
                &error,
                Some(turn.ponytail_mode),
                TurnSavings::new(
                    savings_delta_for_model(
                        &headroom_project,
                        before_savings,
                        after_savings,
                        &turn.model,
                    ),
                    request.crux_tokens_saved(),
                )
                .with_split(savings_split(
                    &headroom_project,
                    before_savings,
                    after_savings,
                )),
            );
            match persist_result {
                Ok(()) => Err(error.message),
                Err(error) => Err(error),
            }
        }
    };
    chat.clear_request(&turn.conversation_id, &request);
    crate::auth::wake_savings_reporter();
    response
}

#[allow(clippy::too_many_arguments)]
fn persist_success_before_terminal_delta(
    emitter: &impl ChatDeltaEmitter,
    storage: &Storage,
    conversation_id: &str,
    model: &str,
    success: &StreamSuccess,
    active_goal: Option<&str>,
    ponytail_mode: Option<PonytailMode>,
    savings: TurnSavings,
) -> Result<(), String> {
    let (assistant_text, goal_signal) =
        assistant_text_without_goal_marker(&success.assistant_text, active_goal);

    let assistant = storage
        .insert_message_with_ponytail(
            NewMessage {
                conversation_id,
                role: "assistant",
                content: &assistant_text,
                model: Some(model),
                input_tokens: success.usage.as_ref().map(|usage| usage.input_tokens),
                output_tokens: success.usage.as_ref().map(|usage| usage.output_tokens),
                error: false,
                mentions: None,
            },
            ponytail_mode,
        )
        .map_err(|error| error.to_string())?;
    emit_delta(
        emitter,
        ChatDeltaPayload {
            usage: success.usage.clone(),
            goal_signal: goal_signal.map(ToString::to_string),
            ..ChatDeltaPayload::new(conversation_id, ChatDeltaKind::Done)
        },
    )?;
    insert_turn_savings_snapshot(
        storage,
        &assistant.id,
        model,
        success.usage.as_ref(),
        &savings,
    )
    .map_err(|error| error.to_string())?;
    if savings.tokens_saved.is_some_and(|tokens| tokens > 0) || savings.crux_tokens_saved > 0 {
        crate::auth::wake_savings_reporter();
    }
    Ok(())
}

fn persist_error_before_terminal_delta(
    emitter: &impl ChatDeltaEmitter,
    storage: &Storage,
    conversation_id: &str,
    model: &str,
    error: &ChatError,
    ponytail_mode: Option<PonytailMode>,
    savings: TurnSavings,
) -> Result<(), String> {
    let error_text = if error.partial_text.is_empty() {
        error.message.clone()
    } else {
        error.partial_text.clone()
    };

    let assistant = storage
        .insert_message_with_ponytail(
            NewMessage {
                conversation_id,
                role: "assistant",
                content: &error_text,
                model: Some(model),
                input_tokens: None,
                output_tokens: None,
                error: true,
                mentions: None,
            },
            ponytail_mode,
        )
        .map_err(|storage_error| storage_error.to_string())?;
    emit_delta(
        emitter,
        ChatDeltaPayload {
            text: Some(error.message.clone()),
            ..ChatDeltaPayload::new(conversation_id, ChatDeltaKind::Error)
        },
    )?;
    insert_turn_savings_snapshot(storage, &assistant.id, model, None, &savings)
        .map_err(|storage_error| storage_error.to_string())?;
    if savings.tokens_saved.is_some_and(|tokens| tokens > 0) || savings.crux_tokens_saved > 0 {
        crate::auth::wake_savings_reporter();
    }
    Ok(())
}

fn schedule_title_generation_if_needed(
    app: &AppHandle,
    chat: &ChatManager,
    storage: &Storage,
    sidecar: &SidecarManager,
    turn: &PreparedChatTurn,
) {
    let Some(first_user_message) = title_generation_user_message(&turn.messages) else {
        return;
    };
    schedule_title_generation(
        app,
        chat,
        storage,
        sidecar,
        &turn.conversation_id,
        &turn.model,
        &first_user_message,
        None,
    );
}

#[allow(clippy::too_many_arguments)]
fn schedule_title_generation(
    app: &AppHandle,
    chat: &ChatManager,
    storage: &Storage,
    sidecar: &SidecarManager,
    conversation_id: &str,
    model: &str,
    first_user_message: &str,
    agent_sessions: Option<&AgentSessionManager>,
) {
    let Ok(true) = storage.claim_title_generation(conversation_id) else {
        return;
    };
    let Ok(credential_profile_id) = storage.conversation_credential_profile(conversation_id) else {
        return;
    };

    let app = app.clone();
    let client = chat.client.clone();
    let sidecar = sidecar.clone();
    let conversation_id = conversation_id.to_string();
    let model = model.to_string();
    let first_user_message = first_user_message.to_string();
    let agent_sessions = agent_sessions.cloned();
    tauri::async_runtime::spawn(async move {
        let storage = app.state::<Storage>();
        let Ok(title) = request_generated_title(
            &client,
            storage.inner(),
            &sidecar,
            &conversation_id,
            credential_profile_id.as_deref(),
            &model,
            &first_user_message,
        )
        .await
        else {
            return;
        };
        let Ok(Some(_summary)) = storage.set_generated_conversation_title(&conversation_id, &title)
        else {
            return;
        };
        if let Some(agent_sessions) = agent_sessions {
            let _ = agent_sessions.set_ai_vault_title_for_conversation(
                storage.inner(),
                &conversation_id,
                &title,
            );
        }
        let _ = emit_delta(
            &app,
            ChatDeltaPayload::new(&conversation_id, ChatDeltaKind::ConversationUpdated),
        );
    });
}

fn title_generation_user_message(messages: &[StoredMessage]) -> Option<String> {
    if messages
        .iter()
        .any(|message| message.role == "assistant" && !message.error)
    {
        return None;
    }
    messages
        .iter()
        .find(|message| message.role == "user" && !message.error)
        .map(|message| truncate_title_generation_input(&message.content))
        .filter(|message| !message.trim().is_empty())
}

fn schedule_agent_memory_generation(
    app: &AppHandle,
    turn: &PreparedChatTurn,
    assistant_message: &str,
) {
    let recent_user_message = turn
        .messages
        .iter()
        .rev()
        .find(|message| message.role == "user" && !message.error)
        .map(|message| message.content.clone())
        .unwrap_or_default();
    schedule_agent_memory_generation_for_turn(
        app,
        &turn.conversation_id,
        &turn.model,
        &turn.effort,
        &recent_user_message,
        assistant_message,
    );
}

pub(crate) async fn generate_terminal_agent_memories(
    app: &AppHandle,
    session_id: &str,
    kind: &crate::agent_sessions::AgentKind,
    user_message: &str,
    assistant_message: &str,
) -> Result<usize, String> {
    let storage = app.state::<Storage>();
    let source_id = storage
        .conversation_id_for_agent_session(session_id)
        .map_err(|e| e.to_string())?;
    let context_id = source_id
        .clone()
        .unwrap_or_else(|| format!("terminal-memory-{session_id}"));
    let model = if *kind == crate::agent_sessions::AgentKind::Claude {
        "claude-haiku-4-5"
    } else {
        DEFAULT_MODEL
    };
    let Some(context) =
        agent_memory_generation_context(&app.state::<PreferencesManager>(), &storage, &context_id)?
    else {
        return Ok(0);
    };
    let request =
        memory_generation_request(&context.current_memories, user_message, assistant_message);
    let raw = if *kind == crate::agent_sessions::AgentKind::Claude
        && context.credential_profile_id.is_none()
    {
        crate::agent_sessions::memories::request_claude_memories(
            MEMORY_GENERATION_INSTRUCTIONS,
            &request,
        )
        .await?
    } else {
        let chat = app.state::<ChatManager>();
        let sidecar = app.state::<SidecarManager>();
        request_generated_memories(
            app,
            &chat.client,
            &storage,
            &sidecar,
            &context_id,
            context.credential_profile_id.as_deref(),
            model,
            "low",
            &request,
        )
        .await?
    };
    let memories = parse_generated_memories(&raw)?;
    // Recheck consent and scope after the network call as well as before it.
    let current =
        agent_memory_generation_context(&app.state::<PreferencesManager>(), &storage, &context_id)?;
    if current.as_ref().map(|c| c.project_id) != Some(context.project_id) {
        return Ok(0);
    }
    storage
        .insert_agent_memories(context.project_id, &memories, source_id.as_deref())
        .map_err(|e| e.to_string())
}

pub(crate) fn schedule_agent_memory_generation_for_turn(
    app: &AppHandle,
    conversation_id: &str,
    model: &str,
    effort: &str,
    recent_user_message: &str,
    assistant_message: &str,
) {
    let app = app.clone();
    let conversation_id = conversation_id.to_string();
    let model = model.to_string();
    let effort = effort.to_string();
    let recent_user_message = recent_user_message.to_string();
    let assistant_message = assistant_message.to_string();
    tauri::async_runtime::spawn(async move {
        let Some(chat) = app.try_state::<ChatManager>() else {
            eprintln!(
                "Automatic memory generation failed for {conversation_id}: chat state is unavailable"
            );
            return;
        };
        let client = chat.client.clone();
        let Some(sidecar) = app.try_state::<SidecarManager>() else {
            eprintln!(
                "Automatic memory generation failed for {conversation_id}: sidecar state is unavailable"
            );
            return;
        };
        let sidecar = sidecar.inner().clone();
        if let Err(error) = generate_agent_memories_after_turn(
            &app,
            &client,
            &sidecar,
            &conversation_id,
            &model,
            &effort,
            &recent_user_message,
            &assistant_message,
        )
        .await
        {
            eprintln!("Automatic memory generation failed for {conversation_id}: {error}");
        }
    });
}

struct AgentMemoryGenerationContext {
    project_id: Option<i64>,
    current_memories: Vec<storage::AgentMemory>,
    credential_profile_id: Option<String>,
}

fn agent_memory_generation_context(
    preferences: &PreferencesManager,
    storage: &Storage,
    conversation_id: &str,
) -> Result<Option<AgentMemoryGenerationContext>, String> {
    let global_enabled = preferences
        .memories_enabled()
        .map_err(|error| format!("could not read memory preference: {error}"))?;
    let mode = storage
        .conversation_memories_mode(conversation_id)
        .map_err(|error| error.to_string())?;
    if !memories_effectively_enabled(global_enabled, mode.as_deref()) {
        return Ok(None);
    }
    let project_id = storage
        .conversation_project_id(conversation_id)
        .map_err(|error| error.to_string())?;
    let current_memories = storage
        .agent_memories_for_scope(project_id, MEMORY_DEDUPE_LIMIT)
        .map_err(|error| error.to_string())?;
    let credential_profile_id = storage
        .conversation_credential_profile(conversation_id)
        .map_err(|error| error.to_string())?;
    Ok(Some(AgentMemoryGenerationContext {
        project_id,
        current_memories,
        credential_profile_id,
    }))
}

#[allow(clippy::too_many_arguments)]
async fn generate_agent_memories_after_turn(
    app: &AppHandle,
    client: &Client,
    sidecar: &SidecarManager,
    conversation_id: &str,
    model: &str,
    effort: &str,
    recent_user_message: &str,
    recent_assistant_message: &str,
) -> Result<(), String> {
    let storage = app.state::<Storage>();
    let Some(context) = agent_memory_generation_context(
        &app.state::<PreferencesManager>(),
        storage.inner(),
        conversation_id,
    )?
    else {
        return Ok(());
    };

    let request = memory_generation_request(
        &context.current_memories,
        recent_user_message,
        recent_assistant_message,
    );
    let raw = request_generated_memories(
        app,
        client,
        storage.inner(),
        sidecar,
        conversation_id,
        context.credential_profile_id.as_deref(),
        model,
        effort,
        &request,
    )
    .await?;
    let memories = parse_generated_memories(&raw)?;
    if memories.is_empty() {
        eprintln!("[debug] memory generation returned no facts for {conversation_id}");
        return Ok(());
    }
    storage
        .insert_agent_memories(context.project_id, &memories, Some(conversation_id))
        .map_err(|error| error.to_string())?;
    Ok(())
}

fn memory_generation_request(
    current_memories: &[storage::AgentMemory],
    recent_user_message: &str,
    recent_assistant_message: &str,
) -> String {
    json!({
        "current_memories": current_memories
            .iter()
            .map(|memory| memory.content.as_str())
            .collect::<Vec<_>>(),
        "recent_turn": {
            "user": recent_user_message,
            "assistant": recent_assistant_message,
        }
    })
    .to_string()
}

#[allow(clippy::too_many_arguments)]
async fn request_generated_memories(
    app: &AppHandle,
    client: &Client,
    storage: &Storage,
    sidecar: &SidecarManager,
    conversation_id: &str,
    credential_profile_id: Option<&str>,
    model: &str,
    effort: &str,
    request: &str,
) -> Result<String, String> {
    // A memory is a background request. It streams through `proxy_port`,
    // which starts the engine on demand, so check first: a memory never
    // starts an engine that is off.
    if !sidecar.serves_background_requests() {
        return Err(crate::sidecar::ENGINE_OFF_MESSAGE.to_string());
    }
    let cancel = Arc::new(AtomicBool::new(false));
    let headroom_project = gui_background_headroom_project(conversation_id, "memory");
    let response = if is_claude_model(model) {
        request_claude_generated_memories(
            app,
            client,
            storage,
            sidecar,
            conversation_id,
            &headroom_project.key,
            credential_profile_id,
            model,
            effort,
            request,
            cancel,
        )
        .await
    } else {
        request_openai_generated_memories(
            app,
            client,
            storage,
            sidecar,
            conversation_id,
            &headroom_project.key,
            credential_profile_id,
            model,
            effort,
            request,
            cancel,
        )
        .await
    };
    debug_log_excluded_background_savings(sidecar, &headroom_project).await;
    let response = response.map_err(|error| error.message)?;
    Ok(response.trim().to_string())
}

#[allow(clippy::too_many_arguments)]
async fn request_openai_generated_memories(
    app: &AppHandle,
    client: &Client,
    storage: &Storage,
    sidecar: &SidecarManager,
    conversation_id: &str,
    headroom_project: &str,
    credential_profile_id: Option<&str>,
    model: &str,
    effort: &str,
    request: &str,
    cancel: Arc<AtomicBool>,
) -> ChatResult<String> {
    let bearer = resolve_openai_bearer(storage, credential_profile_id)
        .await
        .map_err(|error| ChatError::new(format!("Auth is not ready: {error}")))?;
    let session_id = storage::new_uuid_v4();
    let headers = build_request_headers(&bearer, &session_id, conversation_id)?;
    let mut history = vec![user_input_message(request)];
    let mut body = build_responses_request_body_from_input_with_mcp(
        conversation_id,
        history.clone(),
        model,
        effort,
        None,
        false,
        false,
        exec::ApprovalPolicy::ReadOnly,
        &[],
    );
    if let Some(body) = body.as_object_mut() {
        body.remove("tools");
        body.remove("tool_choice");
        body.remove("parallel_tool_calls");
    }
    body["instructions"] = Value::String(MEMORY_GENERATION_INSTRUCTIONS.to_string());
    body["max_output_tokens"] = json!(MEMORY_GENERATION_MAX_OUTPUT_TOKENS);
    body["client_metadata"] = json!({ "purpose": "agent_memory_generation" });
    let mut budget = StreamBudget::default();
    let mut tool_budget = HardToolBudget::default();
    let mut request_tool_budget = ToolBudget::Hard(&mut tool_budget);
    let mut progress = StreamAttemptProgress::default();
    let response = stream_single_response(
        app,
        client,
        sidecar,
        CapacityCaptureContext::openai(storage, credential_profile_id, model),
        headers,
        headroom_project,
        &body,
        conversation_id,
        &mut history,
        &mut budget,
        &mut request_tool_budget,
        &mut progress,
        false,
        cancel,
    )
    .await?;
    if !response.tool_calls.is_empty() {
        return Err(ChatError::new(
            "Memory generation unexpectedly returned a tool call",
        ));
    }
    Ok(response.assistant_text)
}

#[allow(clippy::too_many_arguments)]
async fn request_claude_generated_memories(
    app: &AppHandle,
    client: &Client,
    storage: &Storage,
    sidecar: &SidecarManager,
    conversation_id: &str,
    headroom_project: &str,
    credential_profile_id: Option<&str>,
    model: &str,
    effort: &str,
    request: &str,
    cancel: Arc<AtomicBool>,
) -> ChatResult<String> {
    let api_key =
        resolve_anthropic_api_key(storage, credential_profile_id).map_err(ChatError::new)?;
    let input = vec![json!({ "role": "user", "content": request })];
    let mut body = build_claude_request_body_for_profile(
        model,
        effort,
        None,
        input,
        false,
        false,
        exec::ApprovalPolicy::ReadOnly,
        &[],
        false,
    );
    if let Some(body) = body.as_object_mut() {
        body.remove("tools");
        body.remove("tool_choice");
    }
    body["system"] = json!([{ "type": "text", "text": MEMORY_GENERATION_INSTRUCTIONS }]);
    body["max_tokens"] = json!(MEMORY_GENERATION_MAX_OUTPUT_TOKENS);
    let mut budget = StreamBudget::default();
    let mut tool_budget = HardToolBudget::default();
    let mut request_tool_budget = ToolBudget::Hard(&mut tool_budget);
    let response = stream_single_claude_response(
        app,
        client,
        sidecar,
        CapacityCaptureContext::anthropic(storage, credential_profile_id, model),
        &api_key,
        headroom_project,
        &body,
        conversation_id,
        &mut budget,
        &mut request_tool_budget,
        false,
        cancel,
    )
    .await?;
    if !response.tool_calls.is_empty() {
        return Err(ChatError::new(
            "Memory generation unexpectedly returned a tool call",
        ));
    }
    Ok(response.assistant_text)
}

fn parse_generated_memories(raw: &str) -> Result<Vec<String>, String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Ok(Vec::new());
    }
    let json = match (raw.find('['), raw.rfind(']')) {
        (Some(start), Some(end)) if start <= end => &raw[start..=end],
        _ => return Err("Memory generation did not return a JSON array".to_string()),
    };
    let values = serde_json::from_str::<Vec<String>>(json)
        .map_err(|error| format!("Memory generation returned invalid JSON: {error}"))?;
    let mut seen = BTreeSet::new();
    let mut memories = Vec::new();
    for value in values {
        let normalized = value.split_whitespace().collect::<Vec<_>>().join(" ");
        if normalized.is_empty() || memory_contains_sensitive_material(&normalized) {
            continue;
        }
        let normalized = normalized
            .chars()
            .take(MEMORY_GENERATION_MAX_FACT_CHARS)
            .collect::<String>();
        if seen.insert(normalized.to_lowercase()) {
            memories.push(normalized);
        }
        if memories.len() == MEMORY_GENERATION_MAX_FACTS {
            break;
        }
    }
    Ok(memories)
}

fn memory_contains_sensitive_material(memory: &str) -> bool {
    let lower = memory.to_lowercase();
    [
        "api key",
        "access token",
        "refresh token",
        "private key",
        "password",
        "credential",
        "bearer ",
        "client secret",
        "auth secret",
        "sk-",
        "ghp_",
        "github_pat_",
        "xoxb-",
        "xoxp-",
    ]
    .iter()
    .any(|marker| lower.contains(marker))
}

async fn request_generated_title(
    client: &Client,
    storage: &Storage,
    sidecar: &SidecarManager,
    conversation_id: &str,
    credential_profile_id: Option<&str>,
    model: &str,
    first_user_message: &str,
) -> Result<String, String> {
    // A title is a background request, so it never starts an engine that is
    // off. A Chat mode title still runs, because its send started the engine.
    if !sidecar.serves_background_requests() {
        return Err(crate::sidecar::ENGINE_OFF_MESSAGE.to_string());
    }
    let headroom_project = gui_background_headroom_project(conversation_id, "title");
    let result = async {
        if is_claude_model(model) {
            return request_generated_claude_title(
                client,
                storage,
                sidecar,
                credential_profile_id,
                model,
                first_user_message,
                &headroom_project.key,
            )
            .await;
        }
        let bearer = resolve_openai_bearer(storage, credential_profile_id).await?;
        let session_id = storage::new_uuid_v4();
        let mut headers = build_request_headers(&bearer, &session_id, conversation_id)
            .map_err(|error| error.to_string())?;
        insert_headroom_project_header(&mut headers, &headroom_project.key)
            .map_err(|error| error.message)?;
        headers.insert(ACCEPT, HeaderValue::from_static("application/json"));
        let body = build_title_generation_request_body(conversation_id, model, first_user_message);
        let port = background_proxy_port(sidecar)
            .await
            .map_err(|error| error.to_string())?;
        let url = format!("http://127.0.0.1:{port}/v1/responses");
        let response = add_headroom_compression_headers(client.post(url).headers(headers))
            .json(&body)
            .send()
            .await
            .map_err(|error| format!("failed to request conversation title: {error}"))?;
        let status = response.status();
        if !status.is_success() {
            return Err(format!("conversation title request returned HTTP {status}"));
        }
        let value = response
            .json::<Value>()
            .await
            .map_err(|error| format!("failed to parse conversation title response: {error}"))?;
        let raw_title = extract_response_text(&value)
            .ok_or_else(|| "conversation title response did not contain text".to_string())?;
        sanitize_generated_title(&raw_title)
            .ok_or_else(|| "conversation title response was empty".to_string())
    }
    .await;
    debug_log_excluded_background_savings(sidecar, &headroom_project).await;
    result
}

async fn request_generated_claude_title(
    client: &Client,
    storage: &Storage,
    sidecar: &SidecarManager,
    credential_profile_id: Option<&str>,
    model: &str,
    first_user_message: &str,
    headroom_project: &str,
) -> Result<String, String> {
    let api_key = resolve_anthropic_api_key(storage, credential_profile_id)?;
    let mut headers = build_claude_request_headers(&api_key).map_err(|error| error.to_string())?;
    insert_headroom_project_header(&mut headers, headroom_project)
        .map_err(|error| error.message)?;
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    let port = background_proxy_port(sidecar)
        .await
        .map_err(|error| error.to_string())?;
    let response = add_headroom_compression_headers(
        client
            .post(format!("http://127.0.0.1:{port}/v1/messages"))
            .headers(headers),
    )
    .json(&json!({
        "model": model,
        "max_tokens": TITLE_GENERATION_MAX_OUTPUT_TOKENS,
        "messages": [{ "role": "user", "content": truncate_title_generation_input(first_user_message) }],
        "system": TITLE_GENERATION_INSTRUCTIONS
    }))
    .send()
    .await
    .map_err(|error| format!("failed to request Claude conversation title: {error}"))?;
    let status = response.status();
    if !status.is_success() {
        return Err(format!("Claude conversation title returned HTTP {status}"));
    }
    let value = response
        .json::<Value>()
        .await
        .map_err(|error| format!("invalid Claude conversation title response: {error}"))?;
    let title = value
        .get("content")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|block| block.get("type").and_then(Value::as_str) == Some("text"))
        .filter_map(|block| block.get("text").and_then(Value::as_str))
        .collect::<String>();
    sanitize_generated_title(&title)
        .ok_or_else(|| "Claude conversation title response was empty".to_string())
}

#[tauri::command]
pub async fn compact_conversation(
    app: AppHandle,
    chat: tauri::State<'_, ChatManager>,
    storage: tauri::State<'_, Storage>,
    sidecar: tauri::State<'_, SidecarManager>,
    conversation_id: String,
    model: String,
    effort: String,
) -> Result<storage::ConversationCompaction, String> {
    let model = normalize_model(&model);
    let model_reasoning = chat.reasoning_for_model(&model);
    let effort = normalize_effort(&effort, model_reasoning.as_ref())?;
    let request = chat
        .try_register_request(&conversation_id, None)
        .map_err(|_| "Cannot compact context while a turn is streaming".to_string())?;
    let result = execute_conversation_compaction(
        &app,
        chat.inner(),
        storage.inner(),
        sidecar.inner(),
        &conversation_id,
        &model,
        &effort,
        request.cancel.clone(),
    )
    .await;
    chat.clear_request(&conversation_id, &request);
    result
}

#[allow(clippy::too_many_arguments)]
async fn execute_conversation_compaction(
    app: &AppHandle,
    chat: &ChatManager,
    storage: &Storage,
    sidecar: &SidecarManager,
    conversation_id: &str,
    model: &str,
    effort: &str,
    cancel: Arc<AtomicBool>,
) -> Result<storage::ConversationCompaction, String> {
    let messages = storage
        .get_messages(conversation_id)
        .map_err(|error| error.to_string())?;
    let completed = completed_turn_stats(&messages);
    if completed.count < 2 {
        return Err("Context compaction needs at least 2 completed turns".to_string());
    }
    let watermark = completed
        .last_message_sequence
        .ok_or_else(|| "No completed turn is available to compact".to_string())?;
    let previous = storage
        .latest_conversation_compaction(conversation_id)
        .map_err(|error| error.to_string())?;
    let source_messages = messages
        .iter()
        .filter(|message| message.sequence <= watermark)
        .cloned()
        .collect::<Vec<_>>();
    let summary = request_compaction_summary(
        app,
        chat,
        storage,
        sidecar,
        conversation_id,
        &source_messages,
        previous.as_ref(),
        model,
        effort,
        cancel,
    )
    .await?;
    let compaction = storage
        .insert_conversation_compaction(conversation_id, watermark, &summary)
        .map_err(|error| error.to_string())?;
    emit_delta(
        app,
        ChatDeltaPayload {
            compaction: Some(compaction.clone()),
            ..ChatDeltaPayload::new(conversation_id, ChatDeltaKind::ContextCompacted)
        },
    )?;
    Ok(compaction)
}

#[allow(clippy::too_many_arguments)]
async fn auto_compact_after_completed_turn(
    app: &AppHandle,
    chat: &ChatManager,
    storage: &Storage,
    sidecar: &SidecarManager,
    conversation_id: &str,
    model: &str,
    effort: &str,
    cancel: Arc<AtomicBool>,
) {
    let result = async {
        let messages = storage
            .get_messages(conversation_id)
            .map_err(|error| error.to_string())?;
        let completed = completed_turn_stats(&messages);
        if completed.count < 2 {
            return Ok(());
        }
        let Some(watermark) = completed.last_message_sequence else {
            return Ok(());
        };
        let previous = storage
            .latest_conversation_compaction(conversation_id)
            .map_err(|error| error.to_string())?;
        if previous
            .as_ref()
            .is_some_and(|compaction| compaction.upto_message_id >= watermark)
        {
            return Ok(());
        }
        execute_conversation_compaction(
            app,
            chat,
            storage,
            sidecar,
            conversation_id,
            model,
            effort,
            cancel,
        )
        .await
        .map(|_| ())
    }
    .await;

    if let Err(error) = result {
        eprintln!("Automatic context compaction failed for {conversation_id}: {error}");
    }
}

#[allow(clippy::too_many_arguments)]
async fn request_compaction_summary(
    app: &AppHandle,
    chat: &ChatManager,
    storage: &Storage,
    sidecar: &SidecarManager,
    conversation_id: &str,
    messages: &[StoredMessage],
    previous: Option<&storage::ConversationCompaction>,
    model: &str,
    effort: &str,
    cancel: Arc<AtomicBool>,
) -> Result<String, String> {
    let credential_profile_id = storage
        .conversation_credential_profile(conversation_id)
        .map_err(|error| error.to_string())?;
    let headroom_project = gui_background_headroom_project(conversation_id, "compaction");
    let summary = if is_claude_model(model) {
        request_claude_compaction_summary(
            app,
            &chat.client,
            storage,
            sidecar,
            conversation_id,
            &headroom_project.key,
            credential_profile_id.as_deref(),
            messages,
            previous,
            model,
            effort,
            cancel,
        )
        .await
    } else {
        request_openai_compaction_summary(
            app,
            &chat.client,
            storage,
            sidecar,
            conversation_id,
            &headroom_project.key,
            credential_profile_id.as_deref(),
            messages,
            previous,
            model,
            effort,
            cancel,
        )
        .await
    };
    debug_log_excluded_background_savings(sidecar, &headroom_project).await;
    let summary = summary.map_err(|error| error.message)?;
    let summary = summary.trim().to_string();
    if summary.is_empty() {
        Err("Context compaction returned an empty summary".to_string())
    } else {
        Ok(summary)
    }
}

#[allow(clippy::too_many_arguments)]
async fn request_openai_compaction_summary(
    app: &AppHandle,
    client: &Client,
    storage: &Storage,
    sidecar: &SidecarManager,
    conversation_id: &str,
    headroom_project: &str,
    credential_profile_id: Option<&str>,
    messages: &[StoredMessage],
    previous: Option<&storage::ConversationCompaction>,
    model: &str,
    effort: &str,
    cancel: Arc<AtomicBool>,
) -> ChatResult<String> {
    let bearer = resolve_openai_bearer(storage, credential_profile_id)
        .await
        .map_err(|error| ChatError::new(format!("Auth is not ready: {error}")))?;
    let session_id = storage::new_uuid_v4();
    let headers = build_request_headers(&bearer, &session_id, conversation_id)?;
    let mut history = build_response_input_items_with_compaction(messages, None, None, previous);
    history.push(user_input_message(COMPACTION_REQUEST));
    let mut body = build_responses_request_body_from_input_with_mcp(
        conversation_id,
        history.clone(),
        model,
        effort,
        None,
        false,
        false,
        exec::ApprovalPolicy::ReadOnly,
        &[],
    );
    if let Some(body) = body.as_object_mut() {
        body.remove("tools");
        body.remove("tool_choice");
        body.remove("parallel_tool_calls");
    }
    body["instructions"] = Value::String(COMPACTION_INSTRUCTIONS.to_string());
    let mut budget = StreamBudget::default();
    let mut tool_budget = HardToolBudget::default();
    let mut request_tool_budget = ToolBudget::Hard(&mut tool_budget);
    let mut progress = StreamAttemptProgress::default();
    let response = stream_single_response(
        app,
        client,
        sidecar,
        CapacityCaptureContext::openai(storage, credential_profile_id, model),
        headers,
        headroom_project,
        &body,
        conversation_id,
        &mut history,
        &mut budget,
        &mut request_tool_budget,
        &mut progress,
        false,
        cancel,
    )
    .await?;
    if !response.tool_calls.is_empty() {
        return Err(ChatError::new(
            "Context compaction unexpectedly returned a tool call",
        ));
    }
    Ok(response.assistant_text)
}

#[allow(clippy::too_many_arguments)]
async fn request_claude_compaction_summary(
    app: &AppHandle,
    client: &Client,
    storage: &Storage,
    sidecar: &SidecarManager,
    conversation_id: &str,
    headroom_project: &str,
    credential_profile_id: Option<&str>,
    messages: &[StoredMessage],
    previous: Option<&storage::ConversationCompaction>,
    model: &str,
    effort: &str,
    cancel: Arc<AtomicBool>,
) -> ChatResult<String> {
    let api_key =
        resolve_anthropic_api_key(storage, credential_profile_id).map_err(ChatError::new)?;
    let mut input = build_claude_messages_with_compaction(messages, None, None, previous);
    input.push(json!({ "role": "user", "content": COMPACTION_REQUEST }));
    let mut body = build_claude_request_body_for_profile(
        model,
        effort,
        None,
        input,
        false,
        false,
        exec::ApprovalPolicy::ReadOnly,
        &[],
        false,
    );
    if let Some(body) = body.as_object_mut() {
        body.remove("tools");
        body.remove("tool_choice");
    }
    body["system"] = json!([{ "type": "text", "text": COMPACTION_INSTRUCTIONS }]);
    let mut budget = StreamBudget::default();
    let mut tool_budget = HardToolBudget::default();
    let mut request_tool_budget = ToolBudget::Hard(&mut tool_budget);
    let response = stream_single_claude_response(
        app,
        client,
        sidecar,
        CapacityCaptureContext::anthropic(storage, credential_profile_id, model),
        &api_key,
        headroom_project,
        &body,
        conversation_id,
        &mut budget,
        &mut request_tool_budget,
        false,
        cancel,
    )
    .await?;
    if !response.tool_calls.is_empty() {
        return Err(ChatError::new(
            "Context compaction unexpectedly returned a tool call",
        ));
    }
    Ok(response.assistant_text)
}

#[tauri::command]
pub async fn chat_cancel(
    chat: tauri::State<'_, ChatManager>,
    conversation_id: String,
) -> Result<(), String> {
    let _ = chat.cancel(&conversation_id);
    Ok(())
}

#[tauri::command]
pub async fn chat_steer(
    app: AppHandle,
    storage: tauri::State<'_, Storage>,
    agent_sessions: tauri::State<'_, AgentSessionManager>,
    conversation_id: String,
    text: String,
    mentions: Option<Vec<String>>,
) -> Result<(), String> {
    let text = text.trim().to_string();
    if text.is_empty() {
        return Err("A steer message needs text".to_string());
    }
    let mentions = normalize_mentions(mentions.unwrap_or_default());
    let (backend, agent_model) = storage
        .conversation_agent_backend(&conversation_id)
        .map_err(|error| error.to_string())?;
    match backend.as_deref() {
        Some("codex_session" | "claude_session" | "zai_session") => {}
        Some("kimi_session") => {
            return Err("Kimi does not support steering an active turn".to_string())
        }
        _ => return Err("This conversation does not support steering".to_string()),
    }

    agent_sessions.steer_conversation_message(&conversation_id, &text)?;
    storage
        .insert_message(NewMessage {
            conversation_id: &conversation_id,
            role: "user",
            content: &text,
            model: agent_model.as_deref(),
            input_tokens: None,
            output_tokens: None,
            error: false,
            mentions: (!mentions.is_empty()).then_some(mentions.as_slice()),
        })
        .map_err(|error| error.to_string())?;
    let _ = emit_delta(
        &app,
        ChatDeltaPayload::new(&conversation_id, ChatDeltaKind::ConversationUpdated),
    );
    Ok(())
}

#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TurnRevertResult {
    reverted_files: usize,
}

#[tauri::command]
pub async fn chat_cancel_and_revert(
    app: AppHandle,
    chat: tauri::State<'_, ChatManager>,
    storage: tauri::State<'_, Storage>,
    conversation_id: String,
) -> Result<TurnRevertResult, String> {
    chat_cancel_and_revert_inner(&app, chat.inner(), storage.inner(), &conversation_id).await
}

async fn chat_cancel_and_revert_inner(
    emitter: &impl ChatDeltaEmitter,
    chat: &ChatManager,
    storage: &Storage,
    conversation_id: &str,
) -> Result<TurnRevertResult, String> {
    let Some(request) = chat.reserve_cancel_and_revert(conversation_id)? else {
        return Err(
            "The turn is no longer running; use Revert this turn for any completed file changes"
                .to_string(),
        );
    };
    let result = match wait_for_request_termination(
        &request,
        "Timed out waiting for stopped turn to finish; no files were reverted",
    )
    .await
    {
        Ok(()) => revert_cancelled_request(emitter, storage, conversation_id, &request).await,
        Err(error) => Err(error),
    };
    chat.release_request(conversation_id, &request);
    result
}

async fn revert_cancelled_request(
    emitter: &impl ChatDeltaEmitter,
    storage: &Storage,
    conversation_id: &str,
    request: &InFlightRequest,
) -> Result<TurnRevertResult, String> {
    if let Some(error) = request.finalization_error() {
        return Err(cancelled_turn_finalization_error(&error));
    }
    let Some(turn_id) = request.turn_id() else {
        return Ok(TurnRevertResult::default());
    };
    let turn_diff = storage
        .list_turn_diffs(conversation_id)
        .map_err(|error| error.to_string())?
        .into_iter()
        .find(|turn_diff| turn_diff.turn_id == turn_id);
    if turn_diff.is_none() {
        return Ok(TurnRevertResult::default());
    }

    chat_revert_turn_inner(emitter, storage, conversation_id, &turn_id).await
}

fn cancelled_turn_finalization_error(error: &str) -> String {
    format!(
        "Turn stopped, but Halv could not finalize its file snapshots: {error}. No files were reverted."
    )
}

async fn wait_for_request_termination(
    request: &InFlightRequest,
    timeout_error: &str,
) -> Result<(), String> {
    tokio::time::timeout(CANCEL_AND_REVERT_WAIT_TIMEOUT, async {
        while !request.finished.load(Ordering::SeqCst) {
            tokio::time::sleep(REQUEST_CANCEL_POLL).await;
        }
    })
    .await
    .map_err(|_| timeout_error.to_string())
}

pub(crate) async fn truncate_conversation_from(
    chat: &ChatManager,
    storage: &Storage,
    conversation_id: &str,
    message_id: &str,
) -> Result<storage::ConversationSummary, storage::StorageTruncateError> {
    let request = chat
        .reserve_message_truncation(conversation_id)
        .map_err(storage_truncate_error)?;
    let result = async {
        if let Some(request) = request {
            wait_for_request_termination(
                &request,
                "Timed out waiting for the turn to finish; no messages were changed",
            )
            .await
            .map_err(storage_truncate_error)?;
        }
        storage
            .delete_messages_from(conversation_id, message_id)
            .map_err(storage::StorageTruncateError::from)?;
        storage
            .conversation_summary(conversation_id)
            .map_err(storage::StorageTruncateError::from)
    }
    .await;
    chat.release_message_truncation(conversation_id);
    result
}

fn storage_truncate_error(message: String) -> storage::StorageTruncateError {
    storage::StorageTruncateError {
        code: storage::StorageTruncateErrorCode::Storage,
        message,
    }
}

#[tauri::command]
pub async fn chat_set_agent_mode(
    storage: tauri::State<'_, Storage>,
    conversation_id: String,
    enabled: bool,
) -> Result<storage::ConversationSummary, String> {
    storage
        .set_conversation_agent_mode(&conversation_id, enabled)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn chat_set_workflow_mode(
    storage: tauri::State<'_, Storage>,
    conversation_id: String,
    mode: String,
) -> Result<storage::ConversationSummary, String> {
    storage
        .set_conversation_workflow_mode(&conversation_id, &mode)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn chat_list_subagents(
    storage: tauri::State<'_, Storage>,
    conversation_id: String,
) -> Result<Vec<storage::SubagentThread>, String> {
    storage
        .list_subagent_threads(&conversation_id)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn chat_interrupt_subagent(
    chat: tauri::State<'_, ChatManager>,
    conversation_id: String,
    agent_id: String,
) -> Result<(), String> {
    let registry = chat
        .subagent_registry(&conversation_id)
        .ok_or_else(|| "No subagents are active for this conversation".to_string())?;
    registry
        .interrupt(AgentTargetArgs { target: agent_id })
        .map(|_| ())
}

#[tauri::command]
pub async fn chat_interrupt_all_subagents(
    chat: tauri::State<'_, ChatManager>,
    conversation_id: String,
) -> Result<usize, String> {
    let registry = chat
        .subagent_registry(&conversation_id)
        .ok_or_else(|| "No subagents are active for this conversation".to_string())?;
    registry.interrupt_all()
}

#[tauri::command]
pub async fn chat_send_subagent_message(
    chat: tauri::State<'_, ChatManager>,
    conversation_id: String,
    agent_id: String,
    message: String,
) -> Result<(), String> {
    let message = message.trim();
    if message.is_empty() {
        return Err("Subagent message cannot be empty".to_string());
    }
    let registry = chat
        .subagent_registry(&conversation_id)
        .ok_or_else(|| "No subagents are active for this conversation".to_string())?;
    registry
        .send_message(AgentMessageArgs {
            target: agent_id,
            message: message.to_string(),
        })
        .map(|_| ())
}

#[tauri::command]
pub async fn chat_tool_decision(
    chat: tauri::State<'_, ChatManager>,
    call_id: String,
    decision: ReviewDecision,
) -> Result<(), String> {
    if chat.resolve_tool_decision(&call_id, decision) {
        Ok(())
    } else {
        Err(format!("No pending tool approval for call id {call_id}"))
    }
}

#[tauri::command]
pub fn chat_approve_last_denied(
    chat: tauri::State<'_, ChatManager>,
    conversation_id: String,
) -> bool {
    chat.approve_last_denied(&conversation_id)
}

#[tauri::command]
pub async fn chat_agent_capabilities() -> Result<exec::AgentCapabilities, String> {
    Ok(exec::agent_capabilities())
}

#[tauri::command]
pub async fn chat_list_turn_diffs(
    storage: tauri::State<'_, Storage>,
    conversation_id: String,
) -> Result<Vec<storage::TurnDiffSummary>, String> {
    storage
        .list_turn_diffs(&conversation_id)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn chat_revert_turn(
    app: AppHandle,
    chat: tauri::State<'_, ChatManager>,
    storage: tauri::State<'_, Storage>,
    conversation_id: String,
    turn_id: String,
) -> Result<(), String> {
    let request = chat.try_register_revert(&conversation_id)?;
    let result = chat_revert_turn_inner(&app, storage.inner(), &conversation_id, &turn_id).await;
    chat.clear_request(&conversation_id, &request);
    result.map(|_| ())
}

async fn chat_revert_turn_inner(
    emitter: &impl ChatDeltaEmitter,
    storage: &Storage,
    conversation_id: &str,
    turn_id: &str,
) -> Result<TurnRevertResult, String> {
    let turn_diff = storage
        .list_turn_diffs(conversation_id)
        .map_err(|error| error.to_string())?
        .into_iter()
        .find(|item| item.turn_id == turn_id);
    let Some(turn_diff) = turn_diff else {
        return Ok(TurnRevertResult::default());
    };
    if !turn_diff.revertible {
        return Err(
            "This turn is not revertible because Halv could not safely capture its preimage"
                .to_string(),
        );
    }
    let snapshots = storage
        .get_turn_snapshots(conversation_id, turn_id)
        .map_err(|error| error.to_string())?;
    if snapshots.is_empty() {
        storage
            .delete_turn(conversation_id, turn_id)
            .map_err(|error| error.to_string())?;
        emit_turn_diff_clear(emitter, conversation_id, turn_id)?;
        return Ok(TurnRevertResult::default());
    }
    ensure_turn_revert_has_no_later_overlap(storage, conversation_id, turn_id, &snapshots)?;
    let cwd = PathBuf::from(turn_diff.cwd);
    let project = storage
        .project_for_conversation(conversation_id)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "Cannot revalidate the project for this turn".to_string())?;
    let project_root = projects::validate_registered_project(&project)?;
    if cwd != project_root {
        return Err("Turn workspace no longer matches the registered project".to_string());
    }
    let mut seen_paths = BTreeSet::new();
    let mut file_snapshots = Vec::new();
    for snapshot in snapshots {
        if !seen_paths.insert(snapshot.absolute_path.clone()) {
            continue;
        }
        file_snapshots.push(FileSnapshot {
            display_path: snapshot.display_path,
            absolute_path: snapshot.absolute_path,
            before: snapshot.before,
            after_exists: snapshot.after_exists,
            after_sha256: snapshot.after_sha256,
        });
    }
    exec::restore_patch_snapshots_sandboxed(cwd, file_snapshots).await?;
    let reverted_files = seen_paths.len();
    storage
        .delete_turn(conversation_id, turn_id)
        .map_err(|error| error.to_string())?;
    emit_turn_diff_clear(emitter, conversation_id, turn_id)?;
    Ok(TurnRevertResult { reverted_files })
}

fn ensure_turn_revert_has_no_later_overlap(
    storage: &Storage,
    conversation_id: &str,
    turn_id: &str,
    snapshots: &[storage::TurnSnapshot],
) -> Result<(), String> {
    let touched_paths = snapshots
        .iter()
        .map(|snapshot| snapshot.absolute_path.clone())
        .collect::<BTreeSet<_>>();
    if touched_paths.is_empty() {
        return Ok(());
    }

    let turns = storage
        .list_turn_diffs(conversation_id)
        .map_err(|error| error.to_string())?;
    let Some(index) = turns.iter().position(|turn| turn.turn_id == turn_id) else {
        return Ok(());
    };

    let mut overlaps = BTreeSet::new();
    for later_turn in turns.iter().skip(index + 1) {
        let later_snapshots = storage
            .get_turn_snapshots(conversation_id, &later_turn.turn_id)
            .map_err(|error| error.to_string())?;
        for snapshot in later_snapshots {
            if touched_paths.contains(&snapshot.absolute_path) {
                overlaps.insert(snapshot.display_path);
            }
        }
    }

    if overlaps.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "Cannot revert this turn because a later turn also touched {}. Revert the later turn first.",
            overlaps.into_iter().collect::<Vec<_>>().join(", ")
        ))
    }
}

#[tauri::command]
pub async fn chat_list_models(
    app: AppHandle,
    chat: tauri::State<'_, ChatManager>,
    storage: tauri::State<'_, Storage>,
    sidecar: tauri::State<'_, SidecarManager>,
) -> Result<Vec<ChatModel>, String> {
    let mut models = Vec::new();
    if let Ok(bearer) = resolve_openai_bearer(storage.inner(), None).await {
        models.extend(
            fetch_openai_models(chat.inner(), sidecar.inner(), &bearer)
                .await
                .unwrap_or_else(|_| fallback_models()),
        );
    }
    if let Ok(api_key) = resolve_anthropic_api_key(storage.inner(), None) {
        models.extend(
            fetch_anthropic_models(&chat.client, &api_key)
                .await
                .unwrap_or_else(|_| fallback_claude_models()),
        );
    }
    let cli_models = crate::agent_sessions::model_catalog::available_models(&app).await;
    models.extend(cli_models.into_iter().map(|model| ChatModel {
        id: model.id,
        label: model.label,
        provider: model.provider,
        reasoning_efforts: model.reasoning_efforts,
        default_effort: model.default_effort,
        supports_fast_mode: false,
        fast_service_tier: "auto".to_string(),
    }));
    if models.is_empty() {
        Ok(uncached_fallback_models(chat.inner()))
    } else {
        chat.cache_model_reasoning(&models);
        Ok(models)
    }
}

async fn fetch_openai_models(
    chat: &ChatManager,
    sidecar: &SidecarManager,
    bearer: &AuthBearer,
) -> Result<Vec<ChatModel>, String> {
    let headroom_project = gui_background_headroom_project("global", "model-registry");
    let session_id = storage::new_uuid_v4();
    let thread_id = storage::new_uuid_v4();
    let mut headers = build_request_headers(bearer, &session_id, &thread_id)
        .map_err(|error| error.to_string())?;
    insert_headroom_project_header(&mut headers, &headroom_project.key)
        .map_err(|error| error.message)?;
    // The app asks for the model list at launch and on each window focus.
    // While the engine is off, this error gives the fallback models.
    let port = background_proxy_port(sidecar)
        .await
        .map_err(|error| error.to_string())?;
    let url =
        format!("http://127.0.0.1:{port}/v1/models?client_version={MODEL_REGISTRY_CLIENT_VERSION}");
    let response = chat
        .client
        .get(url)
        .headers(headers)
        .send()
        .await
        .map_err(|error| error.to_string())?;
    if !response.status().is_success() {
        return Err(format!(
            "model registry returned HTTP {}",
            response.status()
        ));
    }
    let models = parse_model_registry(
        response
            .json::<Value>()
            .await
            .map_err(|error| error.to_string())?,
    );
    let result = (!models.is_empty())
        .then_some(models)
        .ok_or_else(|| "model registry was empty".to_string());
    debug_log_excluded_background_savings(sidecar, &headroom_project).await;
    result
}

async fn fetch_anthropic_models(client: &Client, api_key: &str) -> Result<Vec<ChatModel>, String> {
    let headers = build_claude_request_headers(api_key).map_err(|error| error.to_string())?;
    let response = client
        .get(format!("{ANTHROPIC_API_URL}/v1/models?limit=100"))
        .headers(headers)
        .send()
        .await
        .map_err(|error| format!("Anthropic model request failed: {error}"))?;
    if !response.status().is_success() {
        return Err(format!(
            "Anthropic model request returned HTTP {}",
            response.status()
        ));
    }
    let value = response
        .json::<Value>()
        .await
        .map_err(|error| format!("Anthropic model response was invalid: {error}"))?;
    let models = parse_anthropic_model_registry(value);
    (!models.is_empty())
        .then_some(models)
        .ok_or_else(|| "Anthropic model registry was empty".to_string())
}

#[allow(clippy::too_many_arguments)]
async fn stream_chat_response(
    app: &AppHandle,
    chat: &ChatManager,
    storage: &Storage,
    sidecar: &SidecarManager,
    extensions: &ExtensionManager,
    conversation_id: &str,
    turn_id: &str,
    headroom_project: &str,
    messages: &[StoredMessage],
    model: &str,
    effort: &str,
    goal: Option<&str>,
    project_context: Option<&ProjectPromptContext>,
    referenced_files_context: Option<&str>,
    project_root: Option<&str>,
    agent_mode: bool,
    plan_mode: bool,
    ponytail_mode: PonytailMode,
    cancel: Arc<AtomicBool>,
) -> ChatResult<StreamSuccess> {
    let credential_profile_id = storage
        .conversation_credential_profile(conversation_id)
        .map_err(|error| ChatError::new(error.to_string()))?;
    if is_claude_model(model) {
        return stream_claude_response(
            app,
            chat,
            storage,
            sidecar,
            extensions,
            conversation_id,
            credential_profile_id.as_deref(),
            turn_id,
            headroom_project,
            messages,
            model,
            effort,
            goal,
            project_context,
            referenced_files_context,
            project_root,
            agent_mode,
            plan_mode,
            ponytail_mode,
            cancel,
        )
        .await;
    }
    let bearer = resolve_openai_bearer(storage, credential_profile_id.as_deref())
        .await
        .map_err(|error| ChatError::new(format!("Auth is not ready: {error}")))?;
    let model_supports_fast_mode = chat.model_supports_fast_mode(model);
    let fast_service_tier = chat
        .fast_service_tier_for_model(model)
        .unwrap_or_else(|| DEFAULT_FAST_SERVICE_TIER.to_string());
    let mut fast_mode_active = storage
        .conversation_fast_mode(conversation_id)
        .map_err(|error| ChatError::new(error.to_string()))?
        && bearer_is_chatgpt_subscription(&bearer)
        && model_supports_fast_mode;
    let mut fast_mode_fallback = false;
    let mut service_tier = None;
    let session_id = storage::new_uuid_v4();
    let headers = build_request_headers(&bearer, &session_id, conversation_id)?;
    let compaction = storage
        .latest_conversation_compaction(conversation_id)
        .map_err(|error| ChatError::new(error.to_string()))?;
    let mut history = build_response_input_items_with_compaction(
        messages,
        project_context,
        referenced_files_context,
        compaction.as_ref(),
    );
    let base_context = build_response_input_items(&[], project_context, referenced_files_context);
    let mcp_tools = if agent_mode {
        extensions::enabled_mcp_tools(extensions, project_root.map(Path::new), plan_mode)
            .await
            .map_err(|error| ChatError::new(format!("MCP discovery failed: {error}")))?
    } else {
        Vec::new()
    };
    let personality = app
        .state::<PreferencesManager>()
        .personality()
        .map_err(ChatError::new)?;
    let memory_instructions = memory_instructions_for_conversation(app, storage, conversation_id);
    let mut assistant_text = String::new();
    let mut usage = None;
    let default_cwd = project_root.unwrap_or(".").to_string();
    let write_slug_basis = goal
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .or_else(|| {
            messages
                .iter()
                .find(|message| message.role == "user")
                .map(|message| message.content.clone())
        })
        .unwrap_or_else(|| "agent write".to_string());
    let subagents = SubagentRegistry::new(
        SubagentSpawnContext {
            app: app.clone(),
            client: chat.client.clone(),
            sidecar: sidecar.clone(),
            extensions: extensions.clone(),
            conversation_id: conversation_id.to_string(),
            turn_id: turn_id.to_string(),
            headroom_project: headroom_project.to_string(),
            project_root: default_cwd.clone(),
            model: model.to_string(),
            credential_profile_id: credential_profile_id.clone(),
            effort: effort.to_string(),
            write_slug_basis: write_slug_basis.clone(),
            backend: SubagentBackend::Responses {
                headers: headers.clone(),
            },
            mcp_tools: mcp_tools.clone(),
            parent_cancel: cancel.clone(),
            base_context,
        },
        history.clone(),
        app.state::<PreferencesManager>()
            .subagent_concurrency_limit()
            .map_err(ChatError::new)?,
    )?;
    chat.set_subagent_registry(conversation_id, subagents.clone());

    loop {
        let permissions = load_live_turn_permissions(
            storage,
            app.state::<PreferencesManager>().inner(),
            conversation_id,
            project_root,
        )
        .map_err(|error| prepend_stream_partial(error, &assistant_text))?;
        let live_agent_mode = agent_mode && permissions.agent_mode_enabled;
        let execution_policy = if plan_mode {
            exec::ApprovalPolicy::ReadOnly
        } else {
            permissions.effective_policy
        };
        subagents.update_parent_context(history.clone());
        let mut body = build_responses_request_body_from_input_with_mcp(
            conversation_id,
            history.clone(),
            model,
            effort,
            goal,
            live_agent_mode,
            plan_mode,
            execution_policy,
            &mcp_tools,
        );
        apply_personality_directive(&mut body, personality);
        if let Some(text) = ponytail::instructions(ponytail_mode) {
            apply_instruction_directive(&mut body, &text);
        }
        if let Some(memory_instructions) = memory_instructions.as_deref() {
            apply_instruction_directive(&mut body, memory_instructions);
        }
        if extensions::crux_index_guidance_available(&mcp_tools, project_root.map(Path::new)) {
            apply_instruction_directive(&mut body, CRUX_INDEX_GUIDANCE);
        }
        configure_fast_service_tier(
            &mut body,
            fast_mode_active,
            &bearer,
            model_supports_fast_mode,
            &fast_service_tier,
        );
        let mut retry_index = 0;
        let turn = loop {
            if cancel.load(Ordering::SeqCst) {
                return Err(ChatError::with_partial(
                    "Chat request cancelled",
                    assistant_text.clone(),
                ));
            }

            let history_len = history.len();
            let mut stream_budget = request_stream_budget();
            let mut request_tool_budget = ToolBudget::Unlimited;
            let mut attempt_progress = StreamAttemptProgress::default();
            match stream_single_response(
                app,
                &chat.client,
                sidecar,
                CapacityCaptureContext::openai(storage, credential_profile_id.as_deref(), model),
                headers.clone(),
                headroom_project,
                &body,
                conversation_id,
                &mut history,
                &mut stream_budget,
                &mut request_tool_budget,
                &mut attempt_progress,
                true,
                cancel.clone(),
            )
            .await
            {
                Ok(turn) => break turn,
                Err(error) => {
                    if should_fallback_from_fast_mode(
                        &body,
                        &error,
                        fast_mode_fallback,
                        attempt_progress,
                    ) {
                        storage
                            .set_conversation_fast_mode(conversation_id, false)
                            .map_err(|storage_error| {
                                prepend_stream_partial(
                                    ChatError::new(storage_error.to_string()),
                                    &assistant_text,
                                )
                            })?;
                        body.as_object_mut()
                            .expect("Responses request body should be an object")
                            .remove("service_tier");
                        fast_mode_active = false;
                        fast_mode_fallback = true;
                        history.truncate(history_len);
                        continue;
                    }
                    let Some(delay) = stream_retry_delay(retry_index, &error, attempt_progress)
                    else {
                        return Err(prepend_stream_partial(error, &assistant_text));
                    };
                    history.truncate(history_len);
                    prepare_stream_retry(sidecar, delay, cancel.clone())
                        .await
                        .map_err(|error| prepend_stream_partial(error, &assistant_text))?;
                    retry_index += 1;
                }
            }
        };
        if turn.service_tier.is_some() {
            service_tier = turn.service_tier.clone();
        }
        assistant_text.push_str(&turn.assistant_text);
        merge_token_usage(&mut usage, turn.usage)
            .map_err(|error| ChatError::with_partial(error.message, assistant_text.clone()))?;

        if turn.tool_calls.is_empty() {
            if let Some(updates) = subagents.auto_updates(cancel.clone()).await? {
                history.push(user_input_message(&updates));
                continue;
            }
            merge_token_usage(&mut usage, subagents.take_usage())?;
            return Ok(StreamSuccess {
                assistant_text,
                usage,
                service_tier,
                fast_mode_fallback,
            });
        }

        for call in turn.tool_calls {
            let permissions = load_live_turn_permissions(
                storage,
                app.state::<PreferencesManager>().inner(),
                conversation_id,
                project_root,
            )
            .map_err(|error| prepend_stream_partial(error, &assistant_text))?;
            let plan_tool = plan_mode && call.name == "update_plan";
            if (!agent_mode || !permissions.agent_mode_enabled) && !plan_tool {
                let output =
                    "Agent mode was disabled while this turn was running; tool call was not run.";
                emit_tool_result(
                    app,
                    conversation_id,
                    &call.call_id,
                    "failed",
                    None,
                    None,
                    false,
                )
                .map_err(ChatError::new)
                .map_err(|error| prepend_stream_partial(error, &assistant_text))?;
                history.push(tool_call_output_item(&call, output));
                continue;
            }
            let execution_policy = if plan_mode {
                exec::ApprovalPolicy::ReadOnly
            } else {
                permissions.effective_policy
            };
            let _tool_permit = if plan_tool {
                None
            } else {
                Some(
                    subagents
                        .tool_permit(execution_policy, &call)
                        .await
                        .map_err(|error| prepend_stream_partial(error, &assistant_text))?,
                )
            };
            let output = execute_tool_call(
                app,
                chat,
                storage,
                conversation_id,
                turn_id,
                &default_cwd,
                execution_policy,
                &write_slug_basis,
                extensions,
                &mcp_tools,
                call,
                cancel.clone(),
                Some(&subagents),
            )
            .await
            .map_err(|error| prepend_stream_partial(error, &assistant_text))?;
            history.push(output);
        }
    }
}

#[derive(Debug)]
struct ClaudeSingleResponse {
    assistant_text: String,
    content: Vec<Value>,
    tool_calls: Vec<ToolCallEvent>,
    usage: Option<TokenUsage>,
    stop_reason: Option<String>,
}

#[derive(Debug)]
struct ClaudeContentBlockState {
    block: Value,
    input_json: String,
}

#[derive(Debug, Default)]
struct ClaudeSseParser {
    buffer: Vec<u8>,
}

impl ClaudeSseParser {
    fn push(&mut self, chunk: &[u8], max_events: usize) -> ChatResult<Vec<Value>> {
        self.buffer.extend_from_slice(chunk);
        let mut events = Vec::new();
        let mut consumed = 0_usize;
        while let Some((relative_index, width)) = find_sse_delimiter(&self.buffer[consumed..]) {
            let frame_end = consumed.saturating_add(relative_index);
            if frame_end.saturating_sub(consumed) > MAX_SSE_FRAME_BYTES {
                return Err(ChatError::new("Claude SSE frame exceeded the safety limit"));
            }
            let frame = &self.buffer[consumed..frame_end];
            consumed = frame_end.saturating_add(width);
            if frame.is_empty() {
                continue;
            }
            let frame = std::str::from_utf8(frame)
                .map_err(|error| ChatError::new(format!("invalid Claude SSE UTF-8: {error}")))?;
            if let Some(value) = parse_raw_sse_frame(frame)? {
                if events.len() >= max_events {
                    return Err(ChatError::new(
                        "Claude SSE event count exceeded the safety limit",
                    ));
                }
                events.push(value);
            }
        }
        if consumed > 0 {
            self.buffer.drain(..consumed);
        }
        if self.buffer.len() > MAX_SSE_FRAME_BYTES {
            return Err(ChatError::new(
                "incomplete Claude SSE frame exceeded the safety limit",
            ));
        }
        Ok(events)
    }

    fn is_empty(&self) -> bool {
        self.buffer.is_empty()
    }
}

#[allow(clippy::too_many_arguments)]
async fn stream_claude_response(
    app: &AppHandle,
    chat: &ChatManager,
    storage: &Storage,
    sidecar: &SidecarManager,
    extensions: &ExtensionManager,
    conversation_id: &str,
    credential_profile_id: Option<&str>,
    turn_id: &str,
    headroom_project: &str,
    stored_messages: &[StoredMessage],
    model: &str,
    effort: &str,
    goal: Option<&str>,
    project_context: Option<&ProjectPromptContext>,
    referenced_files_context: Option<&str>,
    project_root: Option<&str>,
    agent_mode: bool,
    plan_mode: bool,
    ponytail_mode: PonytailMode,
    cancel: Arc<AtomicBool>,
) -> ChatResult<StreamSuccess> {
    let api_key =
        resolve_anthropic_api_key(storage, credential_profile_id).map_err(ChatError::new)?;
    let mcp_tools = if agent_mode {
        extensions::enabled_mcp_tools(extensions, project_root.map(Path::new), plan_mode)
            .await
            .map_err(|error| ChatError::new(format!("MCP discovery failed: {error}")))?
    } else {
        Vec::new()
    };
    let personality = app
        .state::<PreferencesManager>()
        .personality()
        .map_err(ChatError::new)?;
    let memory_instructions = memory_instructions_for_conversation(app, storage, conversation_id);
    let compaction = storage
        .latest_conversation_compaction(conversation_id)
        .map_err(|error| ChatError::new(error.to_string()))?;
    let mut messages = build_claude_messages_with_compaction(
        stored_messages,
        project_context,
        referenced_files_context,
        compaction.as_ref(),
    );
    let base_context = build_claude_messages(&[], project_context, referenced_files_context);
    let mut assistant_text = String::new();
    let mut usage = None;
    let default_cwd = project_root.unwrap_or(".").to_string();
    let write_slug_basis = goal
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .or_else(|| {
            stored_messages
                .iter()
                .find(|message| message.role == "user")
                .map(|message| message.content.clone())
        })
        .unwrap_or_else(|| "agent write".to_string());
    let subagents = SubagentRegistry::new(
        SubagentSpawnContext {
            app: app.clone(),
            client: chat.client.clone(),
            sidecar: sidecar.clone(),
            extensions: extensions.clone(),
            conversation_id: conversation_id.to_string(),
            turn_id: turn_id.to_string(),
            headroom_project: headroom_project.to_string(),
            project_root: default_cwd.clone(),
            model: model.to_string(),
            credential_profile_id: credential_profile_id.map(ToString::to_string),
            effort: effort.to_string(),
            write_slug_basis: write_slug_basis.clone(),
            backend: SubagentBackend::Claude {
                api_key: api_key.clone(),
            },
            mcp_tools: mcp_tools.clone(),
            parent_cancel: cancel.clone(),
            base_context,
        },
        messages.clone(),
        app.state::<PreferencesManager>()
            .subagent_concurrency_limit()
            .map_err(ChatError::new)?,
    )?;
    chat.set_subagent_registry(conversation_id, subagents.clone());

    loop {
        let permissions = load_live_turn_permissions(
            storage,
            app.state::<PreferencesManager>().inner(),
            conversation_id,
            project_root,
        )
        .map_err(|error| prepend_stream_partial(error, &assistant_text))?;
        let live_agent_mode = agent_mode && permissions.agent_mode_enabled;
        let execution_policy = if plan_mode {
            exec::ApprovalPolicy::ReadOnly
        } else {
            permissions.effective_policy
        };
        subagents.update_parent_context(messages.clone());
        let mut body = build_claude_request_body(
            model,
            effort,
            goal,
            messages.clone(),
            live_agent_mode,
            plan_mode,
            execution_policy,
            &mcp_tools,
        );
        apply_personality_directive(&mut body, personality);
        if let Some(text) = ponytail::instructions(ponytail_mode) {
            apply_instruction_directive(&mut body, &text);
        }
        if let Some(memory_instructions) = memory_instructions.as_deref() {
            apply_instruction_directive(&mut body, memory_instructions);
        }
        if extensions::crux_index_guidance_available(&mcp_tools, project_root.map(Path::new)) {
            apply_instruction_directive(&mut body, CRUX_INDEX_GUIDANCE);
        }
        let mut stream_budget = request_stream_budget();
        let mut request_tool_budget = ToolBudget::Unlimited;
        let turn = stream_single_claude_response(
            app,
            &chat.client,
            sidecar,
            CapacityCaptureContext::anthropic(storage, credential_profile_id, model),
            &api_key,
            headroom_project,
            &body,
            conversation_id,
            &mut stream_budget,
            &mut request_tool_budget,
            true,
            cancel.clone(),
        )
        .await
        .map_err(|error| prepend_stream_partial(error, &assistant_text))?;
        assistant_text.push_str(&turn.assistant_text);
        merge_token_usage(&mut usage, turn.usage)
            .map_err(|error| ChatError::with_partial(error.message, assistant_text.clone()))?;
        messages.push(json!({ "role": "assistant", "content": turn.content }));

        if turn.stop_reason.as_deref() == Some("pause_turn") && turn.tool_calls.is_empty() {
            continue;
        }
        if turn.tool_calls.is_empty() {
            if let Some(updates) = subagents.auto_updates(cancel.clone()).await? {
                messages.push(json!({ "role": "user", "content": updates }));
                continue;
            }
            merge_token_usage(&mut usage, subagents.take_usage())?;
            return Ok(StreamSuccess {
                assistant_text,
                usage,
                service_tier: None,
                fast_mode_fallback: false,
            });
        }

        let mut results = Vec::new();
        for call in turn.tool_calls {
            let permissions = load_live_turn_permissions(
                storage,
                app.state::<PreferencesManager>().inner(),
                conversation_id,
                project_root,
            )
            .map_err(|error| prepend_stream_partial(error, &assistant_text))?;
            let tool_use_id = call.call_id.clone();
            let plan_tool = plan_mode && call.name == "update_plan";
            let output = if (!agent_mode || !permissions.agent_mode_enabled) && !plan_tool {
                Value::String(
                    "Agent mode was disabled while this turn was running; tool call was not run."
                        .to_string(),
                )
            } else {
                let execution_policy = if plan_mode {
                    exec::ApprovalPolicy::ReadOnly
                } else {
                    permissions.effective_policy
                };
                let _tool_permit = if plan_tool {
                    None
                } else {
                    Some(
                        subagents
                            .tool_permit(execution_policy, &call)
                            .await
                            .map_err(|error| prepend_stream_partial(error, &assistant_text))?,
                    )
                };
                let item = execute_tool_call(
                    app,
                    chat,
                    storage,
                    conversation_id,
                    turn_id,
                    &default_cwd,
                    execution_policy,
                    &write_slug_basis,
                    extensions,
                    &mcp_tools,
                    call,
                    cancel.clone(),
                    Some(&subagents),
                )
                .await
                .map_err(|error| prepend_stream_partial(error, &assistant_text))?;
                tool_output_to_claude_content(&item)
            };
            results.push(json!({
                "type": "tool_result",
                "tool_use_id": tool_use_id,
                "content": output
            }));
        }
        messages.push(json!({ "role": "user", "content": results }));
    }
}

#[allow(clippy::too_many_arguments)]
async fn stream_single_claude_response(
    app: &AppHandle,
    client: &Client,
    sidecar: &SidecarManager,
    capacity: CapacityCaptureContext<'_>,
    api_key: &str,
    headroom_project: &str,
    body: &Value,
    conversation_id: &str,
    budget: &mut StreamBudget,
    tool_budget: &mut ToolBudget<'_>,
    emit_assistant_events: bool,
    cancel: Arc<AtomicBool>,
) -> ChatResult<ClaudeSingleResponse> {
    let mut headers = build_claude_request_headers(api_key)?;
    insert_headroom_project_header(&mut headers, headroom_project)?;
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    headers.insert(ACCEPT, HeaderValue::from_static("text/event-stream"));
    let mut retry_index = 0;
    let mut response = loop {
        let port = proxy_port(sidecar).await?;
        let url = format!("http://127.0.0.1:{port}/v1/messages");
        let request = add_headroom_compression_headers(client.post(url).headers(headers.clone()))
            .json(body)
            .send();
        let response = tokio::select! {
            result = tokio::time::timeout(RESPONSE_HEADERS_TIMEOUT, request) => match result {
                Ok(result) => result.map_err(|error| ChatError::transient(format!("Failed to connect to Claude through the proxy: {error}")))?,
                Err(_) => return Err(ChatError::transient("Timed out waiting for Claude response headers")),
            },
            _ = wait_for_cancel(cancel.clone()) => return Err(ChatError::new("Chat request cancelled")),
        };
        let status = response.status();
        let captured_at = Utc::now();
        let retry_after_seconds = parse_retry_after_seconds(response.headers());
        capacity.capture_headers(response.headers(), captured_at);
        if status.is_success() {
            capacity.clear_cooldown();
            break response;
        }

        let raw_body = read_bounded_error_body(response, cancel.clone()).await?;
        persist_upstream_error_log(app, status, &raw_body).await;
        let error = upstream_http_chat_error(status, &raw_body, String::new());
        capacity.capture_rate_limit_error(
            status,
            &raw_body,
            retry_after_seconds,
            &error.message,
            captured_at,
        );
        let Some(delay) = stream_retry_delay(retry_index, &error, StreamAttemptProgress::default())
        else {
            return Err(error);
        };
        prepare_stream_retry(sidecar, delay, cancel.clone()).await?;
        retry_index += 1;
    };

    let mut parser = ClaudeSseParser::default();
    let mut blocks = BTreeMap::<usize, ClaudeContentBlockState>::new();
    let mut content = Vec::new();
    let mut tool_calls = Vec::new();
    let mut assistant_text = String::new();
    let mut web_citations = Vec::new();
    let mut usage = None;
    let mut stop_reason = None;
    let mut completed = false;

    while !completed {
        let chunk = tokio::select! {
            result = tokio::time::timeout(RESPONSE_CHUNK_IDLE_TIMEOUT, response.chunk()) => match result {
                Ok(result) => result.map_err(|error| ChatError::transient_with_partial(format!("Claude SSE stream failed: {error}"), assistant_text.clone()))?,
                Err(_) => return Err(ChatError::transient_with_partial("Claude SSE stream timed out", assistant_text.clone())),
            },
            _ = wait_for_cancel(cancel.clone()) => return Err(ChatError::with_partial("Chat request cancelled", assistant_text.clone())),
        };
        let Some(chunk) = chunk else { break };
        budget
            .charge_raw(chunk.len())
            .map_err(|error| ChatError::with_partial(error.message, assistant_text.clone()))?;
        let events = parser
            .push(&chunk, budget.remaining_events())
            .map_err(|error| ChatError::with_partial(error.message, assistant_text.clone()))?;
        budget
            .charge_events(events.len())
            .map_err(|error| ChatError::with_partial(error.message, assistant_text.clone()))?;

        for event in events {
            let event_type = event
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or_default();
            match event_type {
                "message_start" => {
                    if let Some(raw_usage) = event
                        .get("message")
                        .and_then(|message| message.get("usage"))
                    {
                        usage = Some(claude_usage(raw_usage, None)?);
                    }
                }
                "content_block_start" => {
                    let index = claude_event_index(&event)?;
                    let block = claude_content_block_from_start(&event)?;
                    let block_type = block.get("type").and_then(Value::as_str);
                    if emit_assistant_events
                        && block_type == Some("server_tool_use")
                        && block.get("name").and_then(Value::as_str) == Some("web_search")
                    {
                        if let Some(call_id) = block.get("id").and_then(Value::as_str) {
                            emit_named_tool_call(
                                app,
                                conversation_id,
                                call_id,
                                "searching the web",
                                "web",
                                None,
                                "running",
                                None,
                            )
                            .map_err(ChatError::new)?;
                        }
                    }
                    if emit_assistant_events && block_type == Some("web_search_tool_result") {
                        if let Some(call_id) = block.get("tool_use_id").and_then(Value::as_str) {
                            let (status, tail) = claude_web_search_result_summary(&block);
                            if let Some(tail) = tail {
                                emit_delta(
                                    app,
                                    ChatDeltaPayload {
                                        call_id: Some(call_id.to_string()),
                                        tail: Some(tail),
                                        ..ChatDeltaPayload::new(
                                            conversation_id,
                                            ChatDeltaKind::ToolOutput,
                                        )
                                    },
                                )
                                .map_err(ChatError::new)?;
                            }
                            emit_tool_result(
                                app,
                                conversation_id,
                                call_id,
                                status,
                                None,
                                None,
                                false,
                            )
                            .map_err(ChatError::new)?;
                        }
                    }
                    let state = ClaudeContentBlockState {
                        block,
                        input_json: String::new(),
                    };
                    if state.block.get("type").and_then(Value::as_str) == Some("text") {
                        if let Some(text) = state
                            .block
                            .get("text")
                            .and_then(Value::as_str)
                            .filter(|text| !text.is_empty())
                        {
                            budget.charge_assistant(text)?;
                            assistant_text.push_str(text);
                            if emit_assistant_events {
                                emit_delta(
                                    app,
                                    ChatDeltaPayload {
                                        text: Some(text.to_string()),
                                        ..ChatDeltaPayload::new(
                                            conversation_id,
                                            ChatDeltaKind::Delta,
                                        )
                                    },
                                )
                                .map_err(ChatError::new)?;
                            }
                        }
                    }
                    blocks.insert(index, state);
                }
                "content_block_delta" => {
                    let index = claude_event_index(&event)?;
                    let delta = event
                        .get("delta")
                        .ok_or_else(|| ChatError::new("Claude block delta omitted delta"))?;
                    let state = blocks.get_mut(&index).ok_or_else(|| {
                        ChatError::new("Claude block delta referenced an unknown block")
                    })?;
                    match delta
                        .get("type")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                    {
                        "text_delta" => {
                            let text = delta
                                .get("text")
                                .and_then(Value::as_str)
                                .unwrap_or_default();
                            budget.charge_assistant(text)?;
                            assistant_text.push_str(text);
                            append_json_string(&mut state.block, "text", text)?;
                            if emit_assistant_events {
                                emit_delta(
                                    app,
                                    ChatDeltaPayload {
                                        text: Some(text.to_string()),
                                        ..ChatDeltaPayload::new(
                                            conversation_id,
                                            ChatDeltaKind::Delta,
                                        )
                                    },
                                )
                                .map_err(ChatError::new)?;
                            }
                        }
                        "thinking_delta" => {
                            let text = delta
                                .get("thinking")
                                .and_then(Value::as_str)
                                .unwrap_or_default();
                            budget.charge_reasoning(text)?;
                            append_json_string(&mut state.block, "thinking", text)?;
                            if emit_assistant_events {
                                emit_delta(
                                    app,
                                    ChatDeltaPayload {
                                        text: Some(text.to_string()),
                                        ..ChatDeltaPayload::new(
                                            conversation_id,
                                            ChatDeltaKind::Reasoning,
                                        )
                                    },
                                )
                                .map_err(ChatError::new)?;
                            }
                        }
                        "signature_delta" => {
                            if let Some(signature) = delta.get("signature").and_then(Value::as_str)
                            {
                                claude_block_object_mut(&mut state.block)?.insert(
                                    "signature".to_string(),
                                    Value::String(signature.to_string()),
                                );
                            }
                        }
                        "input_json_delta" => {
                            if let Some(partial) = delta.get("partial_json").and_then(Value::as_str)
                            {
                                if state.input_json.len().saturating_add(partial.len())
                                    > MAX_SSE_FRAME_BYTES
                                {
                                    return Err(ChatError::new(
                                        "Claude tool input exceeded the safety limit",
                                    ));
                                }
                                state.input_json.push_str(partial);
                            }
                        }
                        "citations_delta" => {
                            if let Some(citation) = delta.get("citation").cloned() {
                                collect_claude_web_citation(&citation, &mut web_citations);
                                let citations = claude_block_object_mut(&mut state.block)?
                                    .entry("citations")
                                    .or_insert_with(|| Value::Array(Vec::new()));
                                if let Some(citations) = citations.as_array_mut() {
                                    citations.push(citation);
                                }
                            }
                        }
                        _ => {}
                    }
                }
                "content_block_stop" => {
                    let index = claude_event_index(&event)?;
                    let mut state = blocks.remove(&index).ok_or_else(|| {
                        ChatError::new("Claude block stop referenced an unknown block")
                    })?;
                    if state.block.get("type").and_then(Value::as_str) == Some("tool_use") {
                        tool_budget.charge_tool_call()?;
                        let input = if state.input_json.trim().is_empty() {
                            json!({})
                        } else {
                            serde_json::from_str(&state.input_json).map_err(|error| {
                                ChatError::new(format!(
                                    "Claude tool input was invalid JSON: {error}"
                                ))
                            })?
                        };
                        claude_block_object_mut(&mut state.block)?
                            .insert("input".to_string(), input.clone());
                        tool_calls.push(claude_tool_call(&state.block, input)?);
                    } else if state.block.get("type").and_then(Value::as_str)
                        == Some("server_tool_use")
                        && state.block.get("name").and_then(Value::as_str) == Some("web_search")
                    {
                        tool_budget.charge_tool_call()?;
                        let input = if state.input_json.trim().is_empty() {
                            json!({})
                        } else {
                            serde_json::from_str(&state.input_json).map_err(|error| {
                                ChatError::new(format!(
                                    "Claude web search input was invalid JSON: {error}"
                                ))
                            })?
                        };
                        claude_block_object_mut(&mut state.block)?
                            .insert("input".to_string(), input.clone());
                        if emit_assistant_events {
                            if let Some(call_id) = state.block.get("id").and_then(Value::as_str) {
                                let command = input
                                    .get("query")
                                    .and_then(Value::as_str)
                                    .unwrap_or("searching the web");
                                emit_named_tool_call(
                                    app,
                                    conversation_id,
                                    call_id,
                                    command,
                                    "web",
                                    None,
                                    "running",
                                    None,
                                )
                                .map_err(ChatError::new)?;
                            }
                        }
                    }
                    budget.charge_output_item()?;
                    content.push(state.block);
                }
                "message_delta" => {
                    stop_reason = event
                        .get("delta")
                        .and_then(|delta| delta.get("stop_reason"))
                        .and_then(Value::as_str)
                        .map(ToString::to_string);
                    if let Some(raw_usage) = event.get("usage") {
                        let input = usage
                            .as_ref()
                            .map(|usage| usage.input_tokens)
                            .unwrap_or_default();
                        let cached = usage
                            .as_ref()
                            .map(|usage| usage.cached_tokens)
                            .unwrap_or_default();
                        usage = Some(claude_usage(raw_usage, Some((input, cached)))?);
                    }
                }
                "message_stop" => completed = true,
                "error" => {
                    let message = event
                        .get("error")
                        .and_then(|error| error.get("message"))
                        .and_then(Value::as_str)
                        .unwrap_or("Claude stream error");
                    return Err(ChatError::with_partial(message, assistant_text));
                }
                "ping" => {}
                _ => {}
            }
        }
    }
    if !completed {
        let message = if parser.is_empty() {
            "Claude SSE stream ended before message_stop"
        } else {
            "Claude SSE stream ended with an incomplete event frame"
        };
        return Err(ChatError::with_partial(message, assistant_text));
    }
    let citation_suffix = web_citations_markdown(&web_citations);
    if !citation_suffix.is_empty() {
        append_citation_suffix_preserving_goal_marker(&mut assistant_text, &citation_suffix);
        if emit_assistant_events {
            emit_delta(
                app,
                ChatDeltaPayload {
                    text: Some(citation_suffix),
                    ..ChatDeltaPayload::new(conversation_id, ChatDeltaKind::Delta)
                },
            )
            .map_err(ChatError::new)?;
        }
    }

    Ok(ClaudeSingleResponse {
        assistant_text,
        content,
        tool_calls,
        usage,
        stop_reason,
    })
}

fn build_claude_messages(
    messages: &[StoredMessage],
    project_context: Option<&ProjectPromptContext>,
    referenced_files_context: Option<&str>,
) -> Vec<Value> {
    build_claude_messages_with_compaction(messages, project_context, referenced_files_context, None)
}

fn build_claude_messages_with_compaction(
    messages: &[StoredMessage],
    project_context: Option<&ProjectPromptContext>,
    referenced_files_context: Option<&str>,
    compaction: Option<&storage::ConversationCompaction>,
) -> Vec<Value> {
    let mut output = Vec::new();
    if let Some(context) = project_context {
        output.push(json!({ "role": "user", "content": context.environment_context }));
        if let Some(instructions) = &context.agents_instructions {
            output.push(json!({ "role": "user", "content": instructions }));
        }
    }
    if let Some(compaction) = compaction {
        output.push(json!({
            "role": "user",
            "content": compacted_summary_message(&compaction.summary)
        }));
    }
    if let Some(context) = referenced_files_context.filter(|value| !value.trim().is_empty()) {
        output.push(json!({ "role": "user", "content": context }));
    }
    output.extend(
        messages
            .iter()
            .filter(|message| match compaction {
                Some(active) => message.sequence > active.upto_message_id,
                None => true,
            })
            .filter(|message| !message.error)
            .filter_map(message_to_claude_message),
    );
    output
}

fn message_to_claude_message(message: &StoredMessage) -> Option<Value> {
    let role = match message.role.as_str() {
        "assistant" => "assistant",
        "user" | "developer" | "system" => "user",
        _ => return None,
    };
    if role == "assistant" || message.attachments.is_empty() {
        return (!message.content.trim().is_empty())
            .then(|| json!({ "role": role, "content": message.content }));
    }

    let mut content = message
        .attachments
        .iter()
        .map(|attachment| {
            json!({
                "type": "image",
                "source": {
                    "type": "base64",
                    "media_type": attachment.media_type,
                    "data": attachment.data,
                }
            })
        })
        .collect::<Vec<_>>();
    if !message.content.trim().is_empty() {
        content.push(json!({ "type": "text", "text": message.content }));
    }
    Some(json!({ "role": role, "content": content }))
}

#[allow(clippy::too_many_arguments)]
fn build_claude_request_body(
    model: &str,
    effort: &str,
    goal: Option<&str>,
    messages: Vec<Value>,
    agent_mode: bool,
    plan_mode: bool,
    effective_policy: exec::ApprovalPolicy,
    mcp_tools: &[extensions::McpToolDescriptor],
) -> Value {
    build_claude_request_body_for_profile(
        model,
        effort,
        goal,
        messages,
        agent_mode,
        plan_mode,
        effective_policy,
        mcp_tools,
        true,
    )
}

#[allow(clippy::too_many_arguments)]
fn build_claude_request_body_for_profile(
    model: &str,
    effort: &str,
    goal: Option<&str>,
    messages: Vec<Value>,
    agent_mode: bool,
    plan_mode: bool,
    effective_policy: exec::ApprovalPolicy,
    mcp_tools: &[extensions::McpToolDescriptor],
    include_coordination_tools: bool,
) -> Value {
    let tools = build_claude_tools(
        agent_mode,
        plan_mode,
        effective_policy,
        mcp_tools,
        include_coordination_tools,
    );
    let mut body = json!({
        "model": model,
        "max_tokens": if matches!(effort, "xhigh" | "max" | "ultra") { 64_000 } else { 32_000 },
        "messages": messages,
        "system": [{ "type": "text", "text": response_instructions(goal, agent_mode, plan_mode), "cache_control": { "type": "ephemeral" } }],
        "stream": true
    });
    if !tools.is_empty() {
        body["tools"] = Value::Array(tools);
        body["tool_choice"] = json!({ "type": "auto" });
    }
    if claude_supports_effort(model) {
        body["output_config"] = json!({ "effort": reasoning_effort_for_request(effort) });
    }
    if claude_supports_adaptive_thinking(model) {
        body["thinking"] = json!({ "type": "adaptive", "display": "summarized" });
    }
    body
}

fn build_claude_tools(
    agent_mode: bool,
    plan_mode: bool,
    effective_policy: exec::ApprovalPolicy,
    mcp_tools: &[extensions::McpToolDescriptor],
    include_coordination_tools: bool,
) -> Vec<Value> {
    let mut tools = vec![json!({
        "type": "web_search_20250305",
        "name": "web_search"
    })];
    if !agent_mode && !plan_mode {
        return tools;
    }
    if agent_mode {
        let shell = exec::shell_command_tool_schema();
        tools.push(json!({
            "name": "shell_command",
            "description": shell.get("description").cloned().unwrap_or(Value::Null),
            "input_schema": shell.get("parameters").cloned().unwrap_or_else(|| json!({ "type": "object" }))
        }));
        tools.push(claude_function_tool(view_image_tool_schema()));
    }
    if plan_mode || (agent_mode && include_coordination_tools) {
        tools.push(claude_function_tool(update_plan_tool_schema()));
    }
    if agent_mode && include_coordination_tools && !plan_mode {
        tools.extend(
            subagent_tool_schemas()
                .into_iter()
                .map(claude_function_tool),
        );
    }
    if agent_mode && !plan_mode {
        tools.extend(browser_tool_schemas().into_iter().map(claude_function_tool));
    }
    if agent_mode && !plan_mode && effective_policy.workspace_write_enabled() {
        tools.push(json!({
            "name": "apply_patch",
            "description": "Apply a Codex-style patch to workspace files.",
            "input_schema": {
                "type": "object",
                "properties": { "patch": { "type": "string", "description": "The complete *** Begin Patch ... *** End Patch envelope." } },
                "required": ["patch"],
                "additionalProperties": false
            }
        }));
    }
    if agent_mode {
        tools.extend(mcp_tools.iter().map(|tool| {
            json!({
                "name": tool.wire_name,
                "description": tool.description,
                "input_schema": tool.input_schema
            })
        }));
    }
    tools
}

fn claude_function_tool(tool: Value) -> Value {
    json!({
        "name": tool.get("name").cloned().unwrap_or(Value::Null),
        "description": tool.get("description").cloned().unwrap_or(Value::Null),
        "input_schema": tool.get("parameters").cloned().unwrap_or_else(|| json!({ "type": "object" }))
    })
}

fn claude_tool_call(block: &Value, input: Value) -> ChatResult<ToolCallEvent> {
    let call_id = block
        .get("id")
        .and_then(Value::as_str)
        .ok_or_else(|| ChatError::new("Claude tool_use block omitted id"))?
        .to_string();
    let name = block
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| ChatError::new("Claude tool_use block omitted name"))?
        .to_string();
    let (kind, input) = if name == "apply_patch" {
        let patch = input
            .get("patch")
            .and_then(Value::as_str)
            .ok_or_else(|| ChatError::new("Claude apply_patch call omitted patch"))?;
        (ToolCallKind::Custom, patch.to_string())
    } else {
        (ToolCallKind::Function, input.to_string())
    };
    Ok(ToolCallEvent {
        call_id,
        name,
        input,
        kind,
        item: block.clone(),
    })
}

fn claude_web_search_result_summary(block: &Value) -> (&'static str, Option<String>) {
    let Some(content) = block.get("content") else {
        return ("completed", None);
    };
    if let Some(error_code) = content
        .get("error_code")
        .and_then(Value::as_str)
        .or_else(|| {
            (content.get("type").and_then(Value::as_str) == Some("web_search_tool_result_error"))
                .then(|| content.get("type").and_then(Value::as_str))
                .flatten()
        })
    {
        return ("failed", Some(error_code.to_string()));
    }
    let Some(results) = content.as_array() else {
        return ("completed", None);
    };
    let lines = results
        .iter()
        .filter_map(|result| {
            let url = result.get("url").and_then(Value::as_str)?;
            let title = result
                .get("title")
                .and_then(Value::as_str)
                .filter(|title| !title.trim().is_empty())
                .unwrap_or(url);
            Some(format!("{title}\n{url}"))
        })
        .collect::<Vec<_>>();
    ("completed", (!lines.is_empty()).then(|| lines.join("\n\n")))
}

fn collect_claude_web_citation(citation: &Value, citations: &mut Vec<WebCitation>) {
    if citation.get("type").and_then(Value::as_str) != Some("web_search_result_location") {
        return;
    }
    let Some(url) = citation.get("url").and_then(Value::as_str) else {
        return;
    };
    let title = citation.get("title").and_then(Value::as_str).unwrap_or(url);
    push_web_citation(citations, title, url);
}

fn claude_usage(raw: &Value, prior_input: Option<(i64, i64)>) -> ChatResult<TokenUsage> {
    let cache_read = raw
        .get("cache_read_input_tokens")
        .and_then(Value::as_i64)
        .unwrap_or_else(|| prior_input.map(|(_, cached)| cached).unwrap_or_default());
    let cache_creation = raw
        .get("cache_creation_input_tokens")
        .and_then(Value::as_i64)
        .unwrap_or_default();
    let base_input = raw
        .get("input_tokens")
        .and_then(Value::as_i64)
        .unwrap_or_else(|| prior_input.map(|(input, _)| input).unwrap_or_default());
    let input_tokens = if prior_input.is_some() {
        base_input
    } else {
        base_input
            .checked_add(cache_read)
            .and_then(|value| value.checked_add(cache_creation))
            .ok_or_else(|| ChatError::new("Claude input token count overflowed"))?
    };
    let output_tokens = raw
        .get("output_tokens")
        .and_then(Value::as_i64)
        .unwrap_or_default();
    Ok(TokenUsage {
        input_tokens,
        output_tokens,
        total_tokens: input_tokens
            .checked_add(output_tokens)
            .ok_or_else(|| ChatError::new("Claude total token count overflowed"))?,
        cached_tokens: cache_read,
        reasoning_tokens: 0,
    })
}

fn claude_event_index(event: &Value) -> ChatResult<usize> {
    event
        .get("index")
        .and_then(Value::as_u64)
        .and_then(|index| usize::try_from(index).ok())
        .ok_or_else(|| ChatError::new("Claude content event omitted a valid index"))
}

// Extract the content block from a content_block_start event.
// Reject a frame whose content block is not a JSON object.
fn claude_content_block_from_start(event: &Value) -> ChatResult<Value> {
    let block = event
        .get("content_block")
        .ok_or_else(|| ChatError::new("Claude content block start omitted content_block"))?;
    if !block.is_object() {
        return Err(ChatError::new(
            "Claude content block start carried a non-object content_block",
        ));
    }
    Ok(block.clone())
}

// Borrow a stream-derived content block as a JSON object map.
// Return a stream-protocol error instead of a panic on a bad frame.
fn claude_block_object_mut(block: &mut Value) -> ChatResult<&mut serde_json::Map<String, Value>> {
    block
        .as_object_mut()
        .ok_or_else(|| ChatError::new("Claude content block was not a JSON object"))
}

fn append_json_string(value: &mut Value, key: &str, suffix: &str) -> ChatResult<()> {
    let current = value.get(key).and_then(Value::as_str).unwrap_or_default();
    let next = Value::String(format!("{current}{suffix}"));
    claude_block_object_mut(value)?.insert(key.to_string(), next);
    Ok(())
}

fn is_claude_model(model: &str) -> bool {
    model.starts_with("claude-")
}

fn claude_supports_effort(model: &str) -> bool {
    model.contains("-5")
        || model.contains("opus-4-8")
        || model.contains("opus-4-7")
        || model.contains("opus-4-6")
        || model.contains("sonnet-4-6")
        || model.contains("opus-4-5")
}

fn claude_supports_adaptive_thinking(model: &str) -> bool {
    model.contains("-5")
        || model.contains("opus-4-8")
        || model.contains("opus-4-7")
        || model.contains("opus-4-6")
        || model.contains("sonnet-4-6")
        || model.contains("mythos-preview")
}

fn prepend_stream_partial(mut error: ChatError, prior_text: &str) -> ChatError {
    if prior_text.is_empty() {
        return error;
    }
    let mut partial = String::with_capacity(prior_text.len() + error.partial_text.len());
    partial.push_str(prior_text);
    partial.push_str(&error.partial_text);
    error.partial_text = partial;
    error
}

fn merge_token_usage(
    accumulated: &mut Option<TokenUsage>,
    next: Option<TokenUsage>,
) -> ChatResult<()> {
    let Some(next) = next else {
        return Ok(());
    };
    validate_token_usage(&next)?;
    let Some(current) = accumulated.as_ref() else {
        *accumulated = Some(next);
        return Ok(());
    };
    validate_token_usage(current)?;

    let merged = TokenUsage {
        input_tokens: checked_usage_sum(current.input_tokens, next.input_tokens, "input_tokens")?,
        output_tokens: checked_usage_sum(
            current.output_tokens,
            next.output_tokens,
            "output_tokens",
        )?,
        total_tokens: checked_usage_sum(current.total_tokens, next.total_tokens, "total_tokens")?,
        cached_tokens: checked_usage_sum(
            current.cached_tokens,
            next.cached_tokens,
            "cached_tokens",
        )?,
        reasoning_tokens: checked_usage_sum(
            current.reasoning_tokens,
            next.reasoning_tokens,
            "reasoning_tokens",
        )?,
    };
    *accumulated = Some(merged);
    Ok(())
}

fn validate_token_usage(usage: &TokenUsage) -> ChatResult<()> {
    for (name, value) in [
        ("input_tokens", usage.input_tokens),
        ("output_tokens", usage.output_tokens),
        ("total_tokens", usage.total_tokens),
        ("cached_tokens", usage.cached_tokens),
        ("reasoning_tokens", usage.reasoning_tokens),
    ] {
        if value < 0 {
            return Err(ChatError::new(format!(
                "response usage field `{name}` cannot be negative"
            )));
        }
    }
    Ok(())
}

fn checked_usage_sum(left: i64, right: i64, name: &str) -> ChatResult<i64> {
    left.checked_add(right).ok_or_else(|| {
        ChatError::new(format!(
            "response usage field `{name}` overflowed across agent rounds"
        ))
    })
}

struct SingleResponse {
    assistant_text: String,
    usage: Option<TokenUsage>,
    service_tier: Option<String>,
    tool_calls: Vec<ToolCallEvent>,
}

#[derive(Debug, Default, Clone, Copy)]
struct StreamAttemptProgress {
    any_delta_emitted: bool,
    tool_call_parsed: bool,
}

fn stream_retry_delay(
    retry_index: usize,
    error: &ChatError,
    progress: StreamAttemptProgress,
) -> Option<Duration> {
    if !error.retryable
        || !error.partial_text.is_empty()
        || progress.any_delta_emitted
        || progress.tool_call_parsed
    {
        return None;
    }
    RESPONSE_RETRY_DELAYS.get(retry_index).copied()
}

async fn wait_for_stream_retry_backoff(delay: Duration, cancel: Arc<AtomicBool>) -> ChatResult<()> {
    if cancel.load(Ordering::SeqCst) {
        return Err(ChatError::new("Chat request cancelled"));
    }
    tokio::select! {
        biased;
        _ = wait_for_cancel(cancel) => Err(ChatError::new("Chat request cancelled")),
        _ = tokio::time::sleep(delay) => Ok(()),
    }
}

async fn prepare_stream_retry(
    sidecar: &SidecarManager,
    delay: Duration,
    cancel: Arc<AtomicBool>,
) -> ChatResult<()> {
    wait_for_stream_retry_backoff(delay, cancel.clone()).await?;
    if cancel.load(Ordering::SeqCst) {
        return Err(ChatError::new("Chat request cancelled"));
    }
    tokio::select! {
        biased;
        _ = wait_for_cancel(cancel.clone()) => return Err(ChatError::new("Chat request cancelled")),
        _ = sidecar.ensure_ready() => {}
    }
    if cancel.load(Ordering::SeqCst) {
        return Err(ChatError::new("Chat request cancelled"));
    }
    Ok(())
}

fn sanitize_headroom_project_name(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, ':' | '-' | '_') {
                character
            } else {
                '_'
            }
        })
        .take(HEADROOM_PROJECT_MAX_CHARS)
        .collect()
}

fn gui_turn_headroom_project(conversation_id: &str, turn_id: &str) -> GuiHeadroomProject {
    GuiHeadroomProject {
        key: sanitize_headroom_project_name(&format!("gui:{conversation_id}:{turn_id}")),
        scope: GuiHeadroomProjectScope::Turn,
    }
}

fn gui_background_headroom_project(conversation_id: &str, kind: &str) -> GuiHeadroomProject {
    GuiHeadroomProject {
        key: sanitize_headroom_project_name(&format!("gui-bg:{conversation_id}:{kind}")),
        scope: GuiHeadroomProjectScope::Background,
    }
}

fn insert_headroom_project_header(
    headers: &mut HeaderMap,
    headroom_project: &str,
) -> ChatResult<()> {
    let headroom_project = sanitize_headroom_project_name(headroom_project);
    if headroom_project.is_empty() {
        return Err(ChatError::new("Headroom project key cannot be empty"));
    }
    headers.insert(
        HeaderName::from_static(HEADROOM_PROJECT_HEADER),
        HeaderValue::from_str(&headroom_project)
            .map_err(|error| ChatError::new(format!("invalid Headroom project header: {error}")))?,
    );
    Ok(())
}

async fn debug_log_excluded_background_savings(
    sidecar: &SidecarManager,
    project: &GuiHeadroomProject,
) {
    #[cfg(debug_assertions)]
    {
        match sidecar.project_savings_totals(&project.key).await {
            Ok(savings) => eprintln!(
                "Excluded background Headroom project {} from message snapshots: {} tokens",
                project.key, savings.tokens_saved
            ),
            Err(error) => eprintln!(
                "Could not read excluded background Headroom project {}: {error}",
                project.key
            ),
        }
    }
    #[cfg(not(debug_assertions))]
    let _ = (sidecar, project);
}

fn add_headroom_compression_headers(request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
    request
        .header("x-headroom-compress-messages", "true")
        .header("x-headroom-protect-recent", "0")
}

#[allow(clippy::too_many_arguments)]
async fn stream_single_response(
    app: &AppHandle,
    client: &Client,
    sidecar: &SidecarManager,
    capacity: CapacityCaptureContext<'_>,
    mut headers: HeaderMap,
    headroom_project: &str,
    body: &Value,
    conversation_id: &str,
    history: &mut Vec<Value>,
    budget: &mut StreamBudget,
    tool_budget: &mut ToolBudget<'_>,
    progress: &mut StreamAttemptProgress,
    emit_assistant_events: bool,
    cancel: Arc<AtomicBool>,
) -> ChatResult<SingleResponse> {
    insert_headroom_project_header(&mut headers, headroom_project)?;
    let port = tokio::select! {
        biased;
        _ = wait_for_cancel(cancel.clone()) => return Err(ChatError::new("Chat request cancelled")),
        result = proxy_port(sidecar) => result?,
    };
    let url = format!("http://127.0.0.1:{port}/v1/responses");
    let request = add_headroom_compression_headers(client.post(&url).headers(headers))
        .json(body)
        .send();
    let mut response = tokio::select! {
        result = tokio::time::timeout(RESPONSE_HEADERS_TIMEOUT, request) => match result {
            Ok(result) => result.map_err(|error| ChatError::transient(format!("Failed to connect to the proxy: {error}")))?,
            Err(_) => return Err(ChatError::transient("Timed out waiting for proxy response headers")),
        },
        _ = wait_for_cancel(cancel.clone()) => return Err(ChatError::new("Chat request cancelled")),
    };

    let status = response.status();
    let captured_at = Utc::now();
    let retry_after_seconds = parse_retry_after_seconds(response.headers());
    capacity.capture_headers(response.headers(), captured_at);
    if !status.is_success() {
        let raw_body = read_bounded_error_body(response, cancel.clone()).await?;
        persist_upstream_error_log(app, status, &raw_body).await;
        let error = upstream_http_chat_error(status, &raw_body, String::new());
        capacity.capture_rate_limit_error(
            status,
            &raw_body,
            retry_after_seconds,
            &error.message,
            captured_at,
        );
        return Err(error);
    }
    capacity.clear_cooldown();

    let mut parser = SseParser::default();
    let mut assistant_text = String::new();
    let mut web_citations = Vec::new();
    let mut completed_usage = None;
    let mut completed_service_tier = None;
    let mut completed = false;
    let mut tool_calls = Vec::new();

    loop {
        let chunk = tokio::select! {
            result = tokio::time::timeout(RESPONSE_CHUNK_IDLE_TIMEOUT, response.chunk()) => match result {
                Ok(result) => result.map_err(|error| {
                    ChatError::transient_with_partial(
                        format!("SSE stream failed: {error}"),
                        assistant_text.clone(),
                    )
                })?,
                Err(_) => return Err(ChatError::transient_with_partial(
                    "SSE stream timed out waiting for the next chunk",
                    assistant_text.clone(),
                )),
            },
            _ = wait_for_cancel(cancel.clone()) => {
                return Err(ChatError::with_partial("Chat request cancelled", assistant_text.clone()));
            }
        };

        let Some(chunk) = chunk else {
            break;
        };
        budget
            .charge_raw(chunk.len())
            .map_err(|error| ChatError::with_partial(error.message, assistant_text.clone()))?;

        let events = parser
            .push(&chunk, budget.remaining_events())
            .map_err(|error| ChatError::with_partial(error.message, assistant_text.clone()))?;
        budget
            .charge_events(events.len())
            .map_err(|error| ChatError::with_partial(error.message, assistant_text.clone()))?;

        for event in events {
            if cancel.load(Ordering::SeqCst) {
                return Err(ChatError::with_partial(
                    "Chat request cancelled",
                    assistant_text.clone(),
                ));
            }
            match event {
                ParsedSseEvent::Delta(delta) => {
                    budget.charge_assistant(&delta).map_err(|error| {
                        ChatError::with_partial(error.message, assistant_text.clone())
                    })?;
                    assistant_text.push_str(&delta);
                    if emit_assistant_events {
                        emit_delta(
                            app,
                            ChatDeltaPayload {
                                text: Some(delta),
                                ..ChatDeltaPayload::new(conversation_id, ChatDeltaKind::Delta)
                            },
                        )
                        .map_err(|message| {
                            ChatError::with_partial(message, assistant_text.clone())
                        })?;
                        progress.any_delta_emitted = true;
                    }
                }
                ParsedSseEvent::Reasoning(delta) => {
                    budget.charge_reasoning(&delta).map_err(|error| {
                        ChatError::with_partial(error.message, assistant_text.clone())
                    })?;
                    if emit_assistant_events {
                        emit_delta(
                            app,
                            ChatDeltaPayload {
                                text: Some(delta),
                                ..ChatDeltaPayload::new(conversation_id, ChatDeltaKind::Reasoning)
                            },
                        )
                        .map_err(|message| {
                            ChatError::with_partial(message, assistant_text.clone())
                        })?;
                        progress.any_delta_emitted = true;
                    }
                }
                ParsedSseEvent::ReasoningBoundary => {
                    if emit_assistant_events {
                        emit_delta(
                            app,
                            ChatDeltaPayload {
                                text: Some(REASONING_FRAGMENT_SEPARATOR.to_string()),
                                ..ChatDeltaPayload::new(conversation_id, ChatDeltaKind::Reasoning)
                            },
                        )
                        .map_err(|message| {
                            ChatError::with_partial(message, assistant_text.clone())
                        })?;
                    }
                }
                ParsedSseEvent::FinalMessage { text, item } => {
                    budget.charge_output_item().map_err(|error| {
                        ChatError::with_partial(error.message, assistant_text.clone())
                    })?;
                    collect_openai_web_citations(&item, &mut web_citations);
                    history.push(item);
                    if assistant_text.is_empty() {
                        budget.charge_assistant(&text).map_err(|error| {
                            ChatError::with_partial(error.message, assistant_text.clone())
                        })?;
                        assistant_text = text;
                    }
                }
                ParsedSseEvent::OutputItem(item) => {
                    budget.charge_output_item().map_err(|error| {
                        ChatError::with_partial(error.message, assistant_text.clone())
                    })?;
                    history.push(item);
                }
                ParsedSseEvent::WebSearchCall {
                    call_id,
                    command,
                    status,
                    item,
                    tail,
                } => {
                    progress.tool_call_parsed = true;
                    if let Some(item) = item {
                        tool_budget.charge_tool_call().map_err(|error| {
                            ChatError::with_partial(error.message, assistant_text.clone())
                        })?;
                        budget.charge_output_item().map_err(|error| {
                            ChatError::with_partial(error.message, assistant_text.clone())
                        })?;
                        history.push(item);
                    }
                    if emit_assistant_events {
                        emit_named_tool_call(
                            app,
                            conversation_id,
                            &call_id,
                            &command,
                            "web",
                            None,
                            &status,
                            None,
                        )
                        .map_err(|message| {
                            ChatError::with_partial(message, assistant_text.clone())
                        })?;
                        if let Some(tail) = tail.filter(|tail| !tail.is_empty()) {
                            emit_delta(
                                app,
                                ChatDeltaPayload {
                                    call_id: Some(call_id.clone()),
                                    tail: Some(tail),
                                    ..ChatDeltaPayload::new(
                                        conversation_id,
                                        ChatDeltaKind::ToolOutput,
                                    )
                                },
                            )
                            .map_err(|message| {
                                ChatError::with_partial(message, assistant_text.clone())
                            })?;
                        }
                        if status == "completed" || status == "failed" {
                            emit_tool_result(
                                app,
                                conversation_id,
                                &call_id,
                                &status,
                                None,
                                None,
                                false,
                            )
                            .map_err(|message| {
                                ChatError::with_partial(message, assistant_text.clone())
                            })?;
                        }
                    }
                }
                ParsedSseEvent::ToolCall(call) => {
                    progress.tool_call_parsed = true;
                    tool_budget.charge_tool_call().map_err(|error| {
                        ChatError::with_partial(error.message, assistant_text.clone())
                    })?;
                    budget.charge_output_item().map_err(|error| {
                        ChatError::with_partial(error.message, assistant_text.clone())
                    })?;
                    history.push(call.item.clone());
                    tool_calls.push(call);
                }
                ParsedSseEvent::Completed {
                    usage,
                    service_tier,
                } => {
                    completed_usage = usage;
                    completed_service_tier = service_tier;
                    completed = true;
                }
                ParsedSseEvent::Failed(message) => {
                    return Err(ChatError::with_partial(message, assistant_text.clone()));
                }
                ParsedSseEvent::Ignored => {}
            }
            if completed {
                break;
            }
        }

        if completed {
            break;
        }
    }

    if !completed {
        let message = if parser.is_empty() {
            "SSE stream ended before response.completed"
        } else {
            "SSE stream ended with an incomplete event frame"
        };
        return Err(ChatError::with_partial(message, assistant_text.clone()));
    }

    let citation_suffix = web_citations_markdown(&web_citations);
    if !citation_suffix.is_empty() {
        append_citation_suffix_preserving_goal_marker(&mut assistant_text, &citation_suffix);
        if emit_assistant_events && progress.any_delta_emitted {
            emit_delta(
                app,
                ChatDeltaPayload {
                    text: Some(citation_suffix),
                    ..ChatDeltaPayload::new(conversation_id, ChatDeltaKind::Delta)
                },
            )
            .map_err(|message| ChatError::with_partial(message, assistant_text.clone()))?;
        }
    }

    Ok(SingleResponse {
        assistant_text,
        usage: completed_usage,
        service_tier: completed_service_tier,
        tool_calls,
    })
}

async fn read_bounded_error_body(
    mut response: reqwest::Response,
    cancel: Arc<AtomicBool>,
) -> ChatResult<String> {
    let mut bytes = Vec::new();
    let mut truncated = false;
    loop {
        let chunk = tokio::select! {
            result = tokio::time::timeout(RESPONSE_CHUNK_IDLE_TIMEOUT, response.chunk()) => match result {
                Ok(result) => result.map_err(|error| ChatError::new(format!("Failed to read proxy error body: {error}")))?,
                Err(_) => return Err(ChatError::new("Timed out reading proxy error body")),
            },
            _ = wait_for_cancel(cancel.clone()) => {
                return Err(ChatError::new("Chat request cancelled while reading proxy error body"));
            }
        };
        let Some(chunk) = chunk else {
            break;
        };
        let remaining = MAX_ERROR_BODY_BYTES.saturating_sub(bytes.len());
        if chunk.len() > remaining {
            bytes.extend_from_slice(&chunk[..remaining]);
            truncated = true;
            break;
        }
        bytes.extend_from_slice(&chunk);
    }

    let mut body = String::from_utf8_lossy(&bytes).into_owned();
    if truncated {
        body.push_str("\n[... error body truncated by Halv]");
    }
    Ok(body)
}

async fn persist_upstream_error_log(app: &AppHandle, status: StatusCode, raw_body: &str) {
    let app_data_dir = match app_data::resolved_app_data_dir(app) {
        Ok(path) => path,
        Err(error) => {
            eprintln!("Failed to resolve the upstream error log directory: {error}");
            return;
        }
    };
    let line = format_upstream_error_log_line(&Utc::now().to_rfc3339(), status, raw_body);
    match tokio::task::spawn_blocking(move || append_upstream_error_log(&app_data_dir, &line)).await
    {
        Ok(Ok(())) => {}
        Ok(Err(error)) => eprintln!("Failed to persist upstream HTTP error details: {error}"),
        Err(error) => eprintln!("Failed to join upstream HTTP error logger: {error}"),
    }
}

fn append_upstream_error_log(app_data_dir: &Path, line: &str) -> std::io::Result<()> {
    let _guard = UPSTREAM_ERROR_LOG_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .map_err(|_| std::io::Error::other("upstream error log lock poisoned"))?;
    let logs_dir = app_data_dir.join("logs");
    fs::create_dir_all(&logs_dir)?;
    let path = logs_dir.join(UPSTREAM_ERROR_LOG_FILE_NAME);
    let record_bytes = u64::try_from(line.len())
        .unwrap_or(u64::MAX)
        .saturating_add(1);
    let current_bytes = match fs::metadata(&path) {
        Ok(metadata) => metadata.len(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => 0,
        Err(error) => return Err(error),
    };
    if current_bytes.saturating_add(record_bytes) > UPSTREAM_ERROR_LOG_MAX_BYTES {
        fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&path)?;
    }
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    file.write_all(line.as_bytes())?;
    file.write_all(b"\n")?;
    Ok(())
}

fn format_upstream_error_log_line(timestamp: &str, status: StatusCode, raw_body: &str) -> String {
    let error_type = parse_upstream_error_details(raw_body)
        .and_then(|details| details.error_type)
        .map(|error_type| {
            scrub_and_truncate_upstream_log_text(&error_type, UPSTREAM_ERROR_LOG_TYPE_MAX_CHARS)
        })
        .filter(|error_type| !error_type.is_empty())
        .unwrap_or_else(|| "unknown".to_string());
    let body = scrub_and_truncate_upstream_log_text(raw_body, UPSTREAM_ERROR_LOG_BODY_MAX_CHARS);
    let body = if body.is_empty() {
        "<empty>".to_string()
    } else {
        body
    };
    format!(
        "{timestamp} HTTP {} type={error_type} body={body}",
        status.as_u16()
    )
}

fn scrub_and_truncate_upstream_log_text(raw: &str, max_chars: usize) -> String {
    let collapsed = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    let scrubbed = scrub_upstream_log_secrets(&collapsed);
    let mut chars = scrubbed.chars();
    let truncated = chars.by_ref().take(max_chars).collect::<String>();
    if chars.next().is_some() {
        format!("{truncated}… [truncated by Halv]")
    } else {
        truncated
    }
}

fn scrub_upstream_log_secrets(raw: &str) -> String {
    let lower = raw.to_ascii_lowercase();
    let mut redactions = Vec::<(usize, usize)>::new();

    for (start, _) in lower.match_indices("sk-") {
        let end = secret_token_end(raw.as_bytes(), start);
        if end > start + 3 {
            redactions.push((start, end));
        }
    }
    for (start, _) in lower.match_indices("bearer") {
        if is_ascii_word_boundary(lower.as_bytes(), start, "bearer".len()) {
            if let Some(range) = bearer_secret_range(raw, start + "bearer".len()) {
                redactions.push(range);
            }
        }
    }
    for marker in [
        "authorization",
        "x-api-key",
        "api-key",
        "api_key",
        "apikey",
        "access_token",
    ] {
        for (start, _) in lower.match_indices(marker) {
            if is_ascii_word_boundary(lower.as_bytes(), start, marker.len()) {
                if let Some(range) = labeled_secret_range(raw, start + marker.len()) {
                    redactions.push(range);
                }
            }
        }
    }

    redactions.sort_unstable_by_key(|range| range.0);
    let mut merged = Vec::<(usize, usize)>::new();
    for (start, end) in redactions {
        if let Some(last) = merged.last_mut() {
            if start <= last.1 {
                last.1 = last.1.max(end);
                continue;
            }
        }
        merged.push((start, end));
    }

    let mut scrubbed = String::with_capacity(raw.len());
    let mut cursor = 0;
    for (start, end) in merged {
        scrubbed.push_str(&raw[cursor..start]);
        scrubbed.push_str("[REDACTED]");
        cursor = end;
    }
    scrubbed.push_str(&raw[cursor..]);
    scrubbed
}

fn is_ascii_word_boundary(bytes: &[u8], start: usize, len: usize) -> bool {
    let is_word = |byte: u8| byte.is_ascii_alphanumeric() || byte == b'_';
    start
        .checked_sub(1)
        .is_none_or(|index| !is_word(bytes[index]))
        && bytes
            .get(start + len)
            .copied()
            .is_none_or(|byte| !is_word(byte))
}

fn bearer_secret_range(raw: &str, marker_end: usize) -> Option<(usize, usize)> {
    let bytes = raw.as_bytes();
    let mut start = marker_end;
    while bytes.get(start).is_some_and(|byte| {
        byte.is_ascii_whitespace() || matches!(byte, b':' | b'=' | b'"' | b'\'')
    }) {
        start += 1;
    }
    let end = secret_token_end(bytes, start);
    (end > start).then_some((start, end))
}

fn labeled_secret_range(raw: &str, marker_end: usize) -> Option<(usize, usize)> {
    let bytes = raw.as_bytes();
    let mut cursor = marker_end;
    if bytes
        .get(cursor)
        .is_some_and(|byte| matches!(byte, b'"' | b'\''))
    {
        cursor += 1;
    }
    while bytes.get(cursor).is_some_and(u8::is_ascii_whitespace) {
        cursor += 1;
    }
    if !bytes
        .get(cursor)
        .is_some_and(|byte| matches!(byte, b':' | b'='))
    {
        return None;
    }
    cursor += 1;
    while bytes.get(cursor).is_some_and(u8::is_ascii_whitespace) {
        cursor += 1;
    }

    let quote = bytes
        .get(cursor)
        .copied()
        .filter(|byte| matches!(byte, b'"' | b'\''));
    if quote.is_some() {
        cursor += 1;
    }
    let start = cursor;
    let end = if let Some(quote) = quote {
        bytes[start..]
            .iter()
            .position(|byte| *byte == quote)
            .map(|offset| start + offset)
            .unwrap_or(bytes.len())
    } else if raw[start..].to_ascii_lowercase().starts_with("bearer ") {
        secret_token_end(bytes, start + "bearer ".len())
    } else {
        secret_token_end(bytes, start)
    };
    (end > start).then_some((start, end))
}

fn secret_token_end(bytes: &[u8], start: usize) -> usize {
    bytes[start..]
        .iter()
        .position(|byte| {
            byte.is_ascii_whitespace() || matches!(byte, b'"' | b'\'' | b',' | b';' | b'}' | b']')
        })
        .map(|offset| start + offset)
        .unwrap_or(bytes.len())
}

#[derive(Debug, Default)]
struct UpstreamErrorDetails {
    error_type: Option<String>,
    message: Option<String>,
    resets_in_seconds: Option<u64>,
    resets_at: Option<i64>,
}

#[derive(Clone, Copy)]
struct CapacityCaptureContext<'a> {
    storage: &'a Storage,
    provider: &'static str,
    credential_profile_id: Option<&'a str>,
    model: &'a str,
}

impl<'a> CapacityCaptureContext<'a> {
    fn openai(
        storage: &'a Storage,
        credential_profile_id: Option<&'a str>,
        model: &'a str,
    ) -> Self {
        Self {
            storage,
            provider: "openai",
            credential_profile_id,
            model,
        }
    }

    fn anthropic(
        storage: &'a Storage,
        credential_profile_id: Option<&'a str>,
        model: &'a str,
    ) -> Self {
        Self {
            storage,
            provider: "anthropic",
            credential_profile_id,
            model,
        }
    }

    fn capture_headers(&self, headers: &HeaderMap, captured_at: chrono::DateTime<Utc>) {
        let records = match self.provider {
            "openai" => parse_openai_usage_headers(
                headers,
                self.credential_profile_id,
                self.model,
                captured_at,
            ),
            "anthropic" => parse_anthropic_usage_headers(
                headers,
                self.credential_profile_id,
                self.model,
                captured_at,
            ),
            _ => Vec::new(),
        };
        for record in records {
            let _ = self.storage.upsert_usage_window(&record);
        }
    }

    fn capture_rate_limit_error(
        &self,
        status: StatusCode,
        raw_body: &str,
        retry_after_seconds: Option<i64>,
        formatted_message: &str,
        recorded_at: chrono::DateTime<Utc>,
    ) {
        let details = parse_upstream_error_details(raw_body);
        let error_type = details
            .as_ref()
            .and_then(|details| details.error_type.as_deref());
        if !is_rate_limit_error(status, error_type) {
            return;
        }

        let reason = error_type
            .map(str::trim)
            .filter(|reason| !reason.is_empty())
            .unwrap_or("rate_limit");
        let next_attempt_at = details
            .as_ref()
            .and_then(|details| details.resets_at)
            .and_then(|timestamp| Utc.timestamp_opt(timestamp, 0).single())
            .or_else(|| {
                details
                    .as_ref()
                    .and_then(|details| details.resets_in_seconds)
                    .and_then(|seconds| i64::try_from(seconds).ok())
                    .and_then(chrono::Duration::try_seconds)
                    .and_then(|duration| recorded_at.checked_add_signed(duration))
            })
            .or_else(|| {
                retry_after_seconds
                    .and_then(chrono::Duration::try_seconds)
                    .and_then(|duration| recorded_at.checked_add_signed(duration))
            })
            .map(|timestamp| timestamp.to_rfc3339());
        let cooldown = storage::ProfileCooldown {
            provider: self.provider.to_string(),
            credential_profile_id: self.credential_profile_id.map(ToString::to_string),
            reason: reason.to_string(),
            next_attempt_at,
            last_error: Some(formatted_message.to_string()),
            recorded_at: recorded_at.to_rfc3339(),
        };
        let _ = self.storage.upsert_profile_cooldown(&cooldown);
    }

    fn clear_cooldown(&self) {
        let _ = self
            .storage
            .clear_profile_cooldown(self.provider, self.credential_profile_id);
    }
}

fn parse_openai_usage_headers(
    headers: &HeaderMap,
    credential_profile_id: Option<&str>,
    model: &str,
    captured_at: chrono::DateTime<Utc>,
) -> Vec<storage::UsageWindow> {
    let retry_after_seconds = parse_retry_after_seconds(headers);
    [
        (
            "requests",
            "x-ratelimit-limit-requests",
            "x-ratelimit-remaining-requests",
            "x-ratelimit-reset-requests",
        ),
        (
            "tokens",
            "x-ratelimit-limit-tokens",
            "x-ratelimit-remaining-tokens",
            "x-ratelimit-reset-tokens",
        ),
    ]
    .into_iter()
    .filter_map(|(window_kind, limit_name, remaining_name, reset_name)| {
        usage_window_from_headers(
            headers,
            "openai",
            credential_profile_id,
            model,
            window_kind,
            limit_name,
            remaining_name,
            reset_name,
            retry_after_seconds,
            &captured_at,
            parse_openai_reset_at,
        )
    })
    .collect()
}

fn parse_anthropic_usage_headers(
    headers: &HeaderMap,
    credential_profile_id: Option<&str>,
    model: &str,
    captured_at: chrono::DateTime<Utc>,
) -> Vec<storage::UsageWindow> {
    let retry_after_seconds = parse_retry_after_seconds(headers);
    [
        (
            "requests",
            "anthropic-ratelimit-requests-limit",
            "anthropic-ratelimit-requests-remaining",
            "anthropic-ratelimit-requests-reset",
        ),
        (
            "input_tokens",
            "anthropic-ratelimit-input-tokens-limit",
            "anthropic-ratelimit-input-tokens-remaining",
            "anthropic-ratelimit-input-tokens-reset",
        ),
        (
            "output_tokens",
            "anthropic-ratelimit-output-tokens-limit",
            "anthropic-ratelimit-output-tokens-remaining",
            "anthropic-ratelimit-output-tokens-reset",
        ),
    ]
    .into_iter()
    .filter_map(|(window_kind, limit_name, remaining_name, reset_name)| {
        usage_window_from_headers(
            headers,
            "anthropic",
            credential_profile_id,
            model,
            window_kind,
            limit_name,
            remaining_name,
            reset_name,
            retry_after_seconds,
            &captured_at,
            parse_anthropic_reset_at,
        )
    })
    .collect()
}

#[allow(clippy::too_many_arguments)]
fn usage_window_from_headers(
    headers: &HeaderMap,
    provider: &str,
    credential_profile_id: Option<&str>,
    model: &str,
    window_kind: &str,
    limit_name: &str,
    remaining_name: &str,
    reset_name: &str,
    retry_after_seconds: Option<i64>,
    captured_at: &chrono::DateTime<Utc>,
    parse_reset_at: fn(Option<&HeaderValue>, &chrono::DateTime<Utc>) -> Option<String>,
) -> Option<storage::UsageWindow> {
    let limit_header = headers.get(limit_name);
    let remaining_header = headers.get(remaining_name);
    let reset_header = headers.get(reset_name);
    if limit_header.is_none() && remaining_header.is_none() && reset_header.is_none() {
        return None;
    }

    let limit_value = parse_integer_header(limit_header);
    let remaining = parse_integer_header(remaining_header);
    let used = limit_value
        .zip(remaining)
        .and_then(|(limit, remaining)| limit.checked_sub(remaining))
        .filter(|used| *used >= 0);
    Some(storage::UsageWindow {
        id: None,
        provider: provider.to_string(),
        credential_profile_id: credential_profile_id.map(ToString::to_string),
        model: Some(model.to_string()),
        window_kind: window_kind.to_string(),
        limit_value,
        remaining,
        used,
        resets_at: parse_reset_at(reset_header, captured_at),
        retry_after_seconds,
        source: "headers".to_string(),
        confidence: "known".to_string(),
        captured_at: captured_at.to_rfc3339(),
    })
}

fn parse_integer_header(value: Option<&HeaderValue>) -> Option<i64> {
    value?.to_str().ok()?.trim().parse().ok()
}

fn parse_retry_after_seconds(headers: &HeaderMap) -> Option<i64> {
    parse_integer_header(headers.get("retry-after")).filter(|seconds| *seconds >= 0)
}

fn parse_openai_reset_at(
    value: Option<&HeaderValue>,
    captured_at: &chrono::DateTime<Utc>,
) -> Option<String> {
    let duration = parse_rate_limit_duration(value?.to_str().ok()?)?;
    captured_at
        .checked_add_signed(duration)
        .map(|timestamp| timestamp.to_rfc3339())
}

fn parse_anthropic_reset_at(
    value: Option<&HeaderValue>,
    _captured_at: &chrono::DateTime<Utc>,
) -> Option<String> {
    chrono::DateTime::parse_from_rfc3339(value?.to_str().ok()?.trim())
        .ok()
        .map(|timestamp| timestamp.with_timezone(&Utc).to_rfc3339())
}

fn parse_rate_limit_duration(raw: &str) -> Option<chrono::Duration> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }

    let bytes = raw.as_bytes();
    let mut cursor = 0;
    let mut total_milliseconds = 0_i64;
    while cursor < bytes.len() {
        let number_start = cursor;
        while bytes.get(cursor).is_some_and(u8::is_ascii_digit) {
            cursor += 1;
        }
        if cursor == number_start {
            return None;
        }
        let amount = raw[number_start..cursor].parse::<i64>().ok()?;
        let unit_milliseconds = if raw[cursor..].starts_with("ms") {
            cursor += 2;
            1
        } else {
            let unit = *bytes.get(cursor)?;
            cursor += 1;
            match unit {
                b's' => 1_000,
                b'm' => 60_000,
                b'h' => 3_600_000,
                b'd' => 86_400_000,
                _ => return None,
            }
        };
        total_milliseconds =
            total_milliseconds.checked_add(amount.checked_mul(unit_milliseconds)?)?;
    }
    chrono::Duration::try_milliseconds(total_milliseconds)
}

fn upstream_http_chat_error(status: StatusCode, raw_body: &str, partial_text: String) -> ChatError {
    let message = format_upstream_error_message(status, raw_body);
    if is_retryable_upstream_http_error(status, raw_body) {
        if partial_text.is_empty() {
            ChatError::transient(message)
        } else {
            ChatError::transient_with_partial(message, partial_text)
        }
    } else if partial_text.is_empty() {
        ChatError::new(message)
    } else {
        ChatError::with_partial(message, partial_text)
    }
}

fn is_retryable_upstream_http_error(status: StatusCode, raw_body: &str) -> bool {
    let error_type = parse_upstream_error_details(raw_body)
        .and_then(|details| details.error_type)
        .filter(|error_type| !error_type.trim().is_empty());
    if matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN)
        || is_rate_limit_error(status, error_type.as_deref())
    {
        return false;
    }
    matches!(status.as_u16(), 400 | 408 | 409 | 425 | 500..=599)
}

fn format_upstream_error_message(status: StatusCode, raw_body: &str) -> String {
    let details = parse_upstream_error_details(raw_body);
    let error_type = details
        .as_ref()
        .and_then(|details| details.error_type.as_deref());
    let envelope_message = details
        .as_ref()
        .and_then(|details| details.message.as_deref())
        .filter(|message| !message.trim().is_empty());

    let mut message = if matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN) {
        let mut message = "Your ChatGPT session expired — reconnect it in Settings.".to_string();
        if let Some(envelope_message) = envelope_message {
            message.push(' ');
            message.push_str(envelope_message);
        }
        message
    } else if is_rate_limit_error(status, error_type) {
        let mut message = "You've hit your ChatGPT plan's usage limit.".to_string();
        if let Some(reset_seconds) = details.as_ref().and_then(reset_seconds_from_details) {
            message.push_str(" Resets in about ");
            message.push_str(&format_reset_duration(reset_seconds));
            message.push('.');
        }
        message
    } else if let Some(envelope_message) = envelope_message {
        envelope_message.to_string()
    } else if details.is_some() {
        "Something went wrong while talking to the model. This is usually temporary — try again."
            .to_string()
    } else {
        clean_error_body(raw_body)
    };

    message.push_str(&format_technical_error_suffix(status, error_type));
    message
}

fn parse_upstream_error_details(raw_body: &str) -> Option<UpstreamErrorDetails> {
    let root: Value = serde_json::from_str(raw_body).ok()?;
    let envelope = root
        .get("error")
        .filter(|error| error.is_object())
        .unwrap_or(&root);
    let field = |name| envelope.get(name).or_else(|| root.get(name));

    Some(UpstreamErrorDetails {
        error_type: field("type")
            .and_then(Value::as_str)
            .map(ToString::to_string),
        message: field("message")
            .and_then(Value::as_str)
            .or_else(|| field("detail").and_then(Value::as_str))
            .map(ToString::to_string),
        resets_in_seconds: field("resets_in_seconds").and_then(json_u64),
        resets_at: field("resets_at").and_then(json_i64),
    })
}

fn json_u64(value: &Value) -> Option<u64> {
    value
        .as_u64()
        .or_else(|| value.as_i64().and_then(|value| u64::try_from(value).ok()))
        .or_else(|| value.as_str().and_then(|value| value.parse().ok()))
}

fn json_i64(value: &Value) -> Option<i64> {
    value
        .as_i64()
        .or_else(|| value.as_u64().and_then(|value| i64::try_from(value).ok()))
        .or_else(|| value.as_str().and_then(|value| value.parse().ok()))
}

fn reset_seconds_from_details(details: &UpstreamErrorDetails) -> Option<u64> {
    details.resets_in_seconds.or_else(|| {
        details.resets_at.and_then(|resets_at| {
            u64::try_from(resets_at.saturating_sub(Local::now().timestamp()))
                .ok()
                .filter(|seconds| *seconds > 0)
        })
    })
}

fn format_reset_duration(seconds: u64) -> String {
    let minutes = seconds / 60;
    if minutes >= 60 {
        format!("{}h {}m", minutes / 60, minutes % 60)
    } else {
        format!("{minutes}m")
    }
}

fn is_rate_limit_error(status: StatusCode, error_type: Option<&str>) -> bool {
    if status == StatusCode::TOO_MANY_REQUESTS {
        return true;
    }
    let Some(error_type) = error_type else {
        return false;
    };
    let error_type = error_type.to_ascii_lowercase();
    error_type.contains("rate_limit")
        || error_type.contains("usage_limit")
        || error_type.contains("quota")
        || error_type.contains("too_many_requests")
}

fn clean_error_body(raw_body: &str) -> String {
    let cleaned = raw_body.split_whitespace().collect::<Vec<_>>().join(" ");
    if cleaned.is_empty() {
        return "Something went wrong while talking to the model. This is usually temporary — try again."
            .to_string();
    }

    let mut chars = cleaned.chars();
    let truncated = chars
        .by_ref()
        .take(MAX_ERROR_FALLBACK_CHARS)
        .collect::<String>();
    if chars.next().is_some() {
        format!("{truncated}…")
    } else {
        truncated
    }
}

fn format_technical_error_suffix(status: StatusCode, error_type: Option<&str>) -> String {
    match error_type
        .map(str::trim)
        .filter(|error_type| !error_type.is_empty())
    {
        Some(error_type) => format!(" (HTTP {} · {error_type})", status.as_u16()),
        None => format!(" (HTTP {})", status.as_u16()),
    }
}

#[allow(clippy::too_many_arguments)]
async fn execute_tool_call(
    app: &AppHandle,
    chat: &ChatManager,
    storage: &Storage,
    conversation_id: &str,
    turn_id: &str,
    default_cwd: &str,
    effective_policy: exec::ApprovalPolicy,
    write_slug_basis: &str,
    extensions: &ExtensionManager,
    mcp_tools: &[extensions::McpToolDescriptor],
    call: ToolCallEvent,
    cancel: Arc<AtomicBool>,
    subagents: Option<&SubagentRegistry>,
) -> ChatResult<Value> {
    let cwd = Path::new(default_cwd);
    if cwd.is_absolute() {
        projects::validate_registered_project_root(cwd).map_err(ChatError::new)?;
    }
    match call.kind {
        ToolCallKind::Function if call.name == "shell_command" => {
            execute_shell_command_call(
                app,
                chat,
                storage,
                app.state::<PreferencesManager>().inner(),
                conversation_id,
                turn_id,
                default_cwd,
                effective_policy,
                write_slug_basis,
                call,
                ToolInvocation::Model,
                cancel,
            )
            .await
        }
        ToolCallKind::Custom if call.name == "apply_patch" => {
            execute_apply_patch_call(
                app,
                chat,
                storage,
                app.state::<PreferencesManager>().inner(),
                conversation_id,
                turn_id,
                default_cwd,
                effective_policy,
                write_slug_basis,
                call,
                ToolInvocation::Model,
                cancel,
                false,
            )
            .await
        }
        ToolCallKind::Function if call.name == "view_image" => {
            execute_view_image_call(app, conversation_id, default_cwd, call)
        }
        ToolCallKind::Function if call.name == "update_plan" => {
            execute_update_plan_call(app, storage, conversation_id, call)
        }
        ToolCallKind::Function if is_subagent_tool(&call.name) => {
            if let Some(subagents) = subagents {
                execute_subagent_tool_call(subagents, call, cancel).await
            } else {
                unsupported_tool_output(app, conversation_id, call)
            }
        }
        ToolCallKind::Function if is_browser_tool(&call.name) => {
            execute_browser_tool_call(
                app,
                chat,
                storage,
                conversation_id,
                default_cwd,
                effective_policy,
                call,
                cancel,
            )
            .await
        }
        ToolCallKind::Function => {
            if let Some(tool) = mcp_tools.iter().find(|tool| tool.wire_name == call.name) {
                execute_mcp_tool_call(
                    app,
                    chat,
                    conversation_id,
                    default_cwd,
                    extensions,
                    tool,
                    call,
                    cancel,
                )
                .await
            } else {
                unsupported_tool_output(app, conversation_id, call)
            }
        }
        _ => unsupported_tool_output(app, conversation_id, call),
    }
}

fn is_browser_tool(name: &str) -> bool {
    matches!(
        name,
        "browser_open"
            | "browser_snapshot"
            | "browser_click"
            | "browser_type"
            | "browser_screenshot"
            | "browser_tabs"
            | "browser_select_tab"
            | "browser_close_tab"
            | "browser_cdp"
    )
}

fn is_subagent_tool(name: &str) -> bool {
    matches!(
        name,
        "spawn_agent"
            | "send_message"
            | "followup_task"
            | "wait_agent"
            | "list_agents"
            | "interrupt_agent"
    )
}

async fn execute_subagent_tool_call(
    registry: &SubagentRegistry,
    call: ToolCallEvent,
    cancel: Arc<AtomicBool>,
) -> ChatResult<Value> {
    let result = match call.name.as_str() {
        "spawn_agent" => serde_json::from_str::<SpawnAgentArgs>(&call.input)
            .map_err(|error| format!("Invalid spawn_agent arguments: {error}"))
            .and_then(|args| registry.spawn(args)),
        "send_message" => serde_json::from_str::<AgentMessageArgs>(&call.input)
            .map_err(|error| format!("Invalid send_message arguments: {error}"))
            .and_then(|args| registry.send_message(args)),
        "followup_task" => serde_json::from_str::<AgentMessageArgs>(&call.input)
            .map_err(|error| format!("Invalid followup_task arguments: {error}"))
            .and_then(|args| registry.followup_task(args)),
        "wait_agent" => match serde_json::from_str::<WaitAgentArgs>(&call.input) {
            Ok(args) => registry
                .wait(args, cancel)
                .await
                .map_err(|error| error.message),
            Err(error) => Err(format!("Invalid wait_agent arguments: {error}")),
        },
        "list_agents" => serde_json::from_str::<ListAgentsArgs>(&call.input)
            .map_err(|error| format!("Invalid list_agents arguments: {error}"))
            .and_then(|args| registry.list(args)),
        "interrupt_agent" => serde_json::from_str::<AgentTargetArgs>(&call.input)
            .map_err(|error| format!("Invalid interrupt_agent arguments: {error}"))
            .and_then(|args| registry.interrupt(args)),
        _ => Err(format!("Unsupported subagent tool `{}`", call.name)),
    };
    let output = result.unwrap_or_else(|error| error);
    Ok(tool_call_output_item(&call, &output))
}

fn execute_update_plan_call(
    app: &impl ChatDeltaEmitter,
    storage: &Storage,
    conversation_id: &str,
    call: ToolCallEvent,
) -> ChatResult<Value> {
    let parsed: UpdatePlanArgs = match serde_json::from_str(&call.input) {
        Ok(parsed) => parsed,
        Err(error) => {
            return Ok(tool_call_output_item(
                &call,
                &format!("Invalid update_plan arguments: {error}"),
            ));
        }
    };
    let in_progress = parsed
        .plan
        .iter()
        .filter(|item| item.status == "in_progress")
        .count();
    let invalid = parsed.plan.iter().find(|item| {
        item.step.trim().is_empty()
            || !matches!(
                item.status.as_str(),
                "pending" | "in_progress" | "completed"
            )
    });
    if in_progress > 1 || invalid.is_some() {
        let output = if in_progress > 1 {
            "Invalid update_plan arguments: at most one step can be in_progress"
        } else {
            "Invalid update_plan arguments: every step needs text and a valid status"
        };
        return Ok(tool_call_output_item(&call, output));
    }
    let persisted = storage
        .upsert_conversation_task_plan(conversation_id, parsed.explanation.as_deref(), &parsed.plan)
        .map_err(|error| ChatError::new(error.to_string()))?;
    emit_delta(
        app,
        ChatDeltaPayload {
            explanation: persisted.explanation,
            plan: Some(persisted.plan),
            ..ChatDeltaPayload::new(conversation_id, ChatDeltaKind::TaskList)
        },
    )
    .map_err(ChatError::new)?;
    Ok(tool_call_output_item(&call, "Plan updated"))
}

fn execute_view_image_call(
    app: &impl ChatDeltaEmitter,
    conversation_id: &str,
    project_root: &str,
    call: ToolCallEvent,
) -> ChatResult<Value> {
    let parsed: ViewImageArgs = match serde_json::from_str(&call.input) {
        Ok(parsed) => parsed,
        Err(error) => {
            return Ok(tool_call_output_item(
                &call,
                &format!("Invalid view_image arguments: {error}"),
            ));
        }
    };
    let display_path = parsed.path.trim();
    emit_named_tool_call(
        app,
        conversation_id,
        &call.call_id,
        &format!("view_image {display_path}"),
        "image",
        Some(project_root),
        "running",
        None,
    )
    .map_err(ChatError::new)?;

    match load_local_image(Path::new(project_root), &parsed.path) {
        Ok(image) => {
            let confirmation = format!("attached local image {}", image.path.display());
            emit_delta(
                app,
                ChatDeltaPayload {
                    call_id: Some(call.call_id.clone()),
                    tail: Some(confirmation.clone()),
                    ..ChatDeltaPayload::new(conversation_id, ChatDeltaKind::ToolOutput)
                },
            )
            .map_err(ChatError::new)?;
            emit_tool_result(
                app,
                conversation_id,
                &call.call_id,
                "completed",
                None,
                None,
                false,
            )
            .map_err(ChatError::new)?;
            Ok(view_image_tool_call_output_item(
                &call,
                &image,
                &confirmation,
            ))
        }
        Err(error) => {
            let output = format!("view_image failed: {error}");
            emit_delta(
                app,
                ChatDeltaPayload {
                    call_id: Some(call.call_id.clone()),
                    tail: Some(output.clone()),
                    ..ChatDeltaPayload::new(conversation_id, ChatDeltaKind::ToolOutput)
                },
            )
            .map_err(ChatError::new)?;
            emit_tool_result(
                app,
                conversation_id,
                &call.call_id,
                "failed",
                None,
                None,
                false,
            )
            .map_err(ChatError::new)?;
            Ok(tool_call_output_item(&call, &output))
        }
    }
}

fn load_local_image(project_root: &Path, requested_path: &str) -> Result<LocalImage, String> {
    let path = exec::resolve_project_read_path(project_root, requested_path)?;
    let media_type = match Path::new(requested_path.trim())
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        _ => {
            return Err(format!(
                "`{}` has an unsupported image extension; use png, jpg, jpeg, gif, or webp",
                requested_path.trim()
            ))
        }
    };
    let metadata = fs::metadata(&path).map_err(|error| {
        format!(
            "failed to inspect local image `{}`: {error}",
            path.display()
        )
    })?;
    if !metadata.is_file() {
        return Err(format!(
            "local image path `{}` is not a regular file",
            path.display()
        ));
    }
    if metadata.len() > MAX_VIEW_IMAGE_BYTES as u64 {
        return Err(format!(
            "local image `{}` exceeds the {} MiB limit",
            path.display(),
            MAX_VIEW_IMAGE_BYTES / (1024 * 1024)
        ));
    }

    let file = fs::File::open(&path)
        .map_err(|error| format!("failed to open local image `{}`: {error}", path.display()))?;
    let mut data = Vec::with_capacity(metadata.len() as usize);
    file.take((MAX_VIEW_IMAGE_BYTES + 1) as u64)
        .read_to_end(&mut data)
        .map_err(|error| format!("failed to read local image `{}`: {error}", path.display()))?;
    if data.len() > MAX_VIEW_IMAGE_BYTES {
        return Err(format!(
            "local image `{}` exceeds the {} MiB limit",
            path.display(),
            MAX_VIEW_IMAGE_BYTES / (1024 * 1024)
        ));
    }

    Ok(LocalImage {
        path,
        media_type,
        data: STANDARD.encode(data),
    })
}

fn subagent_semaphore(concurrency_limit: Option<usize>) -> Option<Arc<Semaphore>> {
    concurrency_limit.map(|limit| Arc::new(Semaphore::new(limit.min(Semaphore::MAX_PERMITS))))
}

impl SubagentRegistry {
    fn new(
        context: SubagentSpawnContext,
        parent_context: Vec<Value>,
        concurrency_limit: Option<usize>,
    ) -> ChatResult<Self> {
        let persisted = context
            .app
            .state::<Storage>()
            .list_subagent_threads(&context.conversation_id)
            .map_err(|error| ChatError::new(error.to_string()))?;
        let records = persisted
            .into_iter()
            .map(|thread| {
                (
                    thread.agent_id.clone(),
                    SubagentRecord {
                        id: thread.agent_id,
                        task_name: thread.task_name,
                        last_message: thread.task,
                        status: SubagentStatus::from_storage(&thread.status),
                        result: thread.result,
                        error: thread.error,
                        mailbox: Vec::new(),
                        delivered: true,
                        cancel: Arc::new(AtomicBool::new(false)),
                        generation: thread.generation,
                    },
                )
            })
            .collect::<HashMap<_, _>>();
        Ok(Self {
            context,
            records: Arc::new(Mutex::new(records)),
            parent_context: Arc::new(Mutex::new(parent_context)),
            semaphore: subagent_semaphore(concurrency_limit),
            tool_semaphore: Arc::new(Semaphore::new(1)),
            usage: Arc::new(Mutex::new(None)),
        })
    }

    fn update_parent_context(&self, context: Vec<Value>) {
        *self
            .parent_context
            .lock()
            .expect("subagent context lock poisoned") = context;
    }

    fn persist_record(&self, record: &SubagentRecord) -> Result<(), String> {
        self.context
            .app
            .state::<Storage>()
            .upsert_subagent_thread(storage::SubagentThreadUpdate {
                agent_id: &record.id,
                conversation_id: &self.context.conversation_id,
                turn_id: &self.context.turn_id,
                task_name: &record.task_name,
                task: &record.last_message,
                status: record.status.as_str(),
                result: record.result.as_deref(),
                error: record.error.as_deref(),
                generation: record.generation,
            })
            .map_err(|error| error.to_string())
    }

    fn persist_message(
        &self,
        agent_id: &str,
        role: &str,
        content: &str,
        generation: u64,
    ) -> Result<(), String> {
        self.context
            .app
            .state::<Storage>()
            .append_subagent_message(agent_id, role, content, generation)
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    async fn tool_permit(
        &self,
        policy: exec::ApprovalPolicy,
        call: &ToolCallEvent,
    ) -> ChatResult<Option<tokio::sync::OwnedSemaphorePermit>> {
        let serialized = matches!(
            policy,
            exec::ApprovalPolicy::Ask
                | exec::ApprovalPolicy::OnRequest
                | exec::ApprovalPolicy::OnFailure
        ) || call.name.starts_with("mcp__")
            || is_browser_tool(&call.name);
        if !serialized || is_subagent_tool(&call.name) || call.name == "update_plan" {
            return Ok(None);
        }
        self.tool_semaphore
            .clone()
            .acquire_owned()
            .await
            .map(Some)
            .map_err(|_| ChatError::new("Subagent tool scheduler closed"))
    }

    fn spawn(&self, args: SpawnAgentArgs) -> Result<String, String> {
        validate_subagent_task_name(&args.task_name)?;
        let fork_turns = normalize_fork_turns(args.fork_turns.as_deref())?;
        let canonical_name = format!("/root/{}", args.task_name);
        let agent_id = storage::new_uuid_v4();
        let record = SubagentRecord {
            id: agent_id.clone(),
            task_name: canonical_name.clone(),
            last_message: args.message.clone(),
            status: SubagentStatus::Pending,
            result: None,
            error: None,
            mailbox: Vec::new(),
            delivered: false,
            cancel: Arc::new(AtomicBool::new(false)),
            generation: 1,
        };
        {
            let mut records = self.records.lock().expect("subagent lock poisoned");
            if records
                .values()
                .any(|record| record.task_name == canonical_name)
            {
                return Err(format!("Subagent task `{}` already exists", args.task_name));
            }
            self.persist_record(&record)?;
            self.persist_message(&agent_id, "user", &args.message, record.generation)?;
            records.insert(agent_id.clone(), record);
        }
        emit_named_tool_call(
            &self.context.app,
            &self.context.conversation_id,
            &agent_id,
            &canonical_name,
            "subagent",
            Some(&self.context.project_root),
            "pending",
            Some(&args.message),
        )?;
        let context = self.forked_context(&fork_turns);
        self.launch(agent_id.clone(), context, args.message);
        serde_json::to_string(&json!({
            "agent_id": agent_id,
            "task_name": canonical_name,
            "status": "pending"
        }))
        .map_err(|error| format!("failed to encode spawn_agent result: {error}"))
    }

    fn launch(&self, agent_id: String, forked_context: Vec<Value>, message: String) {
        let registry = self.clone();
        tauri::async_runtime::spawn(async move {
            let permit = match registry.semaphore.clone() {
                Some(semaphore) => match semaphore.acquire_owned().await {
                    Ok(permit) => Some(permit),
                    Err(_) => {
                        registry.finish_run(
                            &agent_id,
                            0,
                            Err(ChatError::new("Subagent scheduler closed")),
                        );
                        return;
                    }
                },
                None => None,
            };
            let run_state = {
                let mut records = registry.records.lock().expect("subagent lock poisoned");
                let Some(record) = records.get_mut(&agent_id) else {
                    return;
                };
                if record.cancel.load(Ordering::SeqCst) {
                    record.status = SubagentStatus::Interrupted;
                    None
                } else {
                    record.status = SubagentStatus::Running;
                    Some((
                        record.cancel.clone(),
                        record.generation,
                        record.task_name.clone(),
                    ))
                }
            };
            if let Some(record) = registry
                .records
                .lock()
                .expect("subagent lock poisoned")
                .get(&agent_id)
                .cloned()
            {
                if let Err(error) = registry.persist_record(&record) {
                    eprintln!("failed to persist subagent state: {error}");
                }
            }
            let Some((cancel, generation, task_name)) = run_state else {
                let _ = emit_tool_result(
                    &registry.context.app,
                    &registry.context.conversation_id,
                    &agent_id,
                    "interrupted",
                    None,
                    None,
                    false,
                );
                return;
            };
            let _ = emit_named_tool_call(
                &registry.context.app,
                &registry.context.conversation_id,
                &agent_id,
                &task_name,
                "subagent",
                Some(&registry.context.project_root),
                "running",
                Some(&message),
            );
            let parent_cancel = registry.context.parent_cancel.clone();
            let combined_cancel = cancel.clone();
            let (watcher_done_tx, watcher_done_rx) = oneshot::channel::<()>();
            tauri::async_runtime::spawn(async move {
                tokio::select! {
                    _ = wait_for_cancel(parent_cancel) => {
                        combined_cancel.store(true, Ordering::SeqCst);
                    }
                    _ = watcher_done_rx => {}
                }
            });
            let result = run_subagent_task(
                registry.clone(),
                agent_id.clone(),
                task_name,
                forked_context,
                message,
                cancel,
            )
            .await;
            let _ = watcher_done_tx.send(());
            drop(permit);
            registry.finish_run(&agent_id, generation, result);
        });
    }

    fn finish_run(&self, agent_id: &str, generation: u64, result: ChatResult<StreamSuccess>) {
        let (status, output, usage) = match result {
            Ok(success) => (
                SubagentStatus::Completed,
                success.assistant_text,
                success.usage,
            ),
            Err(error) => {
                let message = if error.partial_text.trim().is_empty() {
                    error.message
                } else {
                    format!(
                        "{}\n\nPartial result:\n{}",
                        error.message, error.partial_text
                    )
                };
                (SubagentStatus::Errored, message, None)
            }
        };
        let (final_status, persisted_record) = {
            let mut records = self.records.lock().expect("subagent lock poisoned");
            let Some(record) = records.get_mut(agent_id) else {
                return;
            };
            if generation != 0 && record.generation != generation {
                return;
            }
            let final_status = if record.cancel.load(Ordering::SeqCst) {
                SubagentStatus::Interrupted
            } else {
                status
            };
            record.status = final_status;
            record.delivered = false;
            if final_status == SubagentStatus::Errored {
                record.error = Some(output.clone());
                record.result = None;
            } else {
                record.result = Some(output.clone());
                record.error = None;
            }
            (final_status, record.clone())
        };
        if let Err(error) = self.persist_record(&persisted_record) {
            eprintln!("failed to persist completed subagent: {error}");
        }
        if !output.trim().is_empty() {
            let role = if final_status == SubagentStatus::Errored {
                "error"
            } else {
                "assistant"
            };
            if let Err(error) =
                self.persist_message(agent_id, role, &output, persisted_record.generation)
            {
                eprintln!("failed to persist subagent transcript: {error}");
            }
        }
        if let Some(usage) = usage {
            let mut accumulated = self.usage.lock().expect("subagent usage lock poisoned");
            let _ = merge_token_usage(&mut accumulated, Some(usage));
        }
        let tail = output.chars().take(8_000).collect::<String>();
        if !tail.is_empty() {
            let _ = emit_delta(
                &self.context.app,
                ChatDeltaPayload {
                    call_id: Some(agent_id.to_string()),
                    tail: Some(tail),
                    ..ChatDeltaPayload::new(
                        &self.context.conversation_id,
                        ChatDeltaKind::ToolOutput,
                    )
                },
            );
        }
        let _ = emit_tool_result(
            &self.context.app,
            &self.context.conversation_id,
            agent_id,
            final_status.as_str(),
            None,
            None,
            false,
        );
    }

    fn forked_context(&self, fork_turns: &str) -> Vec<Value> {
        let parent = self
            .parent_context
            .lock()
            .expect("subagent context lock poisoned");
        if fork_turns == "all" {
            return parent.clone();
        }
        if fork_turns == "none" {
            return self.context.base_context.clone();
        }
        let turns = fork_turns.parse::<usize>().unwrap_or(1);
        let tail_items = turns.saturating_mul(2);
        let start = parent.len().saturating_sub(tail_items);
        let mut selected = self.context.base_context.clone();
        selected.extend(parent[start..].iter().cloned());
        selected
    }

    fn drain_mailbox(&self, agent_id: &str) -> Vec<String> {
        let mut records = self.records.lock().expect("subagent lock poisoned");
        let Some(record) = records.get_mut(agent_id) else {
            return Vec::new();
        };
        std::mem::take(&mut record.mailbox)
    }

    fn send_message(&self, args: AgentMessageArgs) -> Result<String, String> {
        let id = self.resolve_target(&args.target)?;
        let (status, generation) = {
            let mut records = self.records.lock().expect("subagent lock poisoned");
            let record = records
                .get_mut(&id)
                .ok_or_else(|| format!("Unknown subagent `{}`", args.target))?;
            record.mailbox.push(args.message.clone());
            (record.status.as_str().to_string(), record.generation)
        };
        self.persist_message(&id, "user", &args.message, generation)?;
        serde_json::to_string(&json!({ "target": id, "queued": true, "status": status }))
            .map_err(|error| format!("failed to encode send_message result: {error}"))
    }

    fn followup_task(&self, args: AgentMessageArgs) -> Result<String, String> {
        let id = self.resolve_target(&args.target)?;
        let launch = {
            let mut records = self.records.lock().expect("subagent lock poisoned");
            let record = records
                .get_mut(&id)
                .ok_or_else(|| format!("Unknown subagent `{}`", args.target))?;
            record.last_message = args.message.clone();
            if record.status.is_final() {
                let prior = record
                    .result
                    .clone()
                    .or_else(|| record.error.clone())
                    .unwrap_or_default();
                record.status = SubagentStatus::Pending;
                record.result = None;
                record.error = None;
                record.delivered = false;
                record.cancel = Arc::new(AtomicBool::new(false));
                record.generation = record.generation.saturating_add(1);
                Some(prior)
            } else {
                record.mailbox.push(args.message.clone());
                None
            }
        };
        let persisted_record = self
            .records
            .lock()
            .expect("subagent lock poisoned")
            .get(&id)
            .cloned()
            .ok_or_else(|| format!("Unknown subagent `{}`", args.target))?;
        self.persist_record(&persisted_record)?;
        self.persist_message(&id, "user", &args.message, persisted_record.generation)?;
        if let Some(prior) = launch {
            let mut context = self.forked_context("all");
            let followup = if prior.trim().is_empty() {
                args.message.clone()
            } else {
                format!(
                    "Previous result:\n{prior}\n\nFollow-up task:\n{}",
                    args.message
                )
            };
            context.push(user_input_message(&followup));
            self.launch(id.clone(), context, args.message);
        }
        serde_json::to_string(&json!({ "target": id, "accepted": true }))
            .map_err(|error| format!("failed to encode followup_task result: {error}"))
    }

    fn interrupt(&self, args: AgentTargetArgs) -> Result<String, String> {
        let id = self.resolve_target(&args.target)?;
        let (previous_status, persisted_record) = {
            let mut records = self.records.lock().expect("subagent lock poisoned");
            let record = records
                .get_mut(&id)
                .ok_or_else(|| format!("Unknown subagent `{}`", args.target))?;
            let previous_status = record.status.as_str().to_string();
            record.cancel.store(true, Ordering::SeqCst);
            if !record.status.is_final() {
                record.status = SubagentStatus::Interrupted;
                record.error = Some("Interrupted by the user.".to_string());
            }
            (previous_status, record.clone())
        };
        self.persist_record(&persisted_record)?;
        serde_json::to_string(&json!({
            "target": id,
            "previous_status": previous_status
        }))
        .map_err(|error| format!("failed to encode interrupt_agent result: {error}"))
    }

    fn interrupt_all(&self) -> Result<usize, String> {
        let targets = self
            .records
            .lock()
            .expect("subagent lock poisoned")
            .values()
            .filter(|record| !record.status.is_final())
            .map(|record| record.id.clone())
            .collect::<Vec<_>>();
        for target in &targets {
            self.interrupt(AgentTargetArgs {
                target: target.clone(),
            })?;
        }
        Ok(targets.len())
    }

    fn list(&self, args: ListAgentsArgs) -> Result<String, String> {
        let records = self.records.lock().expect("subagent lock poisoned");
        let mut agents = records
            .values()
            .filter(|record| {
                args.path_prefix
                    .as_deref()
                    .map(|prefix| record.task_name.starts_with(prefix))
                    .unwrap_or(true)
            })
            .map(subagent_record_json)
            .collect::<Vec<_>>();
        agents.sort_by(|left, right| {
            left.get("task_name")
                .and_then(Value::as_str)
                .cmp(&right.get("task_name").and_then(Value::as_str))
        });
        serde_json::to_string(&json!({ "agents": agents }))
            .map_err(|error| format!("failed to encode list_agents result: {error}"))
    }

    async fn wait(&self, args: WaitAgentArgs, cancel: Arc<AtomicBool>) -> ChatResult<String> {
        let deadline = tokio::time::Instant::now() + args.timeout()?;
        loop {
            let updates = self.take_completed_updates();
            if !updates.is_empty() {
                return serde_json::to_string(&json!({
                    "message": "Subagent updates are ready.",
                    "timed_out": false,
                    "updates": updates
                }))
                .map_err(|error| ChatError::new(format!("wait_agent output failed: {error}")));
            }
            let has_running = self
                .records
                .lock()
                .expect("subagent lock poisoned")
                .values()
                .any(|record| !record.status.is_final());
            if !has_running {
                return Ok(json!({
                    "message": "No pending subagent updates.",
                    "timed_out": false,
                    "updates": []
                })
                .to_string());
            }
            if tokio::time::Instant::now() >= deadline {
                return Ok(json!({
                    "message": "Wait timed out; subagents are still working.",
                    "timed_out": true,
                    "updates": []
                })
                .to_string());
            }
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_millis(50)) => {},
                _ = wait_for_cancel(cancel.clone()) => return Err(ChatError::new("Chat request cancelled")),
            }
        }
    }

    async fn auto_updates(&self, cancel: Arc<AtomicBool>) -> ChatResult<Option<String>> {
        loop {
            let updates = self.take_completed_updates();
            if !updates.is_empty() {
                return Ok(Some(
                    serde_json::to_string(&json!({ "subagent_updates": updates })).map_err(
                        |error| ChatError::new(format!("subagent update encoding failed: {error}")),
                    )?,
                ));
            }
            let has_running = self
                .records
                .lock()
                .expect("subagent lock poisoned")
                .values()
                .any(|record| !record.status.is_final());
            if !has_running {
                return Ok(None);
            }
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_millis(50)) => {},
                _ = wait_for_cancel(cancel.clone()) => return Err(ChatError::new("Chat request cancelled")),
            }
        }
    }

    fn take_completed_updates(&self) -> Vec<Value> {
        let mut records = self.records.lock().expect("subagent lock poisoned");
        records
            .values_mut()
            .filter(|record| record.status.is_final() && !record.delivered)
            .map(|record| {
                record.delivered = true;
                subagent_record_json(record)
            })
            .collect()
    }

    fn take_usage(&self) -> Option<TokenUsage> {
        self.usage
            .lock()
            .expect("subagent usage lock poisoned")
            .take()
    }

    fn resolve_target(&self, target: &str) -> Result<String, String> {
        let canonical = if target.starts_with("/root/") {
            target.to_string()
        } else {
            format!("/root/{target}")
        };
        let records = self.records.lock().expect("subagent lock poisoned");
        if records.contains_key(target) {
            return Ok(target.to_string());
        }
        records
            .values()
            .find(|record| record.task_name == canonical)
            .map(|record| record.id.clone())
            .ok_or_else(|| format!("Unknown subagent `{target}`"))
    }
}

fn subagent_record_json(record: &SubagentRecord) -> Value {
    json!({
        "agent_id": record.id,
        "task_name": record.task_name,
        "status": record.status.as_str(),
        "last_task_message": record.last_message,
        "result": record.result,
        "error": record.error
    })
}

fn validate_subagent_task_name(task_name: &str) -> Result<(), String> {
    if task_name.is_empty()
        || !task_name
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
    {
        return Err(
            "spawn_agent task_name must use lowercase letters, digits, and underscores".to_string(),
        );
    }
    Ok(())
}

fn normalize_fork_turns(fork_turns: Option<&str>) -> Result<String, String> {
    let fork_turns = fork_turns
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("all");
    if matches!(fork_turns, "all" | "none") {
        return Ok(fork_turns.to_string());
    }
    if fork_turns
        .parse::<usize>()
        .ok()
        .filter(|value| *value > 0)
        .is_some()
    {
        return Ok(fork_turns.to_string());
    }
    Err("fork_turns must be `none`, `all`, or a positive integer string".to_string())
}

async fn run_subagent_task(
    registry: SubagentRegistry,
    agent_id: String,
    task_name: String,
    forked_context: Vec<Value>,
    message: String,
    cancel: Arc<AtomicBool>,
) -> ChatResult<StreamSuccess> {
    let instruction = format!(
        "You are subagent {task_name}. Complete the assigned task directly in the shared project. You may use shell, apply_patch, and enabled MCP tools. Do not spawn subagents. Return a concise result with changed files and verification.\n\nAssigned task:\n{message}"
    );
    match registry.context.backend.clone() {
        SubagentBackend::Responses { headers } => {
            let mut history = forked_context;
            history.push(user_input_message(&instruction));
            let headers = subagent_request_headers(headers, &agent_id)?;
            run_responses_subagent(registry, agent_id, history, headers, cancel).await
        }
        SubagentBackend::Claude { api_key } => {
            let mut messages = forked_context;
            messages.push(json!({ "role": "user", "content": instruction }));
            run_claude_subagent(registry, agent_id, messages, api_key, cancel).await
        }
    }
}

fn subagent_request_headers(mut headers: HeaderMap, agent_id: &str) -> ChatResult<HeaderMap> {
    if headers.contains_key("thread-id") {
        let session_id = storage::new_uuid_v4();
        for (name, value) in [
            ("session-id", session_id.as_str()),
            ("thread-id", agent_id),
            ("x-client-request-id", agent_id),
        ] {
            headers.insert(
                HeaderName::from_static(name),
                HeaderValue::from_str(value).map_err(|error| {
                    ChatError::new(format!("invalid subagent {name} header: {error}"))
                })?,
            );
        }
    }
    Ok(headers)
}

async fn run_responses_subagent(
    registry: SubagentRegistry,
    agent_id: String,
    mut history: Vec<Value>,
    headers: HeaderMap,
    cancel: Arc<AtomicBool>,
) -> ChatResult<StreamSuccess> {
    let mut assistant_text = String::new();
    let mut usage = None;
    let mut budget = StreamBudget::default();
    let mut tool_budget = HardToolBudget::default();
    let prompt_key = format!("{}:{agent_id}", registry.context.conversation_id);
    loop {
        let permissions = load_live_turn_permissions(
            &registry.context.app.state::<Storage>(),
            &registry.context.app.state::<PreferencesManager>(),
            &registry.context.conversation_id,
            Some(&registry.context.project_root),
        )?;
        let body = build_responses_request_body_for_profile(
            &prompt_key,
            history.clone(),
            &registry.context.model,
            &registry.context.effort,
            None,
            permissions.agent_mode_enabled,
            false,
            permissions.effective_policy,
            &registry.context.mcp_tools,
            false,
        );
        let mut request_tool_budget = ToolBudget::Hard(&mut tool_budget);
        let mut progress = StreamAttemptProgress::default();
        let turn = stream_single_response(
            &registry.context.app,
            &registry.context.client,
            &registry.context.sidecar,
            CapacityCaptureContext::openai(
                registry.context.app.state::<Storage>().inner(),
                registry.context.credential_profile_id.as_deref(),
                &registry.context.model,
            ),
            headers.clone(),
            &registry.context.headroom_project,
            &body,
            &registry.context.conversation_id,
            &mut history,
            &mut budget,
            &mut request_tool_budget,
            &mut progress,
            false,
            cancel.clone(),
        )
        .await?;
        assistant_text.push_str(&turn.assistant_text);
        merge_token_usage(&mut usage, turn.usage)?;
        let had_tool_calls = !turn.tool_calls.is_empty();
        for call in turn.tool_calls {
            let _tool_permit = registry
                .tool_permit(permissions.effective_policy, &call)
                .await?;
            let item = execute_tool_call(
                &registry.context.app,
                &registry.context.app.state::<ChatManager>(),
                &registry.context.app.state::<Storage>(),
                &registry.context.conversation_id,
                &registry.context.turn_id,
                &registry.context.project_root,
                permissions.effective_policy,
                &registry.context.write_slug_basis,
                &registry.context.extensions,
                &registry.context.mcp_tools,
                call,
                cancel.clone(),
                None,
            )
            .await?;
            history.push(item);
        }
        let messages = registry.drain_mailbox(&agent_id);
        if !messages.is_empty() {
            history.extend(
                messages
                    .into_iter()
                    .map(|message| user_input_message(&message)),
            );
            continue;
        }
        if !had_tool_calls {
            return Ok(StreamSuccess {
                assistant_text,
                usage,
                service_tier: None,
                fast_mode_fallback: false,
            });
        }
    }
}

async fn run_claude_subagent(
    registry: SubagentRegistry,
    agent_id: String,
    mut messages: Vec<Value>,
    api_key: String,
    cancel: Arc<AtomicBool>,
) -> ChatResult<StreamSuccess> {
    let mut assistant_text = String::new();
    let mut usage = None;
    let mut budget = StreamBudget::default();
    let mut tool_budget = HardToolBudget::default();
    loop {
        let permissions = load_live_turn_permissions(
            &registry.context.app.state::<Storage>(),
            &registry.context.app.state::<PreferencesManager>(),
            &registry.context.conversation_id,
            Some(&registry.context.project_root),
        )?;
        let body = build_claude_request_body_for_profile(
            &registry.context.model,
            &registry.context.effort,
            None,
            messages.clone(),
            permissions.agent_mode_enabled,
            false,
            permissions.effective_policy,
            &registry.context.mcp_tools,
            false,
        );
        let mut request_tool_budget = ToolBudget::Hard(&mut tool_budget);
        let turn = stream_single_claude_response(
            &registry.context.app,
            &registry.context.client,
            &registry.context.sidecar,
            CapacityCaptureContext::anthropic(
                registry.context.app.state::<Storage>().inner(),
                registry.context.credential_profile_id.as_deref(),
                &registry.context.model,
            ),
            &api_key,
            &registry.context.headroom_project,
            &body,
            &registry.context.conversation_id,
            &mut budget,
            &mut request_tool_budget,
            false,
            cancel.clone(),
        )
        .await?;
        assistant_text.push_str(&turn.assistant_text);
        merge_token_usage(&mut usage, turn.usage)?;
        messages.push(json!({ "role": "assistant", "content": turn.content }));
        let mut results = Vec::new();
        for call in turn.tool_calls {
            let tool_use_id = call.call_id.clone();
            let _tool_permit = registry
                .tool_permit(permissions.effective_policy, &call)
                .await?;
            let item = execute_tool_call(
                &registry.context.app,
                &registry.context.app.state::<ChatManager>(),
                &registry.context.app.state::<Storage>(),
                &registry.context.conversation_id,
                &registry.context.turn_id,
                &registry.context.project_root,
                permissions.effective_policy,
                &registry.context.write_slug_basis,
                &registry.context.extensions,
                &registry.context.mcp_tools,
                call,
                cancel.clone(),
                None,
            )
            .await?;
            results.push(json!({
                "type": "tool_result",
                "tool_use_id": tool_use_id,
                "content": tool_output_to_claude_content(&item)
            }));
        }
        let queued = registry.drain_mailbox(&agent_id);
        results.extend(
            queued
                .into_iter()
                .map(|message| json!({ "type": "text", "text": message })),
        );
        if !results.is_empty() {
            messages.push(json!({ "role": "user", "content": results }));
            continue;
        }
        if turn.stop_reason.as_deref() == Some("pause_turn") {
            continue;
        }
        return Ok(StreamSuccess {
            assistant_text,
            usage,
            service_tier: None,
            fast_mode_fallback: false,
        });
    }
}

fn unsupported_tool_output(
    app: &impl ChatDeltaEmitter,
    conversation_id: &str,
    call: ToolCallEvent,
) -> ChatResult<Value> {
    let output = format!("Unsupported tool `{}`; tool call was not run.", call.name);
    emit_tool_result(
        app,
        conversation_id,
        &call.call_id,
        "failed",
        None,
        None,
        false,
    )
    .map_err(ChatError::new)?;
    Ok(tool_call_output_item(&call, &output))
}

#[allow(clippy::too_many_arguments)]
async fn execute_browser_tool_call(
    app: &AppHandle,
    chat: &ChatManager,
    storage: &Storage,
    conversation_id: &str,
    project_root: &str,
    effective_policy: exec::ApprovalPolicy,
    call: ToolCallEvent,
    cancel: Arc<AtomicBool>,
) -> ChatResult<Value> {
    let browser = app.state::<BrowserManager>();
    let display_command = format!("{} {}", call.name, call.input);
    if matches!(
        effective_policy,
        exec::ApprovalPolicy::ReadOnly | exec::ApprovalPolicy::Never
    ) {
        emit_named_tool_call(
            app,
            conversation_id,
            &call.call_id,
            &display_command,
            "browser",
            None,
            "denied",
            Some("Browser control is unavailable under the current permission mode."),
        )
        .map_err(ChatError::new)?;
        emit_tool_result(
            app,
            conversation_id,
            &call.call_id,
            "denied",
            None,
            None,
            false,
        )
        .map_err(ChatError::new)?;
        return Ok(tool_call_output_item(
            &call,
            "Browser control was denied by the current permission mode.",
        ));
    }

    let permission = browser
        .permission_request(&call.name, &call.input)
        .await
        .map_err(ChatError::new)?;
    if permission.blocked {
        let hostname = permission.hostname.as_deref().unwrap_or("this site");
        let message = format!("Browser access to {hostname} is blocked in Browser settings.");
        emit_named_tool_call(
            app,
            conversation_id,
            &call.call_id,
            &display_command,
            "browser",
            None,
            "denied",
            Some(&message),
        )
        .map_err(ChatError::new)?;
        emit_tool_result(
            app,
            conversation_id,
            &call.call_id,
            "denied",
            None,
            None,
            false,
        )
        .map_err(ChatError::new)?;
        return Ok(tool_call_output_item(&call, &message));
    }
    let session_approved = browser
        .session_is_approved(conversation_id, permission.hostname.as_deref())
        .await;
    let needs_approval = permission.sensitive || (!permission.site_allowed && !session_approved);
    if !matches!(effective_policy, exec::ApprovalPolicy::SkipAll) && needs_approval {
        let decision_receiver = chat
            .register_tool_decision(&call.call_id)
            .map_err(ChatError::new)?;
        let approval_reason = if permission.sensitive {
            format!(
                "Halv wants to perform a potentially consequential browser action on {}. Review the exact action before allowing it.",
                permission.hostname.as_deref().unwrap_or("the active page")
            )
        } else {
            format!(
                "Halv wants to access {} in its dedicated browser profile. Page content is untrusted and actions can affect external services.",
                permission.hostname.as_deref().unwrap_or("the active page")
            )
        };
        if let Err(message) = emit_named_tool_call(
            app,
            conversation_id,
            &call.call_id,
            &display_command,
            "browser",
            None,
            "awaiting_approval",
            Some(&approval_reason),
        ) {
            chat.clear_tool_decision(&call.call_id);
            return Err(ChatError::new(message));
        }
        let decision =
            match wait_for_tool_decision(chat, &call.call_id, decision_receiver, cancel.clone())
                .await
            {
                Ok(decision) => decision,
                Err(error) => {
                    emit_tool_result(
                        app,
                        conversation_id,
                        &call.call_id,
                        "cancelled",
                        None,
                        None,
                        false,
                    )
                    .map_err(ChatError::new)?;
                    return Err(error);
                }
            };
        match decision {
            ReviewDecision::Denied | ReviewDecision::McpElicitation { .. } => {
                emit_tool_result(
                    app,
                    conversation_id,
                    &call.call_id,
                    "denied",
                    None,
                    None,
                    false,
                )
                .map_err(ChatError::new)?;
                return Ok(tool_call_output_item(&call, "User denied browser control."));
            }
            ReviewDecision::ApprovedForSession => {
                browser
                    .approve_session(conversation_id, permission.hostname.as_deref())
                    .await;
            }
            ReviewDecision::Approved | ReviewDecision::ApprovedPaths { .. } => {}
        }
    }

    if !live_tool_permissions_match(
        storage,
        app.state::<PreferencesManager>().inner(),
        conversation_id,
        project_root,
        effective_policy,
        ToolInvocation::Model,
    )? {
        return stale_tool_permissions_output(app, conversation_id, &call);
    }
    emit_named_tool_call(
        app,
        conversation_id,
        &call.call_id,
        &display_command,
        "browser",
        None,
        "running",
        None,
    )
    .map_err(ChatError::new)?;

    let execution = browser.execute(&call.name, &call.input);
    tokio::pin!(execution);
    let result = tokio::select! {
        result = &mut execution => result,
        _ = wait_for_cancel(cancel) => return Err(ChatError::new("Chat request cancelled")),
    };
    match result {
        Ok(output) => {
            let tail = if output.screenshot_png.is_some() {
                format!("{}\n\n[PNG screenshot attached]", output.text)
            } else {
                output.text.clone()
            };
            emit_delta(
                app,
                ChatDeltaPayload {
                    call_id: Some(call.call_id.clone()),
                    tail: Some(tail.chars().take(8_000).collect()),
                    ..ChatDeltaPayload::new(conversation_id, ChatDeltaKind::ToolOutput)
                },
            )
            .map_err(ChatError::new)?;
            emit_tool_result(
                app,
                conversation_id,
                &call.call_id,
                "completed",
                None,
                None,
                false,
            )
            .map_err(ChatError::new)?;
            Ok(browser_tool_call_output_item(&call, &output))
        }
        Err(error) => {
            emit_tool_result(
                app,
                conversation_id,
                &call.call_id,
                "failed",
                None,
                None,
                false,
            )
            .map_err(ChatError::new)?;
            Ok(tool_call_output_item(
                &call,
                &format!("Browser tool failed: {error}"),
            ))
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn execute_mcp_tool_call(
    app: &AppHandle,
    chat: &ChatManager,
    conversation_id: &str,
    project_root: &str,
    extensions: &ExtensionManager,
    tool: &extensions::McpToolDescriptor,
    call: ToolCallEvent,
    cancel: Arc<AtomicBool>,
) -> ChatResult<Value> {
    let arguments = match serde_json::from_str::<Value>(&call.input) {
        Ok(Value::Object(arguments)) => Value::Object(arguments),
        Ok(_) => {
            return Ok(tool_call_output_item(
                &call,
                "MCP tool arguments must be a JSON object.",
            ));
        }
        Err(error) => {
            return Ok(tool_call_output_item(
                &call,
                &format!("Invalid MCP tool arguments: {error}"),
            ));
        }
    };
    let display_command = format!("{}/{} {}", tool.server_name, tool.name, call.input);
    let requires_approval = tool.approval_mode != "auto" && tool.approval_mode != "approve";
    if requires_approval {
        let decision_receiver = chat
            .register_tool_decision(&call.call_id)
            .map_err(ChatError::new)?;
        if let Err(message) = emit_named_tool_call(
            app,
            conversation_id,
            &call.call_id,
            &display_command,
            "mcp",
            None,
            "awaiting_approval",
            Some("An MCP server requested this external tool call."),
        ) {
            chat.clear_tool_decision(&call.call_id);
            return Err(ChatError::new(message));
        }
        let decision =
            match wait_for_tool_decision(chat, &call.call_id, decision_receiver, cancel.clone())
                .await
            {
                Ok(decision) => decision,
                Err(error) => {
                    emit_tool_result(
                        app,
                        conversation_id,
                        &call.call_id,
                        "cancelled",
                        None,
                        None,
                        false,
                    )
                    .map_err(ChatError::new)?;
                    return Err(error);
                }
            };
        if matches!(
            decision,
            ReviewDecision::Denied | ReviewDecision::McpElicitation { .. }
        ) {
            emit_tool_result(
                app,
                conversation_id,
                &call.call_id,
                "denied",
                None,
                None,
                false,
            )
            .map_err(ChatError::new)?;
            return Ok(tool_call_output_item(
                &call,
                "User denied the MCP tool call.",
            ));
        }
    }
    emit_named_tool_call(
        app,
        conversation_id,
        &call.call_id,
        &display_command,
        "mcp",
        None,
        "running",
        None,
    )
    .map_err(ChatError::new)?;
    let (elicitation_sender, mut elicitation_receiver) = tokio::sync::mpsc::channel(4);
    let tool_request = extensions::call_mcp_tool(
        extensions,
        tool,
        arguments,
        Some(elicitation_sender),
        Some(Path::new(project_root)),
    );
    tokio::pin!(tool_request);
    let mut elicitation_index = 0_usize;
    let result = loop {
        tokio::select! {
            result = &mut tool_request => break result,
            request = elicitation_receiver.recv() => {
                let Some(request) = request else {
                    continue;
                };
                elicitation_index = elicitation_index.saturating_add(1);
                handle_mcp_elicitation(
                    app,
                    chat,
                    conversation_id,
                    tool,
                    &call.call_id,
                    elicitation_index,
                    request,
                    cancel.clone(),
                ).await?;
            }
            _ = wait_for_cancel(cancel.clone()) => {
                return Err(ChatError::new("Chat request cancelled"));
            },
        }
    };
    match result {
        Ok(output) => {
            if !output.is_error {
                chat.record_crux_savings(conversation_id, &call.call_id, tool);
            }
            let display_output = if output.text.trim().is_empty() {
                format!("MCP returned {} image(s).", output.images.len())
            } else if output.images.is_empty() {
                output.text.clone()
            } else {
                format!(
                    "{}\n\n[{} image(s) attached]",
                    output.text,
                    output.images.len()
                )
            };
            let tail = display_output.chars().take(8_000).collect::<String>();
            emit_delta(
                app,
                ChatDeltaPayload {
                    call_id: Some(call.call_id.clone()),
                    tail: Some(tail),
                    ..ChatDeltaPayload::new(conversation_id, ChatDeltaKind::ToolOutput)
                },
            )
            .map_err(ChatError::new)?;
            emit_tool_result(
                app,
                conversation_id,
                &call.call_id,
                if output.is_error {
                    "failed"
                } else {
                    "completed"
                },
                None,
                None,
                false,
            )
            .map_err(ChatError::new)?;
            Ok(mcp_tool_call_output_item(&call, &output))
        }
        Err(error) => {
            emit_tool_result(
                app,
                conversation_id,
                &call.call_id,
                "failed",
                None,
                None,
                false,
            )
            .map_err(ChatError::new)?;
            Ok(tool_call_output_item(
                &call,
                &format!("MCP tool execution failed: {error}"),
            ))
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn handle_mcp_elicitation(
    app: &AppHandle,
    chat: &ChatManager,
    conversation_id: &str,
    tool: &extensions::McpToolDescriptor,
    parent_call_id: &str,
    index: usize,
    request: extensions::McpElicitationRequest,
    cancel: Arc<AtomicBool>,
) -> ChatResult<()> {
    let call_id = format!("{parent_call_id}:elicitation:{index}");
    let decision_receiver = chat
        .register_tool_decision(&call_id)
        .map_err(ChatError::new)?;
    let command = format!("{}: {}", tool.server_name, request.message);
    let warning = request
        .url
        .as_ref()
        .map(|url| format!("The server wants to open this external URL after approval: {url}"));
    if let Err(error) = emit_delta(
        app,
        ChatDeltaPayload {
            call_id: Some(call_id.clone()),
            command: Some(command),
            tool_kind: Some("mcp_elicitation".to_string()),
            status: Some("awaiting_approval".to_string()),
            justification: Some(format!(
                "The MCP server `{}` is requesting information before it can continue.",
                tool.server_name
            )),
            warning,
            elicitation_mode: Some(request.mode.clone()),
            elicitation_schema: request.requested_schema.clone(),
            elicitation_url: request.url.clone(),
            ..ChatDeltaPayload::new(conversation_id, ChatDeltaKind::ToolCall)
        },
    ) {
        chat.clear_tool_decision(&call_id);
        return Err(ChatError::new(error));
    }
    let decision = wait_for_tool_decision(chat, &call_id, decision_receiver, cancel).await?;
    let response = match decision {
        ReviewDecision::McpElicitation { action, content } => {
            extensions::McpElicitationResponse { action, content }
        }
        ReviewDecision::Denied => extensions::McpElicitationResponse {
            action: "decline".to_string(),
            content: None,
        },
        ReviewDecision::Approved
        | ReviewDecision::ApprovedForSession
        | ReviewDecision::ApprovedPaths { .. } => extensions::McpElicitationResponse {
            action: "accept".to_string(),
            content: request.requested_schema.as_ref().map(|_| json!({})),
        },
    };
    let status = match response.action.as_str() {
        "accept" => "completed",
        "decline" => "denied",
        _ => "cancelled",
    };
    emit_tool_result(app, conversation_id, &call_id, status, None, None, false)
        .map_err(ChatError::new)?;
    request
        .responder
        .send(response)
        .map_err(|_| ChatError::new("MCP server stopped waiting for user input"))
}

fn stale_tool_permissions_output(
    app: &impl ChatDeltaEmitter,
    conversation_id: &str,
    call: &ToolCallEvent,
) -> ChatResult<Value> {
    let output =
        "Tool permissions changed while this call was pending; tool call was not run. Retry under the current settings.";
    emit_tool_result(
        app,
        conversation_id,
        &call.call_id,
        "denied",
        None,
        None,
        false,
    )
    .map_err(ChatError::new)?;
    Ok(tool_call_output_item(call, output))
}

#[allow(clippy::too_many_arguments, clippy::collapsible_if)]
async fn execute_shell_command_call<E>(
    app: &E,
    chat: &ChatManager,
    storage: &Storage,
    preferences: &PreferencesManager,
    conversation_id: &str,
    turn_id: &str,
    default_cwd: &str,
    effective_policy: exec::ApprovalPolicy,
    write_slug_basis: &str,
    call: ToolCallEvent,
    invocation: ToolInvocation,
    cancel: Arc<AtomicBool>,
) -> ChatResult<Value>
where
    E: ChatDeltaEmitter + Clone + Send + Sync + 'static,
{
    if call.name != "shell_command" {
        let output = format!(
            "Unsupported function tool `{}`; tool call was not run.",
            call.name
        );
        emit_tool_result(
            app,
            conversation_id,
            &call.call_id,
            "failed",
            None,
            None,
            false,
        )
        .map_err(ChatError::new)?;
        return Ok(tool_call_output_item(&call, &output));
    }

    let mut request = match serde_json::from_str::<ShellCommandRequest>(&call.input) {
        Ok(request) => request,
        Err(error) => {
            let output = format!("Invalid shell_command arguments: {error}");
            emit_tool_result(
                app,
                conversation_id,
                &call.call_id,
                "failed",
                None,
                None,
                false,
            )
            .map_err(ChatError::new)?;
            return Ok(tool_call_output_item(&call, &output));
        }
    };

    if request.command.trim().is_empty() {
        let output = "Invalid shell_command arguments: command cannot be empty";
        emit_tool_result(
            app,
            conversation_id,
            &call.call_id,
            "failed",
            None,
            None,
            false,
        )
        .map_err(ChatError::new)?;
        return Ok(tool_call_output_item(&call, output));
    }

    if let Err(error) = exec::validate_shell_command_request(&request) {
        let output = format!("Invalid shell_command arguments: {error}");
        emit_tool_result(
            app,
            conversation_id,
            &call.call_id,
            "failed",
            None,
            None,
            false,
        )
        .map_err(ChatError::new)?;
        return Ok(tool_call_output_item(&call, &output));
    }

    if let Some(patch) = exec::extract_apply_patch_from_shell_command(&request.command) {
        return match patch {
            Ok(patch) => {
                let patch_call = ToolCallEvent {
                    name: "apply_patch".to_string(),
                    input: patch,
                    kind: ToolCallKind::Function,
                    ..call
                };
                execute_apply_patch_call(
                    app,
                    chat,
                    storage,
                    preferences,
                    conversation_id,
                    turn_id,
                    default_cwd,
                    effective_policy,
                    write_slug_basis,
                    patch_call,
                    invocation,
                    cancel,
                    true,
                )
                .await
            }
            Err(error) => Ok(tool_call_output_item(
                &call,
                &format!("Invalid apply_patch shell command: {error}"),
            )),
        };
    }

    let cwd = match exec::resolve_shell_cwd(&request, default_cwd) {
        Ok(cwd) => cwd,
        Err(error) => {
            let output = format!("Invalid shell_command workdir: {error}");
            emit_tool_result(
                app,
                conversation_id,
                &call.call_id,
                "failed",
                None,
                None,
                false,
            )
            .map_err(ChatError::new)?;
            return Ok(tool_call_output_item(&call, &output));
        }
    };
    request.workdir = Some(cwd.clone());
    let approval_key = exec::approval_key_for(&request, conversation_id, default_cwd, &cwd);
    let mut command_was_approved = chat.approval_cache.contains(&approval_key);
    let command_needs_approval = exec::needs_approval(&request);
    let needs_approval = shell_needs_confirmation(effective_policy, &request);
    if matches!(effective_policy, exec::ApprovalPolicy::Never) && command_needs_approval {
        let output = exec::denied_tool_output(&request.command);
        emit_tool_result(
            app,
            conversation_id,
            &call.call_id,
            "denied",
            None,
            None,
            false,
        )
        .map_err(ChatError::new)?;
        return Ok(tool_call_output_item(&call, &output));
    }

    if needs_approval && !command_was_approved {
        let decision_receiver = chat
            .register_tool_decision(&call.call_id)
            .map_err(ChatError::new)?;
        chat.register_denied_action(
            &call.call_id,
            DeniedToolAction::Shell {
                conversation_id: conversation_id.to_string(),
                command: request.command.clone(),
                cwd: cwd.clone(),
                approval_key: approval_key.clone(),
            },
        );
        if let Err(message) = emit_tool_call(
            app,
            conversation_id,
            &call.call_id,
            &request.command,
            &cwd,
            "awaiting_approval",
            request.justification.as_deref(),
        ) {
            chat.clear_tool_decision(&call.call_id);
            return Err(ChatError::new(message));
        }

        let decision =
            match wait_for_tool_decision(chat, &call.call_id, decision_receiver, cancel.clone())
                .await
            {
                Ok(decision) => decision,
                Err(error) => {
                    emit_tool_result(
                        app,
                        conversation_id,
                        &call.call_id,
                        "cancelled",
                        None,
                        None,
                        false,
                    )
                    .map_err(ChatError::new)?;
                    return Err(error);
                }
            };

        match decision {
            ReviewDecision::Denied | ReviewDecision::McpElicitation { .. } => {
                let output = exec::denied_tool_output(&request.command);
                emit_tool_result(
                    app,
                    conversation_id,
                    &call.call_id,
                    "denied",
                    None,
                    None,
                    false,
                )
                .map_err(ChatError::new)?;
                return Ok(tool_call_output_item(&call, &output));
            }
            ReviewDecision::ApprovedForSession => {
                chat.approval_cache
                    .approve_for_session(approval_key.clone());
                command_was_approved = true;
            }
            ReviewDecision::ApprovedPaths { .. } | ReviewDecision::Approved => {
                command_was_approved = true;
            }
        }
    }

    let sandbox_mode = shell_sandbox_mode(
        effective_policy,
        command_was_approved,
        command_needs_approval,
    );
    let initial_write_capable = matches!(
        sandbox_mode,
        SandboxMode::WorkspaceWrite | SandboxMode::DangerFullAccess
    );
    let turn_may_write =
        initial_write_capable || matches!(effective_policy, exec::ApprovalPolicy::OnFailure);
    let workspace_roots = WorkspaceRoots::for_cwd(default_cwd);
    if initial_write_capable {
        if !ensure_first_write_guard(
            app,
            chat,
            conversation_id,
            &call.call_id,
            default_cwd,
            "shell command",
            write_slug_basis,
            None,
            skips_first_write_confirmation(effective_policy),
            cancel.clone(),
        )
        .await?
        {
            let output = exec::denied_tool_output(&request.command);
            emit_tool_result(
                app,
                conversation_id,
                &call.call_id,
                "denied",
                None,
                None,
                false,
            )
            .map_err(ChatError::new)?;
            return Ok(tool_call_output_item(&call, &output));
        }
    }

    if !live_tool_permissions_match(
        storage,
        preferences,
        conversation_id,
        default_cwd,
        effective_policy,
        invocation,
    )? {
        return stale_tool_permissions_output(app, conversation_id, &call);
    }

    emit_tool_call(
        app,
        conversation_id,
        &call.call_id,
        &request.command,
        &cwd,
        "running",
        request.justification.as_deref(),
    )
    .map_err(ChatError::new)?;

    let shell_snapshots = if turn_may_write {
        // Run the git preimage snapshot on a blocking thread.
        let snapshot_cwd = default_cwd.to_string();
        Some(
            tokio::task::spawn_blocking(move || snapshot_git_shell_preimages(&snapshot_cwd))
                .await
                .unwrap_or_else(|error| Err(format!("git snapshot task failed: {error}"))),
        )
    } else {
        None
    };

    if !live_tool_permissions_match(
        storage,
        preferences,
        conversation_id,
        default_cwd,
        effective_policy,
        invocation,
    )? {
        return stale_tool_permissions_output(app, conversation_id, &call);
    }

    let app_for_output = app.clone();
    let conversation_for_output = conversation_id.to_string();
    let call_for_output = call.call_id.clone();
    let output_callback = OutputCallback::new(move |chunk| {
        let tail = String::from_utf8_lossy(chunk).to_string();
        let _ = emit_delta(
            &app_for_output,
            ChatDeltaPayload {
                call_id: Some(call_for_output.clone()),
                tail: Some(tail),
                ..ChatDeltaPayload::new(&conversation_for_output, ChatDeltaKind::ToolOutput)
            },
        );
    });

    let execution = run_shell_command_with_on_failure_retry(
        app,
        chat,
        storage,
        preferences,
        conversation_id,
        turn_id,
        default_cwd,
        effective_policy,
        write_slug_basis,
        invocation,
        &call,
        request,
        sandbox_mode,
        workspace_roots,
        approval_key,
        command_was_approved,
        cancel,
        output_callback,
    )
    .await;

    let finalization_result = if turn_may_write {
        finalize_shell_turn_diff(
            app,
            storage,
            conversation_id,
            turn_id,
            default_cwd,
            shell_snapshots.expect("write-capable shell call should capture preimages"),
        )
        .await
    } else {
        Ok(())
    };
    if let Err(error) = &finalization_result {
        chat.record_finalization_error(conversation_id, &error.message);
    }
    let execution = match execution {
        Ok(execution) => {
            finalization_result?;
            execution
        }
        Err(error) => {
            finalization_result?;
            return Err(error);
        }
    };

    match execution {
        ShellExecutionOutcome::Completed(result) => {
            emit_shell_tool_result(
                app,
                conversation_id,
                &call.call_id,
                "completed",
                Some(result.exit_code),
                Some(duration_millis(result.wall_time)),
                result.truncated_output,
                result.rtk_route,
            )
            .map_err(ChatError::new)?;
            Ok(tool_call_output_item(&call, &result.model_output))
        }
        ShellExecutionOutcome::Cancelled => {
            emit_tool_result(
                app,
                conversation_id,
                &call.call_id,
                "cancelled",
                None,
                None,
                false,
            )
            .map_err(ChatError::new)?;
            Err(ChatError::new("Chat request cancelled"))
        }
        ShellExecutionOutcome::Failed(error) => {
            let output = format!("Tool execution failed: {error}");
            emit_tool_result(
                app,
                conversation_id,
                &call.call_id,
                "failed",
                None,
                None,
                false,
            )
            .map_err(ChatError::new)?;
            Ok(tool_call_output_item(&call, &output))
        }
        ShellExecutionOutcome::StalePermissions => {
            stale_tool_permissions_output(app, conversation_id, &call)
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OnFailureRetryAction {
    ReturnSandboxedResult,
    RequestApproval,
    RetryUnsandboxed,
}

fn on_failure_retry_action(
    policy: exec::ApprovalPolicy,
    sandbox_mode: SandboxMode,
    exit_code: Option<i32>,
    timed_out: bool,
    approval_cached: bool,
) -> OnFailureRetryAction {
    if !matches!(policy, exec::ApprovalPolicy::OnFailure)
        || matches!(sandbox_mode, SandboxMode::DangerFullAccess)
        || timed_out
        || !exit_code.is_some_and(|exit_code| exit_code != 0)
    {
        return OnFailureRetryAction::ReturnSandboxedResult;
    }
    if approval_cached {
        OnFailureRetryAction::RetryUnsandboxed
    } else {
        OnFailureRetryAction::RequestApproval
    }
}

enum ShellExecutionOutcome {
    Completed(exec::ExecResult),
    Cancelled,
    Failed(exec::ExecError),
    StalePermissions,
}

#[allow(clippy::too_many_arguments)]
async fn run_shell_command_with_on_failure_retry<E>(
    app: &E,
    chat: &ChatManager,
    storage: &Storage,
    preferences: &PreferencesManager,
    conversation_id: &str,
    turn_id: &str,
    default_cwd: &str,
    effective_policy: exec::ApprovalPolicy,
    write_slug_basis: &str,
    invocation: ToolInvocation,
    call: &ToolCallEvent,
    request: ShellCommandRequest,
    sandbox_mode: SandboxMode,
    workspace_roots: WorkspaceRoots,
    approval_key: exec::ApprovalKey,
    approval_cached: bool,
    cancel: Arc<AtomicBool>,
    output_callback: OutputCallback,
) -> ChatResult<ShellExecutionOutcome>
where
    E: ChatDeltaEmitter + Clone + Send + Sync + 'static,
{
    let rtk_database_path = match invocation {
        ToolInvocation::Model => exec::rtk_gui_turn_database_path(turn_id),
        // Direct shell turns have no measured chat settle. Keep their existing
        // database so the direct-shell savings path remains unchanged.
        ToolInvocation::DirectUser => exec::rtk_database_path(),
    };
    let sandboxed_result = match exec::run_shell_command_sandboxed_with_rtk_database(
        request.clone(),
        default_cwd.to_string(),
        sandbox_mode,
        Some(workspace_roots.clone()),
        rtk_database_path.clone(),
        cancel.clone(),
        output_callback.clone(),
    )
    .await
    {
        Ok(result) => result,
        Err(exec::ExecError::Cancelled) => return Ok(ShellExecutionOutcome::Cancelled),
        Err(error) => return Ok(ShellExecutionOutcome::Failed(error)),
    };
    if cancel.load(Ordering::SeqCst) {
        return Ok(ShellExecutionOutcome::Cancelled);
    }

    match on_failure_retry_action(
        effective_policy,
        sandbox_mode,
        Some(sandboxed_result.exit_code),
        sandboxed_result.timed_out,
        approval_cached,
    ) {
        OnFailureRetryAction::ReturnSandboxedResult => {
            return Ok(ShellExecutionOutcome::Completed(sandboxed_result));
        }
        OnFailureRetryAction::RequestApproval => {
            let decision_receiver = chat
                .register_tool_decision(&call.call_id)
                .map_err(ChatError::new)?;
            let retry_justification = request
                .justification
                .as_deref()
                .map(str::trim)
                .filter(|justification| !justification.is_empty())
                .map(|justification| {
                    format!(
                        "The sandboxed command failed. Approve an unsandboxed retry of this exact command. Model justification: {justification}"
                    )
                })
                .unwrap_or_else(|| {
                    "The sandboxed command failed. Approve an unsandboxed retry of this exact command."
                        .to_string()
                });
            if let Err(message) = emit_tool_call(
                app,
                conversation_id,
                &call.call_id,
                &request.command,
                request.workdir.as_deref().unwrap_or(default_cwd),
                "awaiting_approval",
                Some(&retry_justification),
            ) {
                chat.clear_tool_decision(&call.call_id);
                return Err(ChatError::new(message));
            }
            let decision = match wait_for_tool_decision(
                chat,
                &call.call_id,
                decision_receiver,
                cancel.clone(),
            )
            .await
            {
                Ok(decision) => decision,
                Err(_) => return Ok(ShellExecutionOutcome::Cancelled),
            };
            match decision {
                ReviewDecision::Denied | ReviewDecision::McpElicitation { .. } => {
                    return Ok(ShellExecutionOutcome::Completed(sandboxed_result));
                }
                ReviewDecision::ApprovedForSession => {
                    chat.approval_cache.approve_for_session(approval_key);
                }
                ReviewDecision::ApprovedPaths { .. } | ReviewDecision::Approved => {}
            }
        }
        OnFailureRetryAction::RetryUnsandboxed => {}
    }

    if cancel.load(Ordering::SeqCst) {
        return Ok(ShellExecutionOutcome::Cancelled);
    }
    if !live_tool_permissions_match(
        storage,
        preferences,
        conversation_id,
        default_cwd,
        effective_policy,
        invocation,
    )? {
        return Ok(ShellExecutionOutcome::StalePermissions);
    }
    if !ensure_first_write_guard(
        app,
        chat,
        conversation_id,
        &call.call_id,
        default_cwd,
        "unsandboxed shell retry",
        write_slug_basis,
        None,
        true,
        cancel.clone(),
    )
    .await?
    {
        return Ok(ShellExecutionOutcome::Completed(sandboxed_result));
    }
    if cancel.load(Ordering::SeqCst) {
        return Ok(ShellExecutionOutcome::Cancelled);
    }
    if !live_tool_permissions_match(
        storage,
        preferences,
        conversation_id,
        default_cwd,
        effective_policy,
        invocation,
    )? {
        return Ok(ShellExecutionOutcome::StalePermissions);
    }
    emit_tool_call(
        app,
        conversation_id,
        &call.call_id,
        &request.command,
        request.workdir.as_deref().unwrap_or(default_cwd),
        "running",
        request.justification.as_deref(),
    )
    .map_err(ChatError::new)?;

    match exec::run_shell_command_sandboxed_with_rtk_database(
        request,
        default_cwd.to_string(),
        SandboxMode::DangerFullAccess,
        Some(workspace_roots),
        rtk_database_path,
        cancel,
        output_callback,
    )
    .await
    {
        Ok(result) => Ok(ShellExecutionOutcome::Completed(result)),
        Err(exec::ExecError::Cancelled) => Ok(ShellExecutionOutcome::Cancelled),
        Err(error) => Ok(ShellExecutionOutcome::Failed(error)),
    }
}

fn shell_sandbox_mode(
    effective_policy: exec::ApprovalPolicy,
    command_was_approved: bool,
    command_needs_approval: bool,
) -> SandboxMode {
    match effective_policy {
        exec::ApprovalPolicy::SkipAll => SandboxMode::DangerFullAccess,
        exec::ApprovalPolicy::OnRequest | exec::ApprovalPolicy::OnFailure
            if !command_needs_approval =>
        {
            SandboxMode::ReadOnly
        }
        exec::ApprovalPolicy::OnRequest | exec::ApprovalPolicy::OnFailure => {
            SandboxMode::WorkspaceWrite
        }
        exec::ApprovalPolicy::Ask if command_was_approved && command_needs_approval => {
            SandboxMode::WorkspaceWrite
        }
        exec::ApprovalPolicy::ReadOnly
        | exec::ApprovalPolicy::Ask
        | exec::ApprovalPolicy::Never => SandboxMode::ReadOnly,
    }
}

fn shell_needs_confirmation(policy: exec::ApprovalPolicy, request: &ShellCommandRequest) -> bool {
    match policy {
        exec::ApprovalPolicy::ReadOnly | exec::ApprovalPolicy::Ask => exec::needs_approval(request),
        exec::ApprovalPolicy::OnRequest => exec::requests_additional_execution_authority(request),
        exec::ApprovalPolicy::OnFailure
        | exec::ApprovalPolicy::Never
        | exec::ApprovalPolicy::SkipAll => false,
    }
}

fn skips_first_write_confirmation(policy: exec::ApprovalPolicy) -> bool {
    matches!(
        policy,
        exec::ApprovalPolicy::Ask
            | exec::ApprovalPolicy::OnRequest
            | exec::ApprovalPolicy::OnFailure
            | exec::ApprovalPolicy::SkipAll
    )
}

fn patch_needs_confirmation(
    policy: exec::ApprovalPolicy,
    has_uncached_outside_paths: bool,
    ask_session_approved: bool,
) -> bool {
    has_uncached_outside_paths
        || (matches!(policy, exec::ApprovalPolicy::Ask) && !ask_session_approved)
}

#[allow(clippy::too_many_arguments)]
async fn execute_apply_patch_call(
    app: &impl ChatDeltaEmitter,
    chat: &ChatManager,
    storage: &Storage,
    preferences: &PreferencesManager,
    conversation_id: &str,
    turn_id: &str,
    default_cwd: &str,
    effective_policy: exec::ApprovalPolicy,
    write_slug_basis: &str,
    call: ToolCallEvent,
    invocation: ToolInvocation,
    cancel: Arc<AtomicBool>,
    intercepted_shell_call: bool,
) -> ChatResult<Value> {
    let parsed = match exec::parse_apply_patch(&call.input) {
        Ok(parsed) => parsed,
        Err(error) => {
            emit_tool_result(
                app,
                conversation_id,
                &call.call_id,
                "failed",
                None,
                None,
                false,
            )
            .map_err(ChatError::new)?;
            return Ok(tool_call_output_item(
                &call,
                &format!("Invalid apply_patch input: {error}"),
            ));
        }
    };

    let cwd = PathBuf::from(default_cwd);
    let roots = WorkspaceRoots::for_cwd(&cwd);
    let mut changes = exec::apply_patch_file_changes(&parsed);
    let safety = exec::assess_patch_safety(&parsed, &cwd, &roots, effective_policy);
    let requires_approval = match safety {
        exec::PatchSafety::AutoApproved => Vec::new(),
        exec::PatchSafety::Denied(reason) => {
            let output = exec::denied_patch_output(&reason);
            emit_tool_call_with_files(
                app,
                conversation_id,
                &call.call_id,
                default_cwd,
                "denied",
                None,
                Some(&changes),
                None,
            )
            .map_err(ChatError::new)?;
            emit_tool_result(
                app,
                conversation_id,
                &call.call_id,
                "denied",
                None,
                None,
                false,
            )
            .map_err(ChatError::new)?;
            return Ok(tool_call_output_item(&call, &output));
        }
        exec::PatchSafety::RequiresApproval(paths) => paths,
    };

    let approval_keys = requires_approval
        .iter()
        .map(|path| {
            exec::patch_approval_key_for(conversation_id, &cwd, &roots, path)
                .map(|key| (path.clone(), key))
        })
        .collect::<Result<Vec<_>, _>>()
        .map_err(ChatError::new)?;
    let uncached = approval_keys
        .iter()
        .filter(|(_, key)| !chat.approval_cache.contains_patch_path(key))
        .cloned()
        .collect::<Vec<_>>();
    let ask_session_key = if matches!(effective_policy, exec::ApprovalPolicy::Ask) {
        Some(
            exec::patch_approval_key_for(conversation_id, &cwd, &roots, ".")
                .map_err(ChatError::new)?,
        )
    } else {
        None
    };
    let ask_session_approved = ask_session_key
        .as_ref()
        .is_some_and(|key| chat.approval_cache.contains_patch_path(key));
    for change in &mut changes {
        change.requires_approval = requires_approval.iter().any(|path| path == &change.path);
        change.approved = !change.requires_approval
            || approval_keys
                .iter()
                .find(|(path, _)| path == &change.path)
                .is_some_and(|(_, key)| chat.approval_cache.contains_patch_path(key));
    }

    if patch_needs_confirmation(effective_policy, !uncached.is_empty(), ask_session_approved) {
        let decision_receiver = chat
            .register_tool_decision(&call.call_id)
            .map_err(ChatError::new)?;
        let mut denied_approval_keys = approval_keys
            .iter()
            .map(|(_, key)| key.clone())
            .collect::<Vec<_>>();
        denied_approval_keys.extend(ask_session_key.iter().cloned());
        chat.register_denied_action(
            &call.call_id,
            DeniedToolAction::Patch {
                conversation_id: conversation_id.to_string(),
                patch: call.input.clone(),
                cwd: cwd.clone(),
                approval_keys: denied_approval_keys,
            },
        );
        let approval_reason = if uncached.is_empty() {
            "Ask mode requires confirmation before applying file changes."
        } else {
            "Patch touches paths outside the workspace-write roots."
        };
        emit_tool_call_with_files(
            app,
            conversation_id,
            &call.call_id,
            default_cwd,
            "awaiting_approval",
            Some(approval_reason),
            Some(&changes),
            None,
        )
        .map_err(|message| {
            chat.clear_tool_decision(&call.call_id);
            ChatError::new(message)
        })?;

        let decision =
            match wait_for_tool_decision(chat, &call.call_id, decision_receiver, cancel.clone())
                .await
            {
                Ok(decision) => decision,
                Err(error) => {
                    emit_tool_result(
                        app,
                        conversation_id,
                        &call.call_id,
                        "cancelled",
                        None,
                        None,
                        false,
                    )
                    .map_err(ChatError::new)?;
                    return Err(error);
                }
            };

        match decision {
            ReviewDecision::Denied | ReviewDecision::McpElicitation { .. } => {
                let output = exec::denied_patch_output("user denied patch approval");
                emit_tool_result(
                    app,
                    conversation_id,
                    &call.call_id,
                    "denied",
                    None,
                    None,
                    false,
                )
                .map_err(ChatError::new)?;
                return Ok(tool_call_output_item(&call, &output));
            }
            ReviewDecision::ApprovedForSession => {
                chat.approval_cache
                    .approve_patch_paths(uncached.iter().map(|(_, key)| key.clone()));
                if let Some(key) = &ask_session_key {
                    chat.approval_cache.approve_patch_paths([key.clone()]);
                }
            }
            ReviewDecision::ApprovedPaths { paths, remember } => {
                let approved = paths.into_iter().collect::<BTreeSet<_>>();
                let missing = uncached
                    .iter()
                    .map(|(path, _)| path)
                    .filter(|path| !approved.contains(*path))
                    .cloned()
                    .collect::<Vec<_>>();
                if !missing.is_empty() {
                    let output = exec::denied_patch_output(&format!(
                        "not all required paths were approved: {}",
                        missing.join(", ")
                    ));
                    emit_tool_result(
                        app,
                        conversation_id,
                        &call.call_id,
                        "denied",
                        None,
                        None,
                        false,
                    )
                    .map_err(ChatError::new)?;
                    return Ok(tool_call_output_item(&call, &output));
                }
                if remember {
                    chat.approval_cache.approve_patch_paths(
                        uncached
                            .iter()
                            .filter(|(path, _)| approved.contains(path))
                            .map(|(_, key)| key.clone()),
                    );
                    if let Some(key) = &ask_session_key {
                        chat.approval_cache.approve_patch_paths([key.clone()]);
                    }
                }
            }
            ReviewDecision::Approved => {}
        }
    }

    if !ensure_first_write_guard(
        app,
        chat,
        conversation_id,
        &call.call_id,
        default_cwd,
        "apply_patch",
        write_slug_basis,
        Some(&changes),
        skips_first_write_confirmation(effective_policy),
        cancel.clone(),
    )
    .await?
    {
        let output = exec::denied_patch_output("write safety confirmation was denied");
        emit_tool_result(
            app,
            conversation_id,
            &call.call_id,
            "denied",
            None,
            None,
            false,
        )
        .map_err(ChatError::new)?;
        return Ok(tool_call_output_item(&call, &output));
    }

    if !live_tool_permissions_match(
        storage,
        preferences,
        conversation_id,
        default_cwd,
        effective_policy,
        invocation,
    )? {
        return stale_tool_permissions_output(app, conversation_id, &call);
    }

    emit_tool_call_with_files(
        app,
        conversation_id,
        &call.call_id,
        default_cwd,
        "running",
        if intercepted_shell_call {
            Some("shell apply_patch invocation routed to the custom apply_patch handler")
        } else {
            None
        },
        Some(&changes),
        None,
    )
    .map_err(ChatError::new)?;

    let apply_roots = if matches!(effective_policy, exec::ApprovalPolicy::SkipAll) {
        roots
            .with_danger_full_access()
            .approved_patch_roots(&cwd, &exec::apply_patch_touched_paths(&parsed))
            .map_err(ChatError::new)?
    } else if requires_approval.is_empty() {
        roots
    } else {
        roots
            .approved_patch_roots(&cwd, &requires_approval)
            .map_err(ChatError::new)?
    };
    let snapshots = exec::snapshot_patch_paths(&cwd, &parsed).map_err(ChatError::new)?;
    let changed_paths = changes
        .iter()
        .map(|change| change.path.clone())
        .collect::<Vec<_>>();
    if !live_tool_permissions_match(
        storage,
        preferences,
        conversation_id,
        default_cwd,
        effective_policy,
        invocation,
    )? {
        return stale_tool_permissions_output(app, conversation_id, &call);
    }
    let apply_result =
        exec::apply_patch_sandboxed(cwd.clone(), call.input.clone(), apply_roots).await;
    if let Err(error) = finalize_patch_turn_diff(
        app,
        storage,
        conversation_id,
        turn_id,
        &cwd,
        snapshots,
        &changed_paths,
    ) {
        chat.record_finalization_error(conversation_id, &error.message);
        return Err(error);
    }
    let summary = apply_result.map_err(ChatError::new)?;
    emit_tool_result(
        app,
        conversation_id,
        &call.call_id,
        "completed",
        Some(0),
        None,
        false,
    )
    .map_err(ChatError::new)?;

    let output = serde_json::to_string(&summary).unwrap_or_else(|_| summary.message.clone());
    Ok(tool_call_output_item(&call, &output))
}

async fn wait_for_tool_decision(
    chat: &ChatManager,
    call_id: &str,
    receiver: oneshot::Receiver<ReviewDecision>,
    cancel: Arc<AtomicBool>,
) -> ChatResult<ReviewDecision> {
    let decision = tokio::select! {
        result = receiver => result.unwrap_or(ReviewDecision::Denied),
        _ = tokio::time::sleep(TOOL_APPROVAL_TIMEOUT) => {
            chat.clear_tool_decision(call_id);
            ReviewDecision::Denied
        }
        _ = wait_for_cancel(cancel) => {
            chat.clear_tool_decision(call_id);
            return Err(ChatError::new("Chat request cancelled"));
        }
    };
    Ok(decision)
}

#[allow(clippy::too_many_arguments)]
async fn ensure_first_write_guard(
    app: &impl ChatDeltaEmitter,
    chat: &ChatManager,
    conversation_id: &str,
    call_id: &str,
    default_cwd: &str,
    command: &str,
    write_slug_basis: &str,
    file_changes: Option<&[PatchFileChange]>,
    skip_prompts: bool,
    cancel: Arc<AtomicBool>,
) -> ChatResult<bool> {
    if chat
        .write_guards
        .lock()
        .expect("write guard lock poisoned")
        .get(conversation_id)
        .is_some_and(|state| state.confirmed)
    {
        return Ok(true);
    }

    let action = exec::prepare_first_write(Path::new(default_cwd), write_slug_basis);
    match action {
        FirstWriteAction::Proceed(status) => {
            chat.write_guards
                .lock()
                .expect("write guard lock poisoned")
                .insert(
                    conversation_id.to_string(),
                    WriteGuardState { confirmed: true },
                );
            emit_branch_status(app, conversation_id, status).map_err(ChatError::new)?;
            Ok(true)
        }
        FirstWriteAction::RequiresConfirmation(status) => {
            if skip_prompts {
                chat.write_guards
                    .lock()
                    .expect("write guard lock poisoned")
                    .insert(
                        conversation_id.to_string(),
                        WriteGuardState { confirmed: true },
                    );
                emit_branch_status(app, conversation_id, status).map_err(ChatError::new)?;
                return Ok(true);
            }
            let decision_receiver = chat
                .register_tool_decision(call_id)
                .map_err(ChatError::new)?;
            let emit_result = if let Some(file_changes) = file_changes {
                emit_tool_call_with_files(
                    app,
                    conversation_id,
                    call_id,
                    default_cwd,
                    "awaiting_approval",
                    status.warning.as_deref(),
                    Some(file_changes),
                    status.warning.as_deref(),
                )
            } else {
                emit_tool_call(
                    app,
                    conversation_id,
                    call_id,
                    command,
                    default_cwd,
                    "awaiting_approval",
                    status.warning.as_deref(),
                )
            };
            emit_result.map_err(|message| {
                chat.clear_tool_decision(call_id);
                ChatError::new(message)
            })?;
            let decision = wait_for_tool_decision(chat, call_id, decision_receiver, cancel).await?;
            match decision {
                ReviewDecision::Denied | ReviewDecision::McpElicitation { .. } => Ok(false),
                ReviewDecision::Approved
                | ReviewDecision::ApprovedForSession
                | ReviewDecision::ApprovedPaths { .. } => {
                    chat.write_guards
                        .lock()
                        .expect("write guard lock poisoned")
                        .insert(
                            conversation_id.to_string(),
                            WriteGuardState { confirmed: true },
                        );
                    emit_branch_status(app, conversation_id, status).map_err(ChatError::new)?;
                    Ok(true)
                }
            }
        }
    }
}

#[derive(Debug)]
pub(super) struct ShellTurnDiff {
    pub(super) snapshots: Vec<FileSnapshot>,
    pub(super) diff: String,
    pub(super) revertible: bool,
}

fn finalize_patch_turn_diff(
    emitter: &impl ChatDeltaEmitter,
    storage: &Storage,
    conversation_id: &str,
    turn_id: &str,
    cwd: &Path,
    snapshots: Vec<FileSnapshot>,
    changed_paths: &[String],
) -> ChatResult<()> {
    let mut snapshots = match changed_file_snapshots(snapshots) {
        Ok(snapshots) => snapshots,
        Err(error) => {
            let diff = bounded_nonrevertible_diff(changed_paths, &error);
            record_turn_diff(
                storage,
                conversation_id,
                turn_id,
                cwd,
                Vec::new(),
                diff.clone(),
                false,
            )
            .map_err(ChatError::new)?;
            return emit_turn_diff(emitter, conversation_id, turn_id, &diff, false)
                .map_err(ChatError::new);
        }
    };
    if snapshots.is_empty() {
        return Ok(());
    }
    let (snapshots, diff, revertible) = match exec::patch_diff(cwd, &mut snapshots) {
        Ok(diff) => (snapshots, diff, true),
        Err(error) => (
            Vec::new(),
            bounded_nonrevertible_diff(changed_paths, &error),
            false,
        ),
    };
    record_turn_diff(
        storage,
        conversation_id,
        turn_id,
        cwd,
        snapshots,
        diff.clone(),
        revertible,
    )
    .map_err(ChatError::new)?;
    emit_turn_diff(emitter, conversation_id, turn_id, &diff, revertible).map_err(ChatError::new)
}

async fn finalize_shell_turn_diff(
    emitter: &impl ChatDeltaEmitter,
    storage: &Storage,
    conversation_id: &str,
    turn_id: &str,
    default_cwd: &str,
    snapshots: Result<GitShellPreimages, String>,
) -> ChatResult<()> {
    // Run the git diff work on a blocking thread.
    let diff_cwd = default_cwd.to_string();
    let turn_diff = tokio::task::spawn_blocking(move || shell_turn_diff(&diff_cwd, snapshots))
        .await
        .map_err(|error| ChatError::new(format!("git diff task failed: {error}")))?
        .map_err(ChatError::new)?;
    let Some(turn_diff) = turn_diff else {
        return Ok(());
    };
    record_turn_diff(
        storage,
        conversation_id,
        turn_id,
        Path::new(default_cwd),
        turn_diff.snapshots,
        turn_diff.diff.clone(),
        turn_diff.revertible,
    )
    .map_err(ChatError::new)?;
    emit_turn_diff(
        emitter,
        conversation_id,
        turn_id,
        &turn_diff.diff,
        turn_diff.revertible,
    )
    .map_err(ChatError::new)
}

fn record_turn_diff(
    storage: &Storage,
    conversation_id: &str,
    turn_id: &str,
    cwd: &Path,
    snapshots: Vec<FileSnapshot>,
    diff: String,
    revertible: bool,
) -> Result<(), String> {
    let mut seen_paths = BTreeSet::new();
    for snapshot in snapshots {
        if !seen_paths.insert(snapshot.absolute_path.clone()) {
            continue;
        }
        storage
            .insert_turn_snapshot_with_postcondition(
                conversation_id,
                turn_id,
                &snapshot.display_path,
                &snapshot.absolute_path,
                snapshot.before.as_deref(),
                snapshot.after_exists,
                snapshot.after_sha256.as_deref(),
            )
            .map_err(|error| error.to_string())?;
    }
    storage
        .upsert_turn_diff(conversation_id, turn_id, cwd, &diff, revertible)
        .map_err(|error| error.to_string())
}

fn tool_call_output_item(call: &ToolCallEvent, output: &str) -> Value {
    match call.kind {
        ToolCallKind::Function => json!({
            "type": "function_call_output",
            "call_id": call.call_id,
            "output": output
        }),
        ToolCallKind::Custom => json!({
            "type": "custom_tool_call_output",
            "call_id": call.call_id,
            "output": output
        }),
    }
}

fn mcp_tool_call_output_item(
    call: &ToolCallEvent,
    result: &extensions::McpToolCallResult,
) -> Value {
    let mut output = Vec::new();
    if !result.text.trim().is_empty() {
        output.push(json!({
            "type": "input_text",
            "text": result.text
        }));
    }
    output.extend(result.images.iter().map(|image| {
        json!({
            "type": "input_image",
            "image_url": format!("data:{};base64,{}", image.media_type, image.data),
            "detail": "auto"
        })
    }));
    if output.is_empty() {
        output.push(json!({
            "type": "input_text",
            "text": "MCP tool completed without content."
        }));
    }
    match call.kind {
        ToolCallKind::Function => json!({
            "type": "function_call_output",
            "call_id": call.call_id,
            "output": output
        }),
        ToolCallKind::Custom => json!({
            "type": "custom_tool_call_output",
            "call_id": call.call_id,
            "output": output
        }),
    }
}

fn browser_tool_call_output_item(
    call: &ToolCallEvent,
    result: &browser::BrowserToolResult,
) -> Value {
    let mut output = vec![json!({
        "type": "input_text",
        "text": result.text
    })];
    if let Some(image) = result.screenshot_png.as_deref() {
        output.push(json!({
            "type": "input_image",
            "image_url": format!("data:image/png;base64,{image}"),
            "detail": "original"
        }));
    }
    match call.kind {
        ToolCallKind::Function => json!({
            "type": "function_call_output",
            "call_id": call.call_id,
            "output": output
        }),
        ToolCallKind::Custom => json!({
            "type": "custom_tool_call_output",
            "call_id": call.call_id,
            "output": output
        }),
    }
}

fn view_image_tool_call_output_item(
    call: &ToolCallEvent,
    image: &LocalImage,
    confirmation: &str,
) -> Value {
    let output = vec![
        json!({
            "type": "input_text",
            "text": confirmation
        }),
        json!({
            "type": "input_image",
            "image_url": format!("data:{};base64,{}", image.media_type, image.data),
            "detail": "original"
        }),
    ];
    match call.kind {
        ToolCallKind::Function => json!({
            "type": "function_call_output",
            "call_id": call.call_id,
            "output": output
        }),
        ToolCallKind::Custom => json!({
            "type": "custom_tool_call_output",
            "call_id": call.call_id,
            "output": output
        }),
    }
}

fn tool_output_to_claude_content(item: &Value) -> Value {
    let Some(output) = item.get("output") else {
        return Value::String("Tool returned no output".to_string());
    };
    let Some(blocks) = output.as_array() else {
        return output.clone();
    };
    let converted = blocks
        .iter()
        .filter_map(|block| match block.get("type").and_then(Value::as_str) {
            Some("input_text") => block
                .get("text")
                .and_then(Value::as_str)
                .map(|text| json!({ "type": "text", "text": text })),
            Some("input_image") => block
                .get("image_url")
                .and_then(Value::as_str)
                .and_then(parse_image_data_url)
                .map(|(media_type, data)| {
                    json!({
                        "type": "image",
                        "source": {
                            "type": "base64",
                            "media_type": media_type,
                            "data": data
                        }
                    })
                }),
            _ => None,
        })
        .collect::<Vec<_>>();
    if converted.is_empty() {
        Value::String("Tool returned no supported content".to_string())
    } else {
        Value::Array(converted)
    }
}

fn parse_image_data_url(url: &str) -> Option<(&str, &str)> {
    let encoded = url.strip_prefix("data:")?;
    let (media_type, data) = encoded.split_once(";base64,")?;
    matches!(
        media_type,
        "image/png" | "image/jpeg" | "image/gif" | "image/webp"
    )
    .then_some((media_type, data))
}

#[cfg(test)]
fn function_call_output_item(call_id: &str, output: &str) -> Value {
    json!({
        "type": "function_call_output",
        "call_id": call_id,
        "output": output
    })
}

fn emit_tool_call(
    emitter: &impl ChatDeltaEmitter,
    conversation_id: &str,
    call_id: &str,
    command: &str,
    cwd: &str,
    status: &str,
    justification: Option<&str>,
) -> Result<(), String> {
    emit_named_tool_call(
        emitter,
        conversation_id,
        call_id,
        command,
        "shell",
        Some(cwd),
        status,
        justification,
    )
}

#[allow(clippy::too_many_arguments)]
fn emit_named_tool_call(
    emitter: &impl ChatDeltaEmitter,
    conversation_id: &str,
    call_id: &str,
    command: &str,
    tool_kind: &str,
    cwd: Option<&str>,
    status: &str,
    justification: Option<&str>,
) -> Result<(), String> {
    emit_delta(
        emitter,
        ChatDeltaPayload {
            call_id: Some(call_id.to_string()),
            command: Some(command.to_string()),
            tool_kind: Some(tool_kind.to_string()),
            cwd: cwd.map(ToString::to_string),
            status: Some(status.to_string()),
            justification: justification.map(ToString::to_string),
            ..ChatDeltaPayload::new(conversation_id, ChatDeltaKind::ToolCall)
        },
    )
}

#[allow(clippy::too_many_arguments)]
fn emit_tool_call_with_files(
    emitter: &impl ChatDeltaEmitter,
    conversation_id: &str,
    call_id: &str,
    cwd: &str,
    status: &str,
    justification: Option<&str>,
    file_changes: Option<&[PatchFileChange]>,
    warning: Option<&str>,
) -> Result<(), String> {
    emit_delta(
        emitter,
        ChatDeltaPayload {
            call_id: Some(call_id.to_string()),
            command: Some("apply_patch".to_string()),
            tool_kind: Some("apply_patch".to_string()),
            cwd: Some(cwd.to_string()),
            status: Some(status.to_string()),
            justification: justification.map(ToString::to_string),
            file_changes: file_changes.map(|changes| changes.to_vec()),
            warning: warning.map(ToString::to_string),
            ..ChatDeltaPayload::new(conversation_id, ChatDeltaKind::ToolCall)
        },
    )
}

fn emit_tool_result(
    emitter: &impl ChatDeltaEmitter,
    conversation_id: &str,
    call_id: &str,
    status: &str,
    exit_code: Option<i32>,
    wall_time_ms: Option<u64>,
    truncated_output: bool,
) -> Result<(), String> {
    emit_tool_result_with_rtk(
        emitter,
        conversation_id,
        call_id,
        status,
        exit_code,
        wall_time_ms,
        truncated_output,
        None,
    )
}

#[allow(clippy::too_many_arguments)]
fn emit_shell_tool_result(
    emitter: &impl ChatDeltaEmitter,
    conversation_id: &str,
    call_id: &str,
    status: &str,
    exit_code: Option<i32>,
    wall_time_ms: Option<u64>,
    truncated_output: bool,
    rtk_route: Option<exec::RtkRouteKind>,
) -> Result<(), String> {
    emit_tool_result_with_rtk(
        emitter,
        conversation_id,
        call_id,
        status,
        exit_code,
        wall_time_ms,
        truncated_output,
        rtk_route,
    )
}

#[allow(clippy::too_many_arguments)]
fn emit_tool_result_with_rtk(
    emitter: &impl ChatDeltaEmitter,
    conversation_id: &str,
    call_id: &str,
    status: &str,
    exit_code: Option<i32>,
    wall_time_ms: Option<u64>,
    truncated_output: bool,
    rtk_route: Option<exec::RtkRouteKind>,
) -> Result<(), String> {
    emit_delta(
        emitter,
        ChatDeltaPayload {
            call_id: Some(call_id.to_string()),
            status: Some(status.to_string()),
            exit_code,
            wall_time_ms,
            truncated_output: Some(truncated_output),
            rtk_route: rtk_route.map(|route| route.as_str().to_string()),
            ..ChatDeltaPayload::new(conversation_id, ChatDeltaKind::ToolResult)
        },
    )
}

fn emit_turn_diff(
    emitter: &impl ChatDeltaEmitter,
    conversation_id: &str,
    turn_id: &str,
    diff: &str,
    revertible: bool,
) -> Result<(), String> {
    if diff.trim().is_empty() {
        return Ok(());
    }
    emit_delta(
        emitter,
        ChatDeltaPayload {
            diff: Some(diff.to_string()),
            turn_id: Some(turn_id.to_string()),
            revertible: Some(revertible),
            ..ChatDeltaPayload::new(conversation_id, ChatDeltaKind::TurnDiff)
        },
    )
}

fn emit_turn_diff_clear(
    emitter: &impl ChatDeltaEmitter,
    conversation_id: &str,
    turn_id: &str,
) -> Result<(), String> {
    emit_delta(
        emitter,
        ChatDeltaPayload {
            diff: Some(String::new()),
            turn_id: Some(turn_id.to_string()),
            ..ChatDeltaPayload::new(conversation_id, ChatDeltaKind::TurnDiff)
        },
    )
}

fn emit_branch_status(
    emitter: &impl ChatDeltaEmitter,
    conversation_id: &str,
    status: exec::BranchStatus,
) -> Result<(), String> {
    emit_delta(
        emitter,
        ChatDeltaPayload {
            branch: status.auto_created_branch.or(status.branch),
            warning: status.warning,
            ..ChatDeltaPayload::new(conversation_id, ChatDeltaKind::BranchStatus)
        },
    )
}

fn duration_millis(duration: Duration) -> u64 {
    duration.as_millis().min(u128::from(u64::MAX)) as u64
}

/// The engine port for a Chat mode send. It starts the engine on demand,
/// also while the Halv Engine is off.
async fn proxy_port(sidecar: &SidecarManager) -> ChatResult<u16> {
    proxy_port_from_status(sidecar.ensure_ready().await)
}

/// The engine port for a background request: the model list or a title.
/// It never starts an engine that is off (`background_status`).
async fn background_proxy_port(sidecar: &SidecarManager) -> ChatResult<u16> {
    proxy_port_from_status(sidecar.background_status().await)
}

fn proxy_port_from_status(status: SidecarStatus) -> ChatResult<u16> {
    if let Some(port) = halv_managed_listener_port(&status) {
        return Ok(port);
    }

    match status {
        SidecarStatus::Running { port, .. } => Err(ChatError::new(format!(
            "Refusing to send credentials to an unmanaged listener on 127.0.0.1:{port}"
        ))),
        SidecarStatus::Installing { port } => Err(ChatError::new(format!(
            "Proxy is still installing on port {port}"
        ))),
        SidecarStatus::Starting { port } => Err(ChatError::new(format!(
            "Proxy is still starting on port {port}"
        ))),
        SidecarStatus::NotInstalled { install_hint, .. } => Err(ChatError::new(install_hint)),
        SidecarStatus::Error { message, .. } => Err(ChatError::new(message)),
        // `ensure_ready` never returns Off. `background_status` returns Off
        // when the engine is off and no engine runs.
        SidecarStatus::Off { .. } => Err(ChatError::new(crate::sidecar::ENGINE_OFF_MESSAGE)),
    }
}

fn build_request_headers(
    bearer: &AuthBearer,
    session_id: &str,
    thread_id: &str,
) -> ChatResult<HeaderMap> {
    let mut headers = HeaderMap::new();
    let authorization = format!("Bearer {}", bearer.token);
    headers.insert(
        AUTHORIZATION,
        HeaderValue::from_str(&authorization)
            .map_err(|error| ChatError::new(format!("invalid authorization header: {error}")))?,
    );
    headers.insert(ACCEPT, HeaderValue::from_static("text/event-stream"));
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));

    if !bearer.is_api_key {
        let account_id = bearer
            .account_id
            .as_deref()
            .ok_or_else(|| ChatError::new("ChatGPT auth is missing account id"))?;
        headers.insert(
            HeaderName::from_static("chatgpt-account-id"),
            HeaderValue::from_str(account_id).map_err(|error| {
                ChatError::new(format!("invalid chatgpt-account-id header: {error}"))
            })?,
        );
        headers.insert(
            HeaderName::from_static("originator"),
            HeaderValue::from_static(ORIGINATOR),
        );
        headers.insert(
            USER_AGENT,
            HeaderValue::from_str(&user_agent())
                .map_err(|error| ChatError::new(format!("invalid user-agent header: {error}")))?,
        );
        headers.insert(
            HeaderName::from_static("session-id"),
            HeaderValue::from_str(session_id)
                .map_err(|error| ChatError::new(format!("invalid session-id header: {error}")))?,
        );
        headers.insert(
            HeaderName::from_static("thread-id"),
            HeaderValue::from_str(thread_id)
                .map_err(|error| ChatError::new(format!("invalid thread-id header: {error}")))?,
        );
        headers.insert(
            HeaderName::from_static("x-client-request-id"),
            HeaderValue::from_str(thread_id).map_err(|error| {
                ChatError::new(format!("invalid x-client-request-id header: {error}"))
            })?,
        );
    }

    Ok(headers)
}

fn build_claude_request_headers(api_key: &str) -> ChatResult<HeaderMap> {
    let mut headers = HeaderMap::new();
    headers.insert(
        HeaderName::from_static("x-api-key"),
        HeaderValue::from_str(api_key)
            .map_err(|error| ChatError::new(format!("invalid x-api-key header: {error}")))?,
    );
    headers.insert(
        HeaderName::from_static("anthropic-version"),
        HeaderValue::from_static(ANTHROPIC_VERSION),
    );
    Ok(headers)
}

async fn resolve_openai_bearer(
    storage: &Storage,
    credential_profile_id: Option<&str>,
) -> Result<AuthBearer, String> {
    auth::profiles::auth_get_bearer_for_profile(storage, credential_profile_id)
        .await
        .map_err(|error| credential_profile_resolution_error(error, credential_profile_id))
}

fn resolve_anthropic_api_key(
    storage: &Storage,
    credential_profile_id: Option<&str>,
) -> Result<String, String> {
    auth::profiles::auth_get_anthropic_api_key_for_profile(storage, credential_profile_id)
        .map_err(|error| credential_profile_resolution_error(error, credential_profile_id))
}

fn credential_profile_resolution_error(
    error: String,
    credential_profile_id: Option<&str>,
) -> String {
    if credential_profile_id.is_some() {
        format!("{error} No fallback credential was used.")
    } else {
        error
    }
}

#[cfg(test)]
#[allow(clippy::too_many_arguments)]
fn build_responses_request_body(
    conversation_id: &str,
    messages: &[StoredMessage],
    model: &str,
    effort: &str,
    goal: Option<&str>,
    project_context: Option<&ProjectPromptContext>,
    referenced_files_context: Option<&str>,
    agent_mode: bool,
    plan_mode: bool,
    effective_policy: exec::ApprovalPolicy,
) -> Value {
    build_responses_request_body_from_input(
        conversation_id,
        build_response_input_items(messages, project_context, referenced_files_context),
        model,
        effort,
        goal,
        agent_mode,
        plan_mode,
        effective_policy,
    )
}

fn build_response_input_items(
    messages: &[StoredMessage],
    project_context: Option<&ProjectPromptContext>,
    referenced_files_context: Option<&str>,
) -> Vec<Value> {
    build_response_input_items_with_compaction(
        messages,
        project_context,
        referenced_files_context,
        None,
    )
}

fn build_response_input_items_with_compaction(
    messages: &[StoredMessage],
    project_context: Option<&ProjectPromptContext>,
    referenced_files_context: Option<&str>,
    compaction: Option<&storage::ConversationCompaction>,
) -> Vec<Value> {
    let mut input = Vec::new();
    if let Some(context) = project_context {
        input.push(user_input_message(&context.environment_context));
        if let Some(agents_instructions) = &context.agents_instructions {
            input.push(user_input_message(agents_instructions));
        }
    }
    if let Some(compaction) = compaction {
        input.push(user_input_message(&compacted_summary_message(
            &compaction.summary,
        )));
    }
    if let Some(referenced_files_context) = referenced_files_context {
        if !referenced_files_context.trim().is_empty() {
            input.push(user_input_message(referenced_files_context));
        }
    }

    input.extend(
        messages
            .iter()
            .filter(|message| match compaction {
                Some(active) => message.sequence > active.upto_message_id,
                None => true,
            })
            .filter(|message| !message.error)
            .filter_map(message_to_response_item),
    );
    input
}

#[cfg(test)]
#[allow(clippy::too_many_arguments)]
fn build_responses_request_body_from_input(
    conversation_id: &str,
    input: Vec<Value>,
    model: &str,
    effort: &str,
    goal: Option<&str>,
    agent_mode: bool,
    plan_mode: bool,
    effective_policy: exec::ApprovalPolicy,
) -> Value {
    build_responses_request_body_from_input_with_mcp(
        conversation_id,
        input,
        model,
        effort,
        goal,
        agent_mode,
        plan_mode,
        effective_policy,
        &[],
    )
}

#[allow(clippy::too_many_arguments)]
fn build_responses_request_body_from_input_with_mcp(
    conversation_id: &str,
    input: Vec<Value>,
    model: &str,
    effort: &str,
    goal: Option<&str>,
    agent_mode: bool,
    plan_mode: bool,
    effective_policy: exec::ApprovalPolicy,
    mcp_tools: &[extensions::McpToolDescriptor],
) -> Value {
    build_responses_request_body_for_profile(
        conversation_id,
        input,
        model,
        effort,
        goal,
        agent_mode,
        plan_mode,
        effective_policy,
        mcp_tools,
        true,
    )
}

#[allow(clippy::too_many_arguments)]
fn build_responses_request_body_for_profile(
    conversation_id: &str,
    input: Vec<Value>,
    model: &str,
    effort: &str,
    goal: Option<&str>,
    agent_mode: bool,
    plan_mode: bool,
    effective_policy: exec::ApprovalPolicy,
    mcp_tools: &[extensions::McpToolDescriptor],
    include_coordination_tools: bool,
) -> Value {
    let tools = {
        let mut tools = vec![json!({ "type": "web_search" })];
        if agent_mode {
            tools.push(exec::shell_command_tool_schema());
            tools.push(view_image_tool_schema());
        }
        if plan_mode || (agent_mode && include_coordination_tools) {
            tools.push(update_plan_tool_schema());
        }
        if agent_mode && !plan_mode {
            if include_coordination_tools {
                tools.extend(subagent_tool_schemas());
            }
            tools.extend(browser_tool_schemas());
            if effective_policy.workspace_write_enabled() {
                tools.push(exec::apply_patch::apply_patch_tool_schema());
            }
        }
        if agent_mode {
            tools.extend(mcp_tools.iter().map(|tool| {
                json!({
                    "type": "function",
                    "name": tool.wire_name,
                    "description": tool.description,
                    "parameters": tool.input_schema,
                    "strict": false
                })
            }));
        }
        tools
    };
    let wire_effort = reasoning_effort_for_request(effort);
    let mut body = json!({
        "model": model,
        "input": input,
        "tools": tools,
        "tool_choice": "auto",
        "parallel_tool_calls": true,
        "reasoning": {
            "effort": wire_effort,
            "summary": "auto"
        },
        "store": false,
        "stream": true,
        "include": ["reasoning.encrypted_content", "web_search_call.action.sources"],
        "prompt_cache_key": conversation_id,
        "text": {
            "verbosity": "low"
        },
        "client_metadata": {}
    });

    body["instructions"] = Value::String(response_instructions(goal, agent_mode, plan_mode));

    body
}

fn bearer_is_chatgpt_subscription(bearer: &AuthBearer) -> bool {
    !bearer.is_api_key && bearer.account_id.is_some()
}

fn configure_fast_service_tier(
    body: &mut Value,
    fast_mode_enabled: bool,
    bearer: &AuthBearer,
    model_supports_fast_mode: bool,
    fast_service_tier: &str,
) {
    let object = body
        .as_object_mut()
        .expect("Responses request body should be an object");
    if fast_mode_enabled && bearer_is_chatgpt_subscription(bearer) && model_supports_fast_mode {
        object.insert(
            "service_tier".to_string(),
            Value::String(fast_service_tier.to_string()),
        );
    } else {
        object.remove("service_tier");
    }
}

fn should_fallback_from_fast_mode(
    body: &Value,
    error: &ChatError,
    already_retried: bool,
    progress: StreamAttemptProgress,
) -> bool {
    if already_retried
        || progress.any_delta_emitted
        || progress.tool_call_parsed
        || body.get("service_tier").and_then(Value::as_str).is_none()
    {
        return false;
    }
    let message = error.message.to_ascii_lowercase();
    message.contains("http 400")
}

fn update_plan_tool_schema() -> Value {
    json!({
        "type": "function",
        "name": "update_plan",
        "description": "Updates the task plan.\nProvide an optional explanation and a list of plan items, each with a step and status.\nAt most one step can be in_progress at a time.\n",
        "parameters": {
            "type": "object",
            "properties": {
                "explanation": {
                    "type": "string",
                    "description": "Optional explanation for this plan update."
                },
                "plan": {
                    "type": "array",
                    "description": "The list of steps",
                    "items": {
                        "type": "object",
                        "properties": {
                            "step": { "type": "string", "description": "Task step text." },
                            "status": {
                                "type": "string",
                                "enum": ["pending", "in_progress", "completed"],
                                "description": "Step status."
                            }
                        },
                        "required": ["step", "status"],
                        "additionalProperties": false
                    }
                }
            },
            "required": ["plan"],
            "additionalProperties": false
        },
        "strict": false
    })
}

fn view_image_tool_schema() -> Value {
    json!({
        "type": "function",
        "name": "view_image",
        "description": "Attach a local image file from the bound project to the model context for visual inspection.",
        "parameters": {
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Absolute path or path relative to the conversation project root. The resolved file must remain inside that project."
                }
            },
            "required": ["path"],
            "additionalProperties": false
        },
        "strict": false
    })
}

fn browser_tool_schemas() -> Vec<Value> {
    vec![
        json!({
            "type": "function",
            "name": "browser_open",
            "description": "Open an HTTP(S) URL in Halv's persistent controlled browser. By default navigates the active tab; set new_tab to open another tab. Follow with browser_snapshot.",
            "parameters": {
                "type": "object",
                "properties": {
                    "url": { "type": "string", "description": "Complete HTTP or HTTPS URL." },
                    "new_tab": { "type": "boolean", "description": "Open a new tab instead of navigating the active one. Defaults to false." }
                },
                "required": ["url"],
                "additionalProperties": false
            },
            "strict": false
        }),
        json!({
            "type": "function",
            "name": "browser_snapshot",
            "description": "Inspect the active browser tab as visible text plus interactive elements with refs such as r0. Refs expire after page changes, so snapshot again after clicking, submitting, or navigating.",
            "parameters": { "type": "object", "properties": {}, "additionalProperties": false },
            "strict": false
        }),
        json!({
            "type": "function",
            "name": "browser_click",
            "description": "Click an element from the latest browser_snapshot by ref.",
            "parameters": {
                "type": "object",
                "properties": { "ref": { "type": "string", "description": "Element ref from the latest snapshot, for example r3." } },
                "required": ["ref"],
                "additionalProperties": false
            },
            "strict": false
        }),
        json!({
            "type": "function",
            "name": "browser_type",
            "description": "Replace the text in an input, textarea, or contenteditable element identified by a snapshot ref, optionally submitting its form.",
            "parameters": {
                "type": "object",
                "properties": {
                    "ref": { "type": "string", "description": "Element ref from the latest snapshot." },
                    "text": { "type": "string", "description": "Text to enter." },
                    "submit": { "type": "boolean", "description": "Submit the closest form after entering text. Defaults to false." }
                },
                "required": ["ref", "text"],
                "additionalProperties": false
            },
            "strict": false
        }),
        json!({
            "type": "function",
            "name": "browser_screenshot",
            "description": "Capture the visible active tab as a PNG image for visual inspection.",
            "parameters": { "type": "object", "properties": {}, "additionalProperties": false },
            "strict": false
        }),
        json!({
            "type": "function",
            "name": "browser_tabs",
            "description": "List controlled browser tabs, their target IDs, titles, URLs, and active state.",
            "parameters": { "type": "object", "properties": {}, "additionalProperties": false },
            "strict": false
        }),
        json!({
            "type": "function",
            "name": "browser_select_tab",
            "description": "Select a controlled browser tab by target_id from browser_tabs.",
            "parameters": {
                "type": "object",
                "properties": { "target_id": { "type": "string" } },
                "required": ["target_id"],
                "additionalProperties": false
            },
            "strict": false
        }),
        json!({
            "type": "function",
            "name": "browser_close_tab",
            "description": "Close a controlled browser tab by target_id from browser_tabs.",
            "parameters": {
                "type": "object",
                "properties": { "target_id": { "type": "string" } },
                "required": ["target_id"],
                "additionalProperties": false
            },
            "strict": false
        }),
        json!({
            "type": "function",
            "name": "browser_cdp",
            "description": "Run one raw Chrome DevTools Protocol method against the active Halv browser tab. Requires opt-in Developer mode and explicit approval on every call because full CDP can expose sensitive browser internals.",
            "parameters": {
                "type": "object",
                "properties": {
                    "method": { "type": "string", "description": "CDP method such as Network.enable or Performance.getMetrics." },
                    "params": { "type": "object", "additionalProperties": true }
                },
                "required": ["method"],
                "additionalProperties": false
            },
            "strict": false
        }),
    ]
}

fn subagent_tool_schemas() -> Vec<Value> {
    vec![
        json!({
            "type": "function",
            "name": "spawn_agent",
            "description": "Spawns an agent to work on a concrete, bounded task that can run independently alongside useful local work. Returns its canonical task name. Spawned agents share the selected project and cannot spawn descendants.",
            "parameters": {
                "type": "object",
                "properties": {
                    "task_name": {
                        "type": "string",
                        "description": "Task name using lowercase letters, digits, and underscores."
                    },
                    "message": {
                        "type": "string",
                        "description": "Initial plain-text task for the new agent."
                    },
                    "fork_turns": {
                        "type": "string",
                        "description": "Context to fork: none, all, or a positive integer string. Defaults to all."
                    }
                },
                "required": ["task_name", "message"],
                "additionalProperties": false
            },
            "strict": false
        }),
        json!({
            "type": "function",
            "name": "send_message",
            "description": "Queues a message for an existing agent without starting a new turn.",
            "parameters": {
                "type": "object",
                "properties": {
                    "target": { "type": "string", "description": "Agent id or canonical task name." },
                    "message": { "type": "string", "description": "Message to queue." }
                },
                "required": ["target", "message"],
                "additionalProperties": false
            },
            "strict": false
        }),
        json!({
            "type": "function",
            "name": "followup_task",
            "description": "Sends a follow-up task to an existing agent and starts another turn when it is idle.",
            "parameters": {
                "type": "object",
                "properties": {
                    "target": { "type": "string", "description": "Agent id or canonical task name." },
                    "message": { "type": "string", "description": "Follow-up task." }
                },
                "required": ["target", "message"],
                "additionalProperties": false
            },
            "strict": false
        }),
        json!({
            "type": "function",
            "name": "wait_agent",
            "description": "Waits without model inference for a subagent result. Returns early on completion and remains cancellable. Use the default long wait instead of periodic status polling. A timeout means the agent is still working.",
            "parameters": {
                "type": "object",
                "properties": {
                    "timeout_ms": {
                        "type": "number",
                        "description": "Timeout in milliseconds. Defaults to 3600000, min 10000, max 3600000.",
                        "default": DEFAULT_AGENT_WAIT_MS
                    }
                },
                "additionalProperties": false
            },
            "strict": false
        }),
        json!({
            "type": "function",
            "name": "list_agents",
            "description": "Lists subagents and their current status.",
            "parameters": {
                "type": "object",
                "properties": {
                    "path_prefix": { "type": "string", "description": "Optional canonical task-path prefix." }
                },
                "additionalProperties": false
            },
            "strict": false
        }),
        json!({
            "type": "function",
            "name": "interrupt_agent",
            "description": "Interrupts an agent's current turn while leaving it available for follow-up tasks.",
            "parameters": {
                "type": "object",
                "properties": {
                    "target": { "type": "string", "description": "Agent id or canonical task name." }
                },
                "required": ["target"],
                "additionalProperties": false
            },
            "strict": false
        }),
    ]
}

fn reasoning_effort_for_request(effort: &str) -> &str {
    if effort == "ultra" {
        "max"
    } else {
        effort
    }
}

fn build_title_generation_request_body(
    conversation_id: &str,
    model: &str,
    first_user_message: &str,
) -> Value {
    json!({
        "model": normalize_model(model),
        "input": [user_input_message(&truncate_title_generation_input(first_user_message))],
        "tools": [],
        "tool_choice": "none",
        "parallel_tool_calls": false,
        "reasoning": {
            "effort": "low",
            "summary": "auto"
        },
        "store": false,
        "stream": false,
        "instructions": TITLE_GENERATION_INSTRUCTIONS,
        "max_output_tokens": TITLE_GENERATION_MAX_OUTPUT_TOKENS,
        "prompt_cache_key": format!("{conversation_id}:title"),
        "text": {
            "verbosity": "low"
        },
        "client_metadata": {
            "purpose": "conversation_title"
        }
    })
}

fn truncate_title_generation_input(text: &str) -> String {
    text.trim()
        .chars()
        .take(TITLE_GENERATION_INPUT_CHARS)
        .collect()
}

fn sanitize_generated_title(raw_title: &str) -> Option<String> {
    let mut title = raw_title
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    title = title
        .trim()
        .trim_matches(|character| matches!(character, '"' | '\'' | '`'))
        .trim()
        .trim_start_matches('#')
        .trim()
        .to_string();
    title = title.split_whitespace().collect::<Vec<_>>().join(" ");
    if title.is_empty() {
        return None;
    }

    let char_count = title.chars().count();
    if char_count <= TITLE_GENERATION_MAX_CHARS {
        return Some(title);
    }

    let mut capped = title
        .chars()
        .take(TITLE_GENERATION_MAX_CHARS)
        .collect::<String>();
    capped = capped.trim_end().to_string();
    capped.push_str("...");
    Some(capped)
}

fn extract_response_text(value: &Value) -> Option<String> {
    if let Some(text) = value.get("output_text").and_then(Value::as_str) {
        let text = text.trim();
        if !text.is_empty() {
            return Some(text.to_string());
        }
    }

    let output = value.get("output").and_then(Value::as_array)?;
    let mut text = String::new();
    for item in output {
        if item.get("type").and_then(Value::as_str) != Some("message") {
            continue;
        }
        let Some(content) = item.get("content").and_then(Value::as_array) else {
            continue;
        };
        for part in content {
            let part_type = part.get("type").and_then(Value::as_str);
            if part_type == Some("output_text") || part_type == Some("text") {
                if let Some(part_text) = part.get("text").and_then(Value::as_str) {
                    text.push_str(part_text);
                }
            }
        }
    }
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_string())
}

fn user_input_message(text: &str) -> Value {
    json!({
        "type": "message",
        "role": "user",
        "content": [{
            "type": "input_text",
            "text": text
        }]
    })
}

fn response_instructions(goal: Option<&str>, agent_mode: bool, plan_mode: bool) -> String {
    let environment_instructions = if agent_mode {
        AGENT_MODE_ENVIRONMENT_INSTRUCTIONS
    } else {
        CHAT_MODE_ENVIRONMENT_INSTRUCTIONS
    };

    let goal_instructions = if plan_mode {
        goal.map(str::trim)
            .filter(|goal| !goal.is_empty())
            .map(|goal| {
                format!(
                    "The user's standing goal is the planning objective for this conversation: {goal}"
                )
            })
    } else {
        standing_goal_instructions(goal, agent_mode)
    };
    let base = match goal_instructions {
        Some(goal_instructions) => {
            format!("{goal_instructions}\n\n{environment_instructions}")
        }
        None => environment_instructions.to_string(),
    };
    let base = if plan_mode {
        format!("{base}\n\n{PLAN_MODE_INSTRUCTIONS}")
    } else {
        base
    };
    format!("{base}\n\n{WEB_SEARCH_ENVIRONMENT_INSTRUCTIONS}")
}

fn memories_effectively_enabled(global_enabled: bool, mode: Option<&str>) -> bool {
    global_enabled && mode != Some("off")
}

pub(crate) fn memory_instructions_for_conversation(
    app: &AppHandle,
    storage: &Storage,
    conversation_id: &str,
) -> Option<String> {
    match load_memory_instructions(app, storage, conversation_id) {
        Ok(instructions) => instructions,
        Err(error) => {
            eprintln!("Could not load memories for {conversation_id}: {error}");
            None
        }
    }
}

fn load_memory_instructions(
    app: &AppHandle,
    storage: &Storage,
    conversation_id: &str,
) -> Result<Option<String>, String> {
    let global_enabled = app
        .state::<PreferencesManager>()
        .memories_enabled()
        .map_err(|error| format!("could not read memory preference: {error}"))?;
    let mode = storage
        .conversation_memories_mode(conversation_id)
        .map_err(|error| error.to_string())?;
    if !memories_effectively_enabled(global_enabled, mode.as_deref()) {
        return Ok(None);
    }
    let project_id = storage
        .conversation_project_id(conversation_id)
        .map_err(|error| error.to_string())?;
    let memories = storage
        .agent_memories_for_scope(project_id, MEMORY_CONTEXT_LIMIT)
        .map_err(|error| error.to_string())?;
    Ok(format_memory_instructions(&memories))
}

fn format_memory_instructions(memories: &[storage::AgentMemory]) -> Option<String> {
    if memories.is_empty() {
        return None;
    }
    let mut instructions = format!("## Memories\n\n{MEMORY_CONTEXT_CAVEAT}");
    for memory in memories.iter().take(MEMORY_CONTEXT_LIMIT) {
        let prefix = "\n- ";
        let used = instructions.chars().count();
        let available = MEMORY_CONTEXT_MAX_CHARS.saturating_sub(used);
        if available <= prefix.chars().count() {
            break;
        }
        instructions.push_str(prefix);
        let available = MEMORY_CONTEXT_MAX_CHARS.saturating_sub(instructions.chars().count());
        let content = memory.content.trim();
        let content_chars = content.chars().count();
        if content_chars <= available {
            instructions.push_str(content);
            continue;
        }
        instructions.extend(content.chars().take(available));
        break;
    }
    Some(instructions)
}

fn apply_personality_directive(body: &mut Value, personality: Personality) {
    let Some(directive) = personality.directive() else {
        return;
    };
    apply_instruction_directive(body, directive);
}

fn apply_instruction_directive(body: &mut Value, directive: &str) {
    if let Some(instructions) = body.get_mut("instructions") {
        if let Some(current) = instructions.as_str() {
            *instructions = Value::String(format!("{current}\n\n{directive}"));
            return;
        }
    }
    if let Some(text) = body
        .get_mut("system")
        .and_then(Value::as_array_mut)
        .and_then(|system| system.first_mut())
        .and_then(|entry| entry.get_mut("text"))
    {
        if let Some(current) = text.as_str() {
            *text = Value::String(format!("{current}\n\n{directive}"));
        }
    }
}

fn standing_goal_instructions(goal: Option<&str>, agent_mode: bool) -> Option<String> {
    let goal = goal?.trim();
    if goal.is_empty() {
        return None;
    }

    let mut sections = vec![format!(
        "The user's standing goal for this conversation: {goal}"
    )];
    sections.push(
        if agent_mode {
            GOAL_PROTOCOL_INSTRUCTIONS
        } else {
            GOAL_CHAT_MODE_INSTRUCTIONS
        }
        .to_string(),
    );
    Some(sections.join("\n\n"))
}

fn assistant_text_without_goal_marker(
    assistant_text: &str,
    active_goal: Option<&str>,
) -> (String, Option<&'static str>) {
    if active_goal
        .map(str::trim)
        .filter(|goal| !goal.is_empty())
        .is_none()
    {
        return (assistant_text.to_string(), None);
    }

    let trimmed = assistant_text.trim_end();
    if let Some(text) = trimmed.strip_suffix(GOAL_DONE_MARKER) {
        return (text.trim_end().to_string(), Some("done"));
    }
    if let Some(text) = trimmed.strip_suffix(GOAL_CONTINUE_MARKER) {
        return (text.trim_end().to_string(), Some("continue"));
    }
    if let Some(text) = trimmed.strip_suffix(GOAL_BLOCKED_MARKER) {
        return (text.trim_end().to_string(), None);
    }

    (assistant_text.to_string(), Some("continue"))
}

fn build_project_prompt_context(
    project: &storage::Project,
    agent_mode: bool,
    effective_policy: exec::ApprovalPolicy,
) -> ChatResult<ProjectPromptContext> {
    let root = projects::validate_registered_project(project).map_err(ChatError::new)?;
    let root_path = root.to_string_lossy().to_string();

    Ok(ProjectPromptContext {
        environment_context: render_environment_context(
            &root_path,
            &current_shell_name(),
            &current_date(),
            &current_timezone(),
            agent_mode,
            effective_policy,
        ),
        agents_instructions: render_agents_instructions(&root),
    })
}

fn render_environment_context(
    cwd: &str,
    shell: &str,
    current_date: &str,
    timezone: &str,
    agent_mode: bool,
    effective_policy: exec::ApprovalPolicy,
) -> String {
    let filesystem = if agent_mode && effective_policy.workspace_write_enabled() {
        format!(
            "\n  <filesystem><workspace_roots><root>{}</root></workspace_roots><permission_profile type=\"managed\"><entry access=\"read\"><special>:root</special></entry><entry access=\"write\"><path>{}</path></entry><entry access=\"write\"><special>:slash_tmp</special></entry><entry access=\"write\"><special>:tmpdir</special></entry><entry access=\"read\"><path>{}/.git</path></entry><entry access=\"read\"><path>{}/.codex</path></entry><entry access=\"read\"><path>{}/.halv</path></entry><entry access=\"read\"><path>{}/.agents</path></entry></permission_profile></filesystem>",
            escape_xml_text(cwd),
            escape_xml_text(cwd),
            escape_xml_text(cwd),
            escape_xml_text(cwd),
            escape_xml_text(cwd),
            escape_xml_text(cwd),
        )
    } else if agent_mode {
        format!(
            "\n  <filesystem><workspace_roots><root>{}</root></workspace_roots><permission_profile type=\"managed\"><entry access=\"read\"><special>:root</special></entry></permission_profile></filesystem>",
            escape_xml_text(cwd)
        )
    } else {
        "\n  <!-- Project context only; shell tools are disabled for this conversation. -->"
            .to_string()
    };
    format!(
        "<environment_context>\n  <cwd>{}</cwd>\n  <shell>{}</shell>\n  <current_date>{}</current_date>\n  <timezone>{}</timezone>{}\n</environment_context>",
        escape_xml_text(cwd),
        escape_xml_text(shell),
        escape_xml_text(current_date),
        escape_xml_text(timezone),
        filesystem
    )
}

fn render_agents_instructions(project_root: &Path) -> Option<String> {
    let global_path = global_agents_path();
    render_agents_instructions_with_global(project_root, global_path.as_deref())
}

fn render_agents_instructions_with_global(
    project_root: &Path,
    global_path: Option<&Path>,
) -> Option<String> {
    let body = collect_agents_doc_body_with_global(project_root, global_path)?;
    Some(format!(
        "# AGENTS.md instructions for {}\n\n<INSTRUCTIONS>\n{}\n</INSTRUCTIONS>",
        project_root.to_string_lossy(),
        body
    ))
}

fn collect_agents_doc_body_with_global(
    project_root: &Path,
    global_path: Option<&Path>,
) -> Option<String> {
    let mut body = String::new();
    let mut remaining = AGENTS_DOC_MAX_BYTES;

    if let Some(global_path) = global_path {
        if let Some(global) = read_agents_file(global_path, remaining) {
            append_with_budget(&mut body, &global, &mut remaining);
        }
    }

    if remaining > 0 {
        if let Some(project_doc_path) = project_agents_path(project_root) {
            if let Some(project_doc) = read_agents_file(&project_doc_path, remaining) {
                if !body.is_empty() {
                    append_with_budget(&mut body, PROJECT_DOC_SEPARATOR, &mut remaining);
                }
                if remaining > 0 {
                    append_with_budget(&mut body, &project_doc, &mut remaining);
                }
            }
        }
    }

    if body.is_empty() {
        None
    } else {
        Some(body)
    }
}

fn global_agents_path() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .map(|home| home.join(".codex").join("AGENTS.md"))
        .filter(|path| path.is_file())
}

fn project_agents_path(project_root: &Path) -> Option<PathBuf> {
    confined_project_file(project_root, "AGENTS.override.md")
        .or_else(|| confined_project_file(project_root, "AGENTS.md"))
}

/// Resolve `<project_root>/<name>` only if it is a regular file whose real
/// location stays inside the project root. This blocks a malicious repo from
/// symlinking AGENTS.md at, say, ~/.ssh/id_rsa and exfiltrating its contents
/// into the outbound request.
fn confined_project_file(project_root: &Path, relative_path: &str) -> Option<PathBuf> {
    let relative = Path::new(relative_path);
    if relative.is_absolute()
        || relative
            .components()
            .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        return None;
    }
    let candidate = project_root.join(relative);
    let real = candidate.canonicalize().ok()?;
    if !real.is_file() {
        return None;
    }
    let root = project_root.canonicalize().ok()?;
    real.starts_with(&root).then_some(real)
}

fn build_referenced_files_context(
    project_root: Option<&Path>,
    mentions: Option<&[String]>,
) -> ReferencedFilesContext {
    let Some(mentions) = mentions else {
        return ReferencedFilesContext::default();
    };
    if mentions.is_empty() {
        return ReferencedFilesContext::default();
    }

    let mut seen = BTreeSet::new();
    let mut remaining = FILE_MENTION_TOTAL_MAX_BYTES;
    let mut blocks = Vec::new();
    let mut skipped_mentions = Vec::new();
    for raw_path in mentions {
        let display_path = raw_path.trim();
        if display_path.is_empty() || !seen.insert(display_path.to_string()) {
            continue;
        }
        if remaining == 0 {
            skipped_mentions.push(display_path.to_string());
            continue;
        }
        let Some(project_root) = project_root else {
            skipped_mentions.push(display_path.to_string());
            continue;
        };

        let Some(path) = confined_project_file(project_root, display_path) else {
            eprintln!("Skipping referenced file outside project: {display_path}");
            skipped_mentions.push(display_path.to_string());
            continue;
        };
        let budget = FILE_MENTION_MAX_BYTES.min(remaining);
        let Some((text, source_truncated)) = read_utf8_file_prefix(&path, budget) else {
            eprintln!("Skipping unreadable or non-UTF-8 referenced file: {display_path}");
            skipped_mentions.push(display_path.to_string());
            continue;
        };
        let content = truncate_referenced_file_text(&text, budget, source_truncated);
        remaining = remaining.saturating_sub(content.len());
        blocks.push(format!(
            "Referenced file: {display_path}\n```\n{content}\n```"
        ));
    }

    ReferencedFilesContext {
        context: (!blocks.is_empty()).then(|| blocks.join("\n\n")),
        skipped_mentions,
    }
}

fn merge_turn_contexts(first: Option<String>, second: Option<String>) -> Option<String> {
    match (first, second) {
        (Some(first), Some(second)) => Some(format!("{first}\n\n{second}")),
        (Some(context), None) | (None, Some(context)) => Some(context),
        (None, None) => None,
    }
}

fn normalize_mentions(mentions: Vec<String>) -> Vec<String> {
    let mut seen = BTreeSet::new();
    mentions
        .into_iter()
        .map(|mention| mention.trim().to_string())
        .filter(|mention| !mention.is_empty())
        .filter(|mention| seen.insert(mention.clone()))
        .collect()
}

fn normalize_image_attachments(
    attachments: Vec<ImageAttachmentInput>,
) -> Result<Vec<NewMessageAttachment>, String> {
    if attachments.len() > MAX_IMAGE_ATTACHMENTS_PER_MESSAGE {
        return Err(format!(
            "Attach at most {MAX_IMAGE_ATTACHMENTS_PER_MESSAGE} images to one message"
        ));
    }

    let mut total_bytes = 0_usize;
    let mut normalized = Vec::with_capacity(attachments.len());
    for attachment in attachments {
        let name = attachment.name.trim();
        if name.is_empty() {
            return Err("Every image attachment needs a filename".to_string());
        }
        if name.chars().count() > MAX_IMAGE_ATTACHMENT_NAME_CHARS {
            return Err(format!(
                "Image filename is too long; use at most {MAX_IMAGE_ATTACHMENT_NAME_CHARS} characters"
            ));
        }
        if name.chars().any(char::is_control) || name.contains('/') || name.contains('\\') {
            return Err(format!("Image filename `{name}` is not safe"));
        }

        let media_type = match attachment.media_type.trim().to_ascii_lowercase().as_str() {
            "image/jpeg" | "image/jpg" => "image/jpeg",
            "image/png" => "image/png",
            "image/webp" => "image/webp",
            _ => {
                return Err(format!(
                    "`{name}` is not a supported image. Use PNG, JPEG, or WebP"
                ))
            }
        };
        if attachment.size_bytes == 0 {
            return Err(format!("`{name}` is empty"));
        }
        if attachment.size_bytes > MAX_IMAGE_ATTACHMENT_BYTES as u64 {
            return Err(format!(
                "`{name}` exceeds the {} MiB per-image limit",
                MAX_IMAGE_ATTACHMENT_BYTES / (1024 * 1024)
            ));
        }
        let max_encoded_bytes = MAX_IMAGE_ATTACHMENT_BYTES.div_ceil(3) * 4;
        if attachment.data.trim().len() > max_encoded_bytes {
            return Err(format!(
                "`{name}` exceeds the {} MiB per-image limit",
                MAX_IMAGE_ATTACHMENT_BYTES / (1024 * 1024)
            ));
        }
        let data = STANDARD
            .decode(attachment.data.trim())
            .map_err(|_| format!("`{name}` contains invalid image data"))?;
        if data.len() as u64 != attachment.size_bytes {
            return Err(format!("`{name}` has inconsistent image size metadata"));
        }
        if data.len() > MAX_IMAGE_ATTACHMENT_BYTES {
            return Err(format!(
                "`{name}` exceeds the {} MiB per-image limit",
                MAX_IMAGE_ATTACHMENT_BYTES / (1024 * 1024)
            ));
        }
        let detected_type = detect_image_media_type(&data)
            .ok_or_else(|| format!("`{name}` does not contain a valid PNG, JPEG, or WebP image"))?;
        if media_type != detected_type {
            return Err(format!(
                "`{name}` content does not match its declared {media_type} type"
            ));
        }
        total_bytes = total_bytes
            .checked_add(data.len())
            .ok_or_else(|| "Attached image size overflowed".to_string())?;
        if total_bytes > MAX_IMAGE_ATTACHMENTS_TOTAL_BYTES {
            return Err(format!(
                "Attached images exceed the {} MiB total limit",
                MAX_IMAGE_ATTACHMENTS_TOTAL_BYTES / (1024 * 1024)
            ));
        }
        normalized.push(NewMessageAttachment {
            name: name.to_string(),
            media_type: media_type.to_string(),
            data,
        });
    }
    Ok(normalized)
}

#[tauri::command]
pub fn composer_read_dropped_image(path: String) -> Result<ComposerDroppedImage, String> {
    let image_path = Path::new(&path);
    let name = image_path
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .ok_or_else(|| "Choose a PNG, JPEG, or WebP image".to_string())?;
    let extension = image_path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase);
    let expected_media_type = match extension.as_deref() {
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("webp") => "image/webp",
        _ => {
            return Err(format!(
                "`{name}` is not a supported image. Use PNG, JPEG, or WebP"
            ))
        }
    };

    let metadata = fs::metadata(image_path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            format!("Image file `{}` was not found", image_path.display())
        } else {
            format!("Could not inspect image file `{}`", image_path.display())
        }
    })?;
    if !metadata.is_file() {
        return Err(format!(
            "Image path `{}` is not a regular file",
            image_path.display()
        ));
    }
    if metadata.len() == 0 {
        return Err(format!("`{name}` is empty"));
    }
    if metadata.len() > MAX_IMAGE_ATTACHMENT_BYTES as u64 {
        return Err(format!(
            "`{name}` exceeds the {} MiB per-image limit",
            MAX_IMAGE_ATTACHMENT_BYTES / (1024 * 1024)
        ));
    }

    let file = fs::File::open(image_path)
        .map_err(|_| format!("Could not open image file `{}`", image_path.display()))?;
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.take((MAX_IMAGE_ATTACHMENT_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| format!("Could not read image file `{}`", image_path.display()))?;
    if bytes.is_empty() {
        return Err(format!("`{name}` is empty"));
    }
    if bytes.len() > MAX_IMAGE_ATTACHMENT_BYTES {
        return Err(format!(
            "`{name}` exceeds the {} MiB per-image limit",
            MAX_IMAGE_ATTACHMENT_BYTES / (1024 * 1024)
        ));
    }

    let media_type = detect_image_media_type(&bytes)
        .ok_or_else(|| format!("`{name}` does not contain a valid PNG, JPEG, or WebP image"))?;
    if media_type != expected_media_type {
        return Err(format!(
            "`{name}` content does not match its declared image type"
        ));
    }

    Ok(ComposerDroppedImage {
        name: name.to_string(),
        media_type: media_type.to_string(),
        size_bytes: bytes.len() as u64,
        data: STANDARD.encode(bytes),
    })
}

fn detect_image_media_type(data: &[u8]) -> Option<&'static str> {
    if data.starts_with(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]) {
        return Some("image/png");
    }
    if data.starts_with(&[0xff, 0xd8, 0xff]) {
        return Some("image/jpeg");
    }
    if data.len() >= 12 && data.starts_with(b"RIFF") && &data[8..12] == b"WEBP" {
        return Some("image/webp");
    }
    None
}

fn validate_image_attachment_history(messages: &[StoredMessage]) -> Result<(), String> {
    let mut count = 0_usize;
    let mut total_bytes = 0_u64;
    let max_encoded_bytes = MAX_IMAGE_ATTACHMENT_BYTES.div_ceil(3) * 4;
    for message in messages {
        if message.attachments.len() > MAX_IMAGE_ATTACHMENTS_PER_MESSAGE {
            return Err(format!(
                "A stored message includes more than {MAX_IMAGE_ATTACHMENTS_PER_MESSAGE} images. Edit that message before retrying"
            ));
        }
        for attachment in &message.attachments {
            count = count
                .checked_add(1)
                .ok_or_else(|| "Image attachment count overflowed".to_string())?;
            if attachment.size_bytes == 0
                || attachment.size_bytes > MAX_IMAGE_ATTACHMENT_BYTES as u64
                || attachment.data.len() > max_encoded_bytes
            {
                return Err(format!(
                    "Stored image `{}` exceeds the {} MiB per-image limit. Edit that message before retrying",
                    attachment.name,
                    MAX_IMAGE_ATTACHMENT_BYTES / (1024 * 1024)
                ));
            }
            let data = STANDARD.decode(&attachment.data).map_err(|_| {
                format!(
                    "Stored image `{}` contains invalid data. Edit that message before retrying",
                    attachment.name
                )
            })?;
            let detected_type = detect_image_media_type(&data).ok_or_else(|| {
                format!(
                    "Stored image `{}` is not a valid PNG, JPEG, or WebP image. Edit that message before retrying",
                    attachment.name
                )
            })?;
            if data.len() as u64 != attachment.size_bytes || detected_type != attachment.media_type
            {
                return Err(format!(
                    "Stored image `{}` has inconsistent metadata. Edit that message before retrying",
                    attachment.name
                ));
            }
            total_bytes = total_bytes
                .checked_add(data.len() as u64)
                .ok_or_else(|| "Image attachment size overflowed".to_string())?;
        }
    }
    if count > MAX_IMAGE_ATTACHMENTS_PER_HISTORY {
        return Err(format!(
            "This conversation includes more than {MAX_IMAGE_ATTACHMENTS_PER_HISTORY} images. Start a new chat or remove older image messages"
        ));
    }
    if total_bytes > MAX_IMAGE_ATTACHMENTS_TOTAL_BYTES as u64 {
        return Err(format!(
            "This conversation includes more than {} MiB of images. Start a new chat or remove older image messages",
            MAX_IMAGE_ATTACHMENTS_TOTAL_BYTES / (1024 * 1024)
        ));
    }
    Ok(())
}

fn validate_new_attachments_against_history(
    messages: &[StoredMessage],
    attachments: &[NewMessageAttachment],
) -> Result<(), String> {
    let existing_count = messages
        .iter()
        .map(|message| message.attachments.len())
        .sum::<usize>();
    if existing_count.saturating_add(attachments.len()) > MAX_IMAGE_ATTACHMENTS_PER_HISTORY {
        return Err(format!(
            "This conversation would include more than {MAX_IMAGE_ATTACHMENTS_PER_HISTORY} images. Start a new chat or edit an older image message"
        ));
    }
    let existing_bytes = messages
        .iter()
        .flat_map(|message| &message.attachments)
        .map(|attachment| attachment.size_bytes)
        .sum::<u64>();
    let added_bytes = attachments
        .iter()
        .map(|attachment| attachment.data.len() as u64)
        .sum::<u64>();
    if existing_bytes.saturating_add(added_bytes) > MAX_IMAGE_ATTACHMENTS_TOTAL_BYTES as u64 {
        return Err(format!(
            "This conversation would include more than {} MiB of images. Start a new chat or edit an older image message",
            MAX_IMAGE_ATTACHMENTS_TOTAL_BYTES / (1024 * 1024)
        ));
    }
    Ok(())
}

fn truncate_referenced_file_text(text: &str, max_bytes: usize, source_truncated: bool) -> String {
    if !source_truncated && text.len() <= max_bytes {
        return text.to_string();
    }
    if max_bytes == 0 {
        return String::new();
    }

    if max_bytes <= FILE_MENTION_TRUNCATED_SUFFIX.len() {
        return utf8_prefix(FILE_MENTION_TRUNCATED_SUFFIX, max_bytes).to_string();
    }

    let body_budget = max_bytes - FILE_MENTION_TRUNCATED_SUFFIX.len();
    let mut truncated = utf8_prefix(text, body_budget).trim_end().to_string();
    truncated.push_str(FILE_MENTION_TRUNCATED_SUFFIX);
    truncated
}

fn utf8_prefix(text: &str, max_bytes: usize) -> &str {
    let boundary = text
        .char_indices()
        .map(|(index, _)| index)
        .chain(std::iter::once(text.len()))
        .take_while(|index| *index <= max_bytes)
        .last()
        .unwrap_or(0);
    &text[..boundary]
}

fn read_agents_file(path: &Path, max_bytes: usize) -> Option<String> {
    read_utf8_file_prefix(path, max_bytes).map(|(text, _)| text)
}

fn read_utf8_file_prefix(path: &Path, max_bytes: usize) -> Option<(String, bool)> {
    let path_metadata = fs::symlink_metadata(path).ok()?;
    if path_metadata.file_type().is_symlink() || !path_metadata.is_file() {
        return None;
    }

    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    #[cfg(windows)]
    options.custom_flags(windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT);
    let file = options.open(path).ok()?;
    let metadata = file.metadata().ok()?;
    if !metadata.is_file() {
        return None;
    }

    let read_limit = max_bytes.saturating_add(4);
    let mut bytes = Vec::with_capacity(metadata.len().min(read_limit as u64) as usize);
    file.take(read_limit as u64).read_to_end(&mut bytes).ok()?;
    let valid_len = match std::str::from_utf8(&bytes) {
        Ok(_) => bytes.len(),
        Err(error) if error.error_len().is_none() => error.valid_up_to(),
        Err(_) => return None,
    };
    let text = std::str::from_utf8(&bytes[..valid_len]).ok()?;
    let prefix = utf8_prefix(text, max_bytes).to_string();
    let truncated = bytes.len() > prefix.len() || metadata.len() > prefix.len() as u64;
    Some((prefix, truncated))
}

fn append_with_budget(target: &mut String, text: &str, remaining: &mut usize) {
    if *remaining == 0 {
        return;
    }
    let allowed = text
        .char_indices()
        .map(|(index, _)| index)
        .chain(std::iter::once(text.len()))
        .take_while(|index| *index <= *remaining)
        .last()
        .unwrap_or(0);
    target.push_str(&text[..allowed]);
    *remaining -= allowed;
}

fn current_shell_name() -> String {
    std::env::var_os("SHELL")
        .and_then(|shell| {
            PathBuf::from(shell)
                .file_name()
                .map(|name| name.to_os_string())
        })
        .and_then(|name| name.into_string().ok())
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| "sh".to_string())
}

fn current_date() -> String {
    Local::now().date_naive().to_string()
}

fn current_timezone() -> String {
    if let Ok(value) = std::env::var("TZ") {
        let value = value.trim();
        if !value.is_empty() {
            return value.to_string();
        }
    }

    #[cfg(unix)]
    if let Ok(path) = fs::read_link("/etc/localtime") {
        let text = path.to_string_lossy();
        if let Some((_, zone)) = text.split_once("zoneinfo/") {
            if !zone.trim().is_empty() {
                return zone.to_string();
            }
        }
    }

    Local::now().offset().to_string()
}

fn escape_xml_text(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn message_to_response_item(message: &StoredMessage) -> Option<Value> {
    match message.role.as_str() {
        "user" | "developer" | "system" => {
            let mut content = if message.role == "user" {
                message
                    .attachments
                    .iter()
                    .map(|attachment| {
                        json!({
                            "type": "input_image",
                            "image_url": format!(
                                "data:{};base64,{}",
                                attachment.media_type, attachment.data
                            ),
                            "detail": "auto"
                        })
                    })
                    .collect::<Vec<_>>()
            } else {
                Vec::new()
            };
            if !message.content.trim().is_empty() {
                content.push(json!({
                    "type": "input_text",
                    "text": message.content
                }));
            }
            (!content.is_empty()).then(|| {
                json!({
                    "type": "message",
                    "role": message.role,
                    "content": content
                })
            })
        }
        "assistant" => Some(json!({
            "type": "message",
            "role": "assistant",
            "content": [{
                "type": "output_text",
                "text": message.content
            }]
        })),
        _ => None,
    }
}

fn parse_sse_frame(frame: &str) -> ChatResult<Option<ParsedSseEvent>> {
    let Some(value) = parse_raw_sse_frame(frame)? else {
        return Ok(None);
    };
    parse_responses_event(value).map(Some)
}

fn parse_raw_sse_frame(frame: &str) -> ChatResult<Option<Value>> {
    let mut data = String::new();
    for raw_line in frame.lines() {
        let line = raw_line.strip_suffix('\r').unwrap_or(raw_line);
        if line.starts_with(':') || line.is_empty() {
            continue;
        }
        let Some(rest) = line.strip_prefix("data:") else {
            continue;
        };
        if !data.is_empty() {
            data.push('\n');
        }
        data.push_str(rest.strip_prefix(' ').unwrap_or(rest));
    }

    if data.is_empty() || data == "[DONE]" {
        return Ok(None);
    }

    let value: Value = serde_json::from_str(&data)
        .map_err(|error| ChatError::new(format!("invalid SSE JSON payload: {error}")))?;
    Ok(Some(value))
}

fn parse_responses_event(value: Value) -> ChatResult<ParsedSseEvent> {
    let event_type = value
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default();

    match event_type {
        "response.output_text.delta" => Ok(ParsedSseEvent::Delta(required_string(
            &value, "delta", event_type,
        )?)),
        "response.reasoning_summary_text.delta" => {
            require_i64(&value, "summary_index", event_type)?;
            Ok(ParsedSseEvent::Reasoning(required_string(
                &value, "delta", event_type,
            )?))
        }
        "response.reasoning_text.delta" => {
            require_i64(&value, "content_index", event_type)?;
            Ok(ParsedSseEvent::Reasoning(required_string(
                &value, "delta", event_type,
            )?))
        }
        "response.reasoning_summary_part.added" => Ok(ParsedSseEvent::ReasoningBoundary),
        "response.output_item.done" => extract_output_item_done(&value)?
            .map(Ok)
            .unwrap_or(Ok(ParsedSseEvent::Ignored)),
        "response.web_search_call.in_progress" | "response.web_search_call.searching" => {
            Ok(ParsedSseEvent::WebSearchCall {
                call_id: required_string(&value, "item_id", event_type)?,
                command: "searching the web".to_string(),
                status: "running".to_string(),
                item: None,
                tail: None,
            })
        }
        "response.completed" => Ok(ParsedSseEvent::Completed {
            usage: extract_usage(&value)?,
            service_tier: extract_service_tier(&value),
        }),
        "response.failed" => Ok(ParsedSseEvent::Failed(extract_response_error(&value))),
        "response.incomplete" => Ok(ParsedSseEvent::Failed(extract_incomplete_reason(&value))),
        "error" => Ok(ParsedSseEvent::Failed(extract_error_event(&value))),
        "response.created"
        | "response.output_item.added"
        | "response.custom_tool_call_input.delta" => Ok(ParsedSseEvent::Ignored),
        _ => Ok(ParsedSseEvent::Ignored),
    }
}

fn extract_usage(value: &Value) -> ChatResult<Option<TokenUsage>> {
    let Some(usage) = value
        .get("response")
        .and_then(|response| response.get("usage"))
    else {
        return Ok(None);
    };

    let usage = serde_json::from_value::<UsageWire>(usage.clone())
        .map_err(|error| ChatError::new(format!("invalid response.completed usage: {error}")))?;
    Ok(Some(usage.into()))
}

fn extract_service_tier(value: &Value) -> Option<String> {
    value
        .get("response")
        .and_then(|response| response.get("service_tier"))
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

fn extract_final_message_from_item(item: &Value) -> Option<String> {
    if item.get("type")?.as_str()? != "message" {
        return None;
    }
    let content = item.get("content")?.as_array()?;
    let mut text = String::new();
    for part in content {
        if part.get("type").and_then(Value::as_str) == Some("output_text") {
            text.push_str(part.get("text").and_then(Value::as_str).unwrap_or_default());
        }
    }
    if text.is_empty() {
        None
    } else {
        Some(text)
    }
}

fn extract_output_item_done(value: &Value) -> ChatResult<Option<ParsedSseEvent>> {
    let Some(item) = value.get("item") else {
        return Ok(None);
    };
    if item.get("type").and_then(Value::as_str) == Some("function_call") {
        return extract_function_call(value).map(|call| call.map(ParsedSseEvent::ToolCall));
    }
    if item.get("type").and_then(Value::as_str) == Some("custom_tool_call") {
        return extract_custom_tool_call(value).map(|call| call.map(ParsedSseEvent::ToolCall));
    }
    if item.get("type").and_then(Value::as_str) == Some("web_search_call") {
        let call_id = item
            .get("id")
            .and_then(Value::as_str)
            .ok_or_else(|| ChatError::new("web_search_call item missing id"))?
            .to_string();
        let status = match item.get("status").and_then(Value::as_str) {
            Some("failed") => "failed",
            _ => "completed",
        }
        .to_string();
        return Ok(Some(ParsedSseEvent::WebSearchCall {
            call_id,
            command: openai_web_search_command(item),
            status,
            tail: openai_web_search_tail(item),
            item: Some(strip_upstream_item_id(item.clone())),
        }));
    }

    let stripped = strip_upstream_item_id(item.clone());
    if let Some(text) = extract_final_message_from_item(&stripped) {
        return Ok(Some(ParsedSseEvent::FinalMessage {
            text,
            item: stripped,
        }));
    }

    Ok(Some(ParsedSseEvent::OutputItem(stripped)))
}

fn openai_web_search_command(item: &Value) -> String {
    let Some(action) = item.get("action") else {
        return "searching the web".to_string();
    };
    if let Some(query) = action.get("query").and_then(Value::as_str) {
        return query.to_string();
    }
    if let Some(queries) = action.get("queries").and_then(Value::as_array) {
        let queries = queries
            .iter()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>()
            .join(" · ");
        if !queries.is_empty() {
            return queries;
        }
    }
    match action.get("type").and_then(Value::as_str) {
        Some("open_page") => action
            .get("url")
            .and_then(Value::as_str)
            .map(|url| format!("open {url}"))
            .unwrap_or_else(|| "open web page".to_string()),
        Some("find_in_page") => action
            .get("pattern")
            .and_then(Value::as_str)
            .map(|pattern| format!("find {pattern}"))
            .unwrap_or_else(|| "find in web page".to_string()),
        _ => "searching the web".to_string(),
    }
}

fn openai_web_search_tail(item: &Value) -> Option<String> {
    let sources = item.pointer("/action/sources").and_then(Value::as_array)?;
    let lines = sources
        .iter()
        .filter_map(|source| {
            let url = source.get("url").and_then(Value::as_str)?;
            let title = source
                .get("title")
                .and_then(Value::as_str)
                .filter(|title| !title.trim().is_empty())
                .unwrap_or(url);
            Some(format!("{title}\n{url}"))
        })
        .collect::<Vec<_>>();
    (!lines.is_empty()).then(|| lines.join("\n\n"))
}

fn extract_function_call(value: &Value) -> ChatResult<Option<ToolCallEvent>> {
    let Some(item) = value.get("item") else {
        return Ok(None);
    };
    if item.get("type").and_then(Value::as_str) != Some("function_call") {
        return Ok(None);
    }
    let call_id = item
        .get("call_id")
        .and_then(Value::as_str)
        .ok_or_else(|| ChatError::new("function_call item missing call_id"))?
        .to_string();
    let name = item
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| ChatError::new("function_call item missing name"))?
        .to_string();
    let input = item
        .get("arguments")
        .and_then(Value::as_str)
        .ok_or_else(|| ChatError::new("function_call item missing arguments"))?
        .to_string();
    let stripped = strip_upstream_item_id(item.clone());
    Ok(Some(ToolCallEvent {
        call_id,
        name,
        input,
        kind: ToolCallKind::Function,
        item: stripped,
    }))
}

fn extract_custom_tool_call(value: &Value) -> ChatResult<Option<ToolCallEvent>> {
    let Some(item) = value.get("item") else {
        return Ok(None);
    };
    if item.get("type").and_then(Value::as_str) != Some("custom_tool_call") {
        return Ok(None);
    }
    let call_id = item
        .get("call_id")
        .and_then(Value::as_str)
        .ok_or_else(|| ChatError::new("custom_tool_call item missing call_id"))?
        .to_string();
    let name = item
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| ChatError::new("custom_tool_call item missing name"))?
        .to_string();
    let input = item
        .get("input")
        .and_then(Value::as_str)
        .ok_or_else(|| ChatError::new("custom_tool_call item missing input"))?
        .to_string();
    let stripped = strip_upstream_item_id(item.clone());
    Ok(Some(ToolCallEvent {
        call_id,
        name,
        input,
        kind: ToolCallKind::Custom,
        item: stripped,
    }))
}

fn strip_upstream_item_id(mut item: Value) -> Value {
    if let Some(object) = item.as_object_mut() {
        object.remove("id");
    }
    item
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct WebCitation {
    title: String,
    url: String,
}

fn collect_openai_web_citations(item: &Value, citations: &mut Vec<WebCitation>) {
    let Some(content) = item.get("content").and_then(Value::as_array) else {
        return;
    };
    for part in content {
        let Some(annotations) = part.get("annotations").and_then(Value::as_array) else {
            continue;
        };
        for annotation in annotations {
            if annotation.get("type").and_then(Value::as_str) != Some("url_citation") {
                continue;
            }
            let Some(url) = annotation.get("url").and_then(Value::as_str) else {
                continue;
            };
            let title = annotation
                .get("title")
                .and_then(Value::as_str)
                .unwrap_or(url);
            push_web_citation(citations, title, url);
        }
    }
}

fn push_web_citation(citations: &mut Vec<WebCitation>, title: &str, url: &str) {
    if !(url.starts_with("https://") || url.starts_with("http://"))
        || citations.iter().any(|citation| citation.url == url)
    {
        return;
    }
    citations.push(WebCitation {
        title: title.trim().to_string(),
        url: url.to_string(),
    });
}

fn web_citations_markdown(citations: &[WebCitation]) -> String {
    if citations.is_empty() {
        return String::new();
    }
    let sources = citations
        .iter()
        .map(|citation| {
            let title = markdown_link_label(if citation.title.is_empty() {
                &citation.url
            } else {
                &citation.title
            });
            format!("- [{title}]({})", citation.url)
        })
        .collect::<Vec<_>>()
        .join("\n");
    format!("\n\nSources:\n\n{sources}")
}

fn append_citation_suffix_preserving_goal_marker(text: &mut String, suffix: &str) {
    let trimmed = text.trim_end();
    for marker in [GOAL_DONE_MARKER, GOAL_CONTINUE_MARKER, GOAL_BLOCKED_MARKER] {
        if let Some(prefix) = trimmed.strip_suffix(marker) {
            *text = format!("{}{suffix}\n\n{marker}", prefix.trim_end());
            return;
        }
    }
    text.push_str(suffix);
}

fn markdown_link_label(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('[', "\\[")
        .replace(']', "\\]")
        .replace(['\r', '\n'], " ")
}

fn extract_response_error(value: &Value) -> String {
    value
        .get("response")
        .and_then(|response| response.get("error"))
        .and_then(|error| error.get("message"))
        .and_then(Value::as_str)
        .unwrap_or("response.failed")
        .to_string()
}

fn extract_incomplete_reason(value: &Value) -> String {
    let reason = value
        .get("response")
        .and_then(|response| response.get("incomplete_details"))
        .and_then(|details| details.get("reason"))
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    format!("response.incomplete: {reason}")
}

fn extract_error_event(value: &Value) -> String {
    value
        .get("error")
        .and_then(|error| error.get("message"))
        .or_else(|| value.get("message"))
        .and_then(Value::as_str)
        .unwrap_or("SSE error event")
        .to_string()
}

fn required_string(value: &Value, key: &str, event_type: &str) -> ChatResult<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(ToString::to_string)
        .ok_or_else(|| ChatError::new(format!("{event_type} missing string field {key}")))
}

fn require_i64(value: &Value, key: &str, event_type: &str) -> ChatResult<i64> {
    value
        .get(key)
        .and_then(Value::as_i64)
        .ok_or_else(|| ChatError::new(format!("{event_type} missing integer field {key}")))
}

fn find_sse_delimiter(buffer: &[u8]) -> Option<(usize, usize)> {
    for index in 0..buffer.len() {
        let remaining = &buffer[index..];
        if remaining.starts_with(b"\n\n") {
            return Some((index, 2));
        }
        if remaining.starts_with(b"\r\n\r\n") {
            return Some((index, 4));
        }
    }
    None
}

fn emit_delta(emitter: &impl ChatDeltaEmitter, payload: ChatDeltaPayload) -> Result<(), String> {
    emitter.emit_chat_delta(payload)
}

async fn wait_for_cancel(cancel: Arc<AtomicBool>) {
    while !cancel.load(Ordering::SeqCst) {
        tokio::time::sleep(REQUEST_CANCEL_POLL).await;
    }
}

async fn savings_point(
    sidecar: &SidecarManager,
    project: &str,
    rtk_database_path: &Path,
) -> Option<SavingsPoint> {
    Some(SavingsPoint {
        project: sidecar.project_savings_totals(project).await.ok()?,
        rtk_tokens_saved: exec::rtk_tokens_saved_at(rtk_database_path).unwrap_or_default(),
    })
}

async fn restart_safe_savings_point(
    sidecar: &SidecarManager,
    project: &str,
    rtk_database_path: &Path,
) -> Option<SavingsPoint> {
    if let Some(point) = savings_point(sidecar, project, rtk_database_path).await {
        return Some(point);
    }

    // A dead proxy can leave the turn without a project snapshot. The managed
    // replacement retains the persistent project rollup.
    if !matches!(sidecar.ensure_ready().await, SidecarStatus::Running { .. }) {
        return None;
    }
    savings_point(sidecar, project, rtk_database_path).await
}

async fn settled_after_savings(
    sidecar: &SidecarManager,
    project: &str,
    rtk_database_path: &Path,
    before: &Option<SavingsPoint>,
    cancel: Arc<AtomicBool>,
) -> Option<SavingsPoint> {
    let started_at = Instant::now();
    let mut cancellation_started = cancel.load(Ordering::SeqCst).then(Instant::now);
    let mut retries_remaining = if cancellation_started.is_some() {
        SAVINGS_CANCEL_SETTLE_RETRIES
    } else {
        SAVINGS_SETTLE_MAX_RETRIES
    };
    let mut tracker = SavingsSettleTracker::default();
    let mut after = if cancellation_started.is_some() {
        savings_point(sidecar, project, rtk_database_path).await
    } else {
        restart_safe_savings_point(sidecar, project, rtk_database_path).await
    };
    let mut grace_started_at = cancellation_started.unwrap_or(started_at);
    let mut zero_grace = if cancellation_started.is_some() {
        SAVINGS_CANCEL_SETTLE_GRACE
    } else {
        SAVINGS_SETTLE_ZERO_GRACE
    };
    if tracker.observe(before, &after, grace_started_at.elapsed(), zero_grace) {
        return after;
    }

    while retries_remaining > 0 {
        let cancellation_began = if cancellation_started.is_some() {
            tokio::time::sleep(SAVINGS_SETTLE_DELAY).await;
            false
        } else {
            tokio::select! {
                _ = tokio::time::sleep(SAVINGS_SETTLE_DELAY) => false,
                _ = wait_for_cancel(cancel.clone()) => true,
            }
        };
        if cancellation_began {
            let now = Instant::now();
            cancellation_started = Some(now);
            grace_started_at = now;
            zero_grace = SAVINGS_CANCEL_SETTLE_GRACE;
            retries_remaining = SAVINGS_CANCEL_SETTLE_RETRIES;
        } else {
            retries_remaining -= 1;
        }

        after = if cancellation_started.is_some() {
            savings_point(sidecar, project, rtk_database_path).await
        } else {
            restart_safe_savings_point(sidecar, project, rtk_database_path).await
        };
        if tracker.observe(before, &after, grace_started_at.elapsed(), zero_grace) {
            break;
        }
        if cancellation_started.is_none() && cancel.load(Ordering::SeqCst) {
            let now = Instant::now();
            cancellation_started = Some(now);
            grace_started_at = now;
            zero_grace = SAVINGS_CANCEL_SETTLE_GRACE;
            retries_remaining = SAVINGS_CANCEL_SETTLE_RETRIES;
        }
    }
    after
}

#[derive(Debug, Default)]
struct SavingsSettleTracker {
    last: Option<SavingsPoint>,
    stable_samples: usize,
}

impl SavingsSettleTracker {
    fn observe(
        &mut self,
        before: &Option<SavingsPoint>,
        after: &Option<SavingsPoint>,
        elapsed: Duration,
        zero_grace: Duration,
    ) -> bool {
        if before.is_none() {
            return true;
        }

        let Some(after) = after else {
            self.last = None;
            self.stable_samples = 0;
            return false;
        };

        if self.last == Some(*after) {
            self.stable_samples += 1;
        } else {
            self.last = Some(*after);
            self.stable_samples = 1;
        }

        let before = before.expect("savings baseline should exist");
        let has_delta = after.project.tokens_saved > before.project.tokens_saved
            || after
                .project
                .savings_usd
                .zip(before.project.savings_usd)
                .is_some_and(|(after, before)| after > before)
            || (before.project.savings_usd.is_none()
                && after
                    .project
                    .savings_usd
                    .is_some_and(|savings| savings > 0.0))
            || after.rtk_tokens_saved > before.rtk_tokens_saved;
        self.stable_samples >= SAVINGS_SETTLE_STABLE_SAMPLES && (has_delta || elapsed >= zero_grace)
    }
}

fn compression_tokens_delta(before: SavingsPoint, after: SavingsPoint) -> u64 {
    after
        .project
        .tokens_saved
        .saturating_sub(before.project.tokens_saved)
}

fn rtk_tokens_delta(before: SavingsPoint, after: SavingsPoint) -> u64 {
    // The RTK database is owned by the Halv process, so a managed Headroom
    // restart does not reset it. A missing or replaced database is treated as
    // a counter reset and never creates negative savings.
    after
        .rtk_tokens_saved
        .saturating_sub(before.rtk_tokens_saved)
}

fn compression_savings_usd_delta(before: SavingsPoint, after: SavingsPoint) -> Option<f64> {
    after
        .project
        .savings_usd
        .map(|after| (after - before.project.savings_usd.unwrap_or_default()).max(0.0))
}

/// The Headroom and RTK tokens of one turn.
fn savings_split(
    project: &GuiHeadroomProject,
    before: Option<SavingsPoint>,
    after: Option<SavingsPoint>,
) -> Option<(u64, u64)> {
    if project.scope != GuiHeadroomProjectScope::Turn {
        return None;
    }
    let (before, after) = (before?, after?);
    Some((
        compression_tokens_delta(before, after),
        rtk_tokens_delta(before, after),
    ))
}

fn savings_delta_for_model(
    project: &GuiHeadroomProject,
    before: Option<SavingsPoint>,
    after: Option<SavingsPoint>,
    model: &str,
) -> (Option<i64>, Option<f64>, Option<PricingSource>) {
    if project.scope != GuiHeadroomProjectScope::Turn {
        return (None, None, None);
    }
    let (Some(before), Some(after)) = (before, after) else {
        return (None, None, None);
    };

    let rtk_tokens_saved = rtk_tokens_delta(before, after);
    let tokens_saved =
        compression_tokens_delta(before, after).saturating_add(rtk_tokens_saved) as i64;
    let (savings_usd, pricing_source) = match compression_savings_usd_delta(before, after) {
        Some(compression_savings_usd) => {
            let input_price = pricing::input_price(None, Some(model));
            let savings_usd = compression_savings_usd
                + input_price.savings_usd(agent_usage_i64(rtk_tokens_saved));
            let source = if rtk_tokens_saved > 0 {
                PricingSource::Headroom.combine(input_price.source)
            } else {
                PricingSource::Headroom
            };
            (Some(savings_usd), Some(source))
        }
        None => (None, None),
    };
    (Some(tokens_saved), savings_usd, pricing_source)
}

#[cfg(test)]
fn savings_delta(
    project: &GuiHeadroomProject,
    before: Option<SavingsPoint>,
    after: Option<SavingsPoint>,
) -> (Option<i64>, Option<f64>) {
    let savings = savings_delta_for_model(project, before, after, DEFAULT_MODEL);
    (savings.0, savings.1)
}

fn normalize_model(model: &str) -> String {
    let model = model.trim();
    if model.is_empty() {
        DEFAULT_MODEL.to_string()
    } else {
        model.to_string()
    }
}

fn normalize_effort(
    effort: &str,
    model_reasoning: Option<&ModelReasoningMetadata>,
) -> Result<String, String> {
    let effort = effort.trim();
    let effort = if effort.is_empty() {
        model_reasoning
            .map(|reasoning| reasoning.default_effort.as_str())
            .unwrap_or(DEFAULT_EFFORT)
    } else {
        effort
    };
    let is_supported = model_reasoning.map_or_else(
        || KNOWN_REASONING_EFFORTS.contains(&effort),
        |reasoning| {
            reasoning
                .efforts
                .iter()
                .any(|supported| supported == effort)
        },
    );
    if is_supported {
        Ok(effort.to_string())
    } else {
        let expected = model_reasoning
            .map(|reasoning| reasoning.efforts.join(", "))
            .unwrap_or_else(|| KNOWN_REASONING_EFFORTS.join(", "));
        Err(format!(
            "Unsupported reasoning effort `{effort}`; expected one of {}",
            expected
        ))
    }
}

fn uncached_fallback_models(chat: &ChatManager) -> Vec<ChatModel> {
    chat.clear_model_reasoning();
    fallback_models()
}

fn fallback_models() -> Vec<ChatModel> {
    FALLBACK_MODELS
        .iter()
        .map(|model| model_info(model))
        .collect()
}

fn fallback_claude_models() -> Vec<ChatModel> {
    FALLBACK_CLAUDE_MODELS
        .iter()
        .map(|model| claude_model_info(model, model))
        .collect()
}

fn parse_anthropic_model_registry(value: Value) -> Vec<ChatModel> {
    value
        .get("data")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|model| {
            let id = model.get("id").and_then(Value::as_str)?;
            id.starts_with("claude-").then(|| {
                let label = model
                    .get("display_name")
                    .and_then(Value::as_str)
                    .unwrap_or(id);
                claude_model_info(id, label)
            })
        })
        .collect()
}

fn claude_model_info(id: &str, label: &str) -> ChatModel {
    ChatModel {
        id: id.to_string(),
        label: label.to_string(),
        reasoning_efforts: CLAUDE_EFFORTS
            .iter()
            .map(|effort| effort.to_string())
            .collect(),
        default_effort: DEFAULT_CLAUDE_EFFORT.to_string(),
        provider: "anthropic".to_string(),
        supports_fast_mode: false,
        fast_service_tier: DEFAULT_FAST_SERVICE_TIER.to_string(),
    }
}

fn parse_model_registry(value: Value) -> Vec<ChatModel> {
    let models = value
        .get("models")
        .and_then(Value::as_array)
        .or_else(|| value.get("data").and_then(Value::as_array));
    let Some(models) = models else {
        return Vec::new();
    };
    let has_visibility_metadata = models.iter().any(model_has_visibility_metadata);

    models
        .iter()
        .filter_map(|model| {
            let id = model
                .get("slug")
                .or_else(|| model.get("id"))
                .and_then(Value::as_str)?;
            registry_model_is_visible(model, id, has_visibility_metadata)
                .then(|| registry_model_info(model, id))
        })
        .collect()
}

fn model_has_visibility_metadata(model: &Value) -> bool {
    model
        .get("visibility")
        .is_some_and(|visibility| visibility.is_string() || visibility.is_boolean())
        || model.get("visible").is_some_and(Value::is_boolean)
        || model.get("hidden").is_some_and(Value::is_boolean)
}

fn registry_model_is_visible(model: &Value, id: &str, has_visibility_metadata: bool) -> bool {
    if has_visibility_metadata {
        if let Some(visibility) = model.get("visibility") {
            if let Some(visibility) = visibility.as_str() {
                return !matches!(visibility.to_ascii_lowercase().as_str(), "hide" | "hidden");
            }
            if let Some(visible) = visibility.as_bool() {
                return visible;
            }
        }
        if let Some(visible) = model.get("visible").and_then(Value::as_bool) {
            return visible;
        }
        if let Some(hidden) = model.get("hidden").and_then(Value::as_bool) {
            return !hidden;
        }
    }

    !HIDDEN_MODEL_IDS.contains(&id)
}

fn registry_model_info(model: &Value, id: &str) -> ChatModel {
    let reasoning_efforts = model
        .get("supported_reasoning_levels")
        .and_then(Value::as_array)
        .map(|levels| {
            levels
                .iter()
                .filter_map(|level| {
                    level
                        .as_str()
                        .or_else(|| level.get("effort").and_then(Value::as_str))
                })
                .map(str::trim)
                .filter(|effort| !effort.is_empty())
                .map(ToString::to_string)
                .collect::<Vec<_>>()
        })
        .filter(|efforts| !efforts.is_empty())
        .unwrap_or_else(default_reasoning_efforts);
    let default_effort = model
        .get("default_reasoning_level")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|effort| !effort.is_empty())
        .unwrap_or(DEFAULT_EFFORT)
        .to_string();

    ChatModel {
        id: id.to_string(),
        label: id.to_string(),
        reasoning_efforts,
        default_effort,
        provider: "openai".to_string(),
        supports_fast_mode: registry_model_supports_fast_mode(model),
        fast_service_tier: registry_model_fast_service_tier(model),
    }
}

fn model_info(id: &str) -> ChatModel {
    ChatModel {
        id: id.to_string(),
        label: id.to_string(),
        reasoning_efforts: default_reasoning_efforts(),
        default_effort: DEFAULT_EFFORT.to_string(),
        provider: "openai".to_string(),
        supports_fast_mode: false,
        fast_service_tier: DEFAULT_FAST_SERVICE_TIER.to_string(),
    }
}

fn registry_model_supports_fast_mode(model: &Value) -> bool {
    model
        .get("service_tiers")
        .and_then(Value::as_array)
        .is_some_and(|tiers| {
            tiers.iter().any(|tier| {
                registry_service_tier_id(tier).is_some_and(|id| {
                    id.eq_ignore_ascii_case(DEFAULT_FAST_SERVICE_TIER)
                        || id.eq_ignore_ascii_case("fast")
                }) || registry_service_tier_name(tier)
                    .is_some_and(|name| name.eq_ignore_ascii_case("fast"))
            })
        })
        || model
            .get("additional_speed_tiers")
            .and_then(Value::as_array)
            .is_some_and(|tiers| {
                tiers.iter().any(|tier| {
                    registry_service_tier_id(tier).is_some_and(|id| id.eq_ignore_ascii_case("fast"))
                })
            })
}

fn registry_model_fast_service_tier(model: &Value) -> String {
    model
        .get("service_tiers")
        .and_then(Value::as_array)
        .and_then(|tiers| {
            tiers
                .iter()
                .find_map(|tier| {
                    registry_service_tier_id(tier)
                        .filter(|id| id.eq_ignore_ascii_case(DEFAULT_FAST_SERVICE_TIER))
                })
                .or_else(|| {
                    tiers.iter().find_map(|tier| {
                        registry_service_tier_name(tier)
                            .filter(|name| name.eq_ignore_ascii_case("fast"))
                            .and_then(|_| registry_service_tier_id(tier))
                    })
                })
        })
        .unwrap_or(DEFAULT_FAST_SERVICE_TIER)
        .to_string()
}

fn registry_service_tier_id(tier: &Value) -> Option<&str> {
    tier.as_str()
        .or_else(|| tier.get("id").and_then(Value::as_str))
        .map(str::trim)
        .filter(|id| !id.is_empty())
}

fn registry_service_tier_name(tier: &Value) -> Option<&str> {
    tier.get("name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|name| !name.is_empty())
}

fn default_reasoning_efforts() -> Vec<String> {
    REASONING_EFFORTS
        .iter()
        .map(|effort| effort.to_string())
        .collect()
}

fn user_agent() -> String {
    format!(
        "{ORIGINATOR}/{} ({}; {}) Halv",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    #[cfg(target_os = "macos")]
    use std::time::Instant;

    const SSE_FIXTURE: &str = include_str!("../../tests/fixtures/responses_sse.txt");

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
    fn null_conversation_backend_keeps_native_routing_unchanged() {
        assert_eq!(
            conversation_backend_route(None, None, "gpt-5.5", None)
                .expect("native route should resolve"),
            ConversationBackendRoute::Native,
        );
    }

    #[test]
    fn claude_session_backend_routes_to_the_requested_cli_alias() {
        assert_eq!(
            conversation_backend_route(
                Some("claude_session"),
                Some("sonnet"),
                "opus",
                Some("claude_session"),
            )
            .expect("Claude route should resolve"),
            ConversationBackendRoute::ClaudeSession {
                model: "opus".to_string(),
            },
        );
        assert_eq!(
            conversation_backend_route(Some("claude_session"), Some("sonnet"), "gpt-5.5", None,)
                .expect("native switch should resolve"),
            ConversationBackendRoute::Native,
        );
    }

    #[test]
    fn claude_session_backend_routes_to_a_full_resolved_model_id() {
        assert_eq!(
            conversation_backend_route(
                Some("claude_session"),
                Some("fable"),
                "claude-fable-5-20260801",
                Some("claude_session"),
            )
            .expect("resolved Claude route should resolve"),
            ConversationBackendRoute::ClaudeSession {
                model: "claude-fable-5-20260801".to_string(),
            },
        );
    }

    #[test]
    fn bare_model_id_uses_the_last_path_segment() {
        assert_eq!(bare_model_id("gpt-5.6-sol"), "gpt-5.6-sol");
        assert_eq!(bare_model_id("a/b/c"), "c");
    }

    #[test]
    fn agent_backends_accept_bare_and_prefixed_models() {
        assert!(ConversationAgentBackend::Codex.accepts_model("codex-cli/gpt-5.6-sol"));
        assert!(ConversationAgentBackend::Codex.accepts_model("gpt-5.6-sol"));
        assert!(!ConversationAgentBackend::Codex.accepts_model("codex-cli/claude-opus-5"));
        assert!(!ConversationAgentBackend::Codex.accepts_model("not-a-model"));
        assert!(ConversationAgentBackend::Kimi.accepts_model("kimi-cli/kimi-for-coding"));
    }

    #[test]
    fn codex_backend_route_returns_the_bare_launch_model() {
        assert_eq!(
            conversation_backend_route(None, None, "codex-cli/gpt-5.6-sol", Some("codex_session"),)
                .expect("prefixed Codex route should resolve"),
            ConversationBackendRoute::AgentSession {
                backend: ConversationAgentBackend::Codex,
                model: "gpt-5.6-sol".to_string(),
            },
        );
    }

    #[test]
    fn additional_agent_backends_route_and_switch_back_to_native() {
        let cases = [
            ("codex_session", "gpt-5.5", ConversationAgentBackend::Codex),
            (
                "kimi_session",
                "kimi-for-coding",
                ConversationAgentBackend::Kimi,
            ),
            ("zai_session", "sonnet", ConversationAgentBackend::Zai),
            ("grok_session", "opus", ConversationAgentBackend::Grok),
        ];
        for (backend_name, model, backend) in cases {
            assert_eq!(
                conversation_backend_route(None, None, model, Some(backend_name))
                    .expect("agent route should resolve"),
                ConversationBackendRoute::AgentSession {
                    backend,
                    model: model.to_string(),
                },
            );
            assert_eq!(
                conversation_backend_route(Some(backend_name), Some(model), model, None)
                    .expect("native switch should resolve"),
                ConversationBackendRoute::Native,
            );
        }
        assert!(conversation_backend_route(
            Some("codex_session"),
            Some("gpt-5.5"),
            "kimi-for-coding",
            Some("codex_session"),
        )
        .expect_err("an explicit invalid model must not fall back")
        .contains("unsupported Codex CLI model"));
    }

    #[test]
    fn grok_backend_requires_and_selects_an_active_xai_profile() {
        let path = std::env::temp_dir().join(format!(
            "halv-chat-grok-profile-{}.sqlite3",
            storage::new_uuid_v4()
        ));
        let storage = Storage::open(&path).expect("test storage should open");
        let conversation_id = storage
            .ensure_conversation(None, "Grok route", "opus", None)
            .expect("conversation should create");

        let missing =
            conversation_agent_lane(&storage, &conversation_id, ConversationAgentBackend::Grok)
                .expect_err("Grok should require an xAI profile");
        assert_eq!(missing, "Add an active Grok (xAI) profile before sending.");

        let profile = storage::CredentialProfile {
            id: "xai-profile".to_string(),
            provider: "xai".to_string(),
            name: "Grok team key".to_string(),
            kind: "api_key".to_string(),
            priority: 0,
            status: "active".to_string(),
            last_validated_at: None,
            metadata_json: None,
            created_at: "2026-08-20T12:00:00Z".to_string(),
            updated_at: "2026-08-20T12:00:00Z".to_string(),
        };
        storage
            .insert_credential_profile(&profile)
            .expect("xAI profile should insert");
        let (agent_kind, lane) =
            conversation_agent_lane(&storage, &conversation_id, ConversationAgentBackend::Grok)
                .expect("Grok lane should resolve");
        assert_eq!(agent_kind, AgentKind::Grok);
        assert!(matches!(
            lane,
            SessionLane::GrokApiKey {
                credential: SessionCredential::CredentialProfile {
                    credential_profile_id
                },
                model_overrides: None,
            } if credential_profile_id == profile.id
        ));
        assert_eq!(
            storage
                .conversation_credential_profile(&conversation_id)
                .expect("profile pin should read")
                .as_deref(),
            Some(profile.id.as_str())
        );

        drop(storage);
        let _ = fs::remove_file(path);
    }

    #[derive(Clone, Default)]
    struct RecordingEmitter {
        payloads: Arc<Mutex<Vec<ChatDeltaPayload>>>,
    }

    impl ChatDeltaEmitter for RecordingEmitter {
        fn emit_chat_delta(&self, payload: ChatDeltaPayload) -> Result<(), String> {
            self.payloads
                .lock()
                .expect("recording emitter lock should not be poisoned")
                .push(payload);
            Ok(())
        }
    }

    struct TestPreferences {
        manager: PreferencesManager,
        directory: PathBuf,
    }

    impl TestPreferences {
        fn new(name: &str) -> Self {
            let directory = std::env::temp_dir().join(format!(
                "halv-chat-preferences-{name}-{}",
                storage::new_uuid_v4()
            ));
            let manager =
                PreferencesManager::open(&directory).expect("test preferences should open");
            Self { manager, directory }
        }
    }

    impl std::ops::Deref for TestPreferences {
        type Target = PreferencesManager;

        fn deref(&self) -> &Self::Target {
            &self.manager
        }
    }

    impl Drop for TestPreferences {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.directory);
        }
    }

    #[test]
    fn crux_savings_meter_caps_each_message() {
        let mut meter = CruxSavingsMeter::default();
        meter.record("call-1", 15_000);
        meter.record("call-2", 6_000);
        assert_eq!(meter.tokens_saved, CRUX_SAVINGS_PER_MESSAGE_CAP);
    }

    #[test]
    fn crux_savings_meter_records_each_call_once() {
        let mut meter = CruxSavingsMeter::default();
        meter.record("call-1", 5_200);
        meter.record("call-1", 5_200);
        meter.record("call-2", 900);
        assert_eq!(meter.tokens_saved, 6_100);
    }

    #[test]
    fn shell_tool_result_reports_the_actual_rtk_route() {
        let emitter = RecordingEmitter::default();
        emit_shell_tool_result(
            &emitter,
            "conversation-1",
            "call-1",
            "completed",
            Some(0),
            Some(25),
            false,
            Some(exec::RtkRouteKind::Rewritten),
        )
        .expect("shell result should emit");

        let payloads = emitter.payloads.lock().unwrap();
        assert_eq!(payloads.len(), 1);
        assert_eq!(payloads[0].kind, ChatDeltaKind::ToolResult);
        assert_eq!(payloads[0].rtk_route.as_deref(), Some("rewritten"));
    }

    fn chatgpt_bearer() -> AuthBearer {
        AuthBearer {
            token: "chatgpt-token".to_string(),
            source: "app".to_string(),
            account_id: Some("acct_123".to_string()),
            is_api_key: false,
        }
    }

    fn api_key_bearer() -> AuthBearer {
        AuthBearer {
            token: "sk-test".to_string(),
            source: "api-key".to_string(),
            account_id: None,
            is_api_key: true,
        }
    }

    #[test]
    fn parses_model_registry_efforts_defaults_and_visibility() {
        let models = parse_model_registry(json!({
            "models": [
                {
                    "slug": "gpt-5.6-sol",
                    "supported_reasoning_levels": [
                        {"effort": "low", "description": "Fast"},
                        {"effort": "medium", "description": "Balanced"},
                        {"effort": "max", "description": "Maximum"},
                        {"effort": "ultra", "description": "Delegated"}
                    ],
                    "default_reasoning_level": "low",
                    "visibility": "list"
                },
                {
                    "slug": "future-codex",
                    "supported_reasoning_levels": ["medium", "ultra"],
                    "default_reasoning_level": "medium",
                    "visibility": "list"
                },
                {
                    "slug": "codex-auto-review",
                    "supported_reasoning_levels": [
                        {"effort": "medium", "description": "Balanced"}
                    ],
                    "default_reasoning_level": "medium",
                    "visibility": "hide"
                }
            ]
        }));

        assert_eq!(
            models
                .iter()
                .map(|model| model.id.as_str())
                .collect::<Vec<_>>(),
            vec!["gpt-5.6-sol", "future-codex"]
        );
        assert_eq!(
            models[0].reasoning_efforts,
            vec!["low", "medium", "max", "ultra"]
        );
        assert_eq!(models[0].default_effort, "low");
        assert_eq!(models[1].reasoning_efforts, vec!["medium", "ultra"]);
        assert_eq!(models[1].default_effort, "medium");
    }

    #[test]
    fn parses_fast_support_and_service_tier_from_catalog_metadata() {
        let models = parse_model_registry(json!({
            "models": [
                {
                    "slug": "service-tier-model",
                    "service_tiers": [
                        {"id": "standard", "name": "Standard"},
                        {"id": "priority", "name": "Fast"}
                    ]
                },
                {
                    "slug": "legacy-speed-model",
                    "additional_speed_tiers": ["fast"]
                },
                {
                    "slug": "legacy-object-model",
                    "additional_speed_tiers": [{"id": "FAST"}]
                },
                {
                    "slug": "standard-only-model",
                    "service_tiers": [{"id": "standard"}]
                },
                {
                    "slug": "metadata-absent-model"
                }
            ]
        }));

        assert!(models[0].supports_fast_mode);
        assert_eq!(models[0].fast_service_tier, "priority");
        assert!(models[1].supports_fast_mode);
        assert_eq!(models[1].fast_service_tier, "priority");
        assert!(models[2].supports_fast_mode);
        assert_eq!(models[2].fast_service_tier, "priority");
        assert!(!models[3].supports_fast_mode);
        assert!(!models[4].supports_fast_mode);
    }

    #[test]
    fn fast_service_tier_request_matrix_requires_flag_subscription_and_model_support() {
        for fast_mode_enabled in [false, true] {
            for model_supports_fast_mode in [false, true] {
                for bearer in [chatgpt_bearer(), api_key_bearer()] {
                    let mut body = build_responses_request_body(
                        "conversation-1",
                        &[],
                        DEFAULT_MODEL,
                        DEFAULT_EFFORT,
                        None,
                        None,
                        None,
                        false,
                        false,
                        exec::ApprovalPolicy::ReadOnly,
                    );
                    configure_fast_service_tier(
                        &mut body,
                        fast_mode_enabled,
                        &bearer,
                        model_supports_fast_mode,
                        DEFAULT_FAST_SERVICE_TIER,
                    );

                    let expected = fast_mode_enabled
                        && model_supports_fast_mode
                        && bearer_is_chatgpt_subscription(&bearer);
                    assert_eq!(
                        body.get("service_tier").and_then(Value::as_str),
                        expected.then_some(DEFAULT_FAST_SERVICE_TIER),
                        "flag={fast_mode_enabled} support={model_supports_fast_mode} api_key={}",
                        bearer.is_api_key
                    );
                }
            }
        }
    }

    #[test]
    fn service_tier_http_400_falls_back_once_before_any_stream_progress() {
        let mut body = json!({
            "model": DEFAULT_MODEL,
            "service_tier": "priority"
        });
        let error = upstream_http_chat_error(
            StatusCode::BAD_REQUEST,
            r#"{"detail":"Unsupported service_tier: fast"}"#,
            String::new(),
        );
        let progress = StreamAttemptProgress::default();

        assert!(error.retryable);
        assert_eq!(error.message, "Unsupported service_tier: fast (HTTP 400)");
        assert!(should_fallback_from_fast_mode(
            &body, &error, false, progress
        ));
        assert!(should_fallback_from_fast_mode(
            &body,
            &ChatError::new("Bad request (HTTP 400)"),
            false,
            progress
        ));
        body.as_object_mut().unwrap().remove("service_tier");
        assert!(!should_fallback_from_fast_mode(
            &body, &error, false, progress
        ));
        assert!(!should_fallback_from_fast_mode(
            &json!({"service_tier": "priority"}),
            &error,
            true,
            progress
        ));
        assert!(!should_fallback_from_fast_mode(
            &json!({"service_tier": "priority"}),
            &ChatError::new("Unsupported service_tier: fast (HTTP 500)"),
            false,
            progress
        ));
    }

    #[test]
    fn model_registry_without_visibility_uses_broad_known_hidden_filter() {
        let models = parse_model_registry(json!({
            "models": [
                {"slug": "future-codex"},
                {"slug": "codex-auto-review"},
                {"slug": "gpt-5.6"}
            ]
        }));

        assert_eq!(
            models
                .iter()
                .map(|model| model.id.as_str())
                .collect::<Vec<_>>(),
            vec!["future-codex", "gpt-5.6"]
        );
        assert_eq!(models[0].reasoning_efforts, default_reasoning_efforts());
        assert_eq!(models[0].default_effort, DEFAULT_EFFORT);
    }

    #[test]
    fn fallback_models_start_with_the_gpt_6_family() {
        assert_eq!(
            fallback_models()
                .iter()
                .map(|model| model.id.as_str())
                .collect::<Vec<_>>(),
            vec![
                "gpt-6-astra",
                "gpt-6-sol",
                "gpt-6-luna",
                "gpt-5.6-sol",
                "gpt-5.6-terra",
                "gpt-5.6-luna",
                "gpt-5.6",
                "gpt-5.5",
                "gpt-5.4",
                "gpt-5.4-mini",
                "gpt-5.3-codex",
                "gpt-5.2"
            ]
        );
    }

    #[test]
    fn effort_validation_uses_registry_metadata_then_known_superset() {
        let models = parse_model_registry(json!({
            "models": [{
                "slug": "gpt-5.6-sol",
                "supported_reasoning_levels": [
                    {"effort": "low"},
                    {"effort": "ultra"}
                ],
                "default_reasoning_level": "low",
                "visibility": "list"
            }]
        }));
        let chat = ChatManager::new();
        chat.cache_model_reasoning(&models);
        let model_reasoning = chat
            .reasoning_for_model("gpt-5.6-sol")
            .expect("registry reasoning metadata should be cached");

        assert_eq!(
            normalize_effort("ultra", Some(&model_reasoning)),
            Ok("ultra".to_string())
        );
        assert_eq!(
            normalize_effort("", Some(&model_reasoning)),
            Ok("low".to_string())
        );
        assert!(normalize_effort("garbage", Some(&model_reasoning)).is_err());
        assert!(normalize_effort("xhigh", Some(&model_reasoning)).is_err());
        assert_eq!(normalize_effort("ultra", None), Ok("ultra".to_string()));
        assert!(normalize_effort("garbage", None).is_err());
        assert_eq!(reasoning_effort_for_request("ultra"), "max");
        assert_eq!(reasoning_effort_for_request("xhigh"), "xhigh");
    }

    #[test]
    fn conversation_runtime_resolves_approval_policy_precedence() {
        let root =
            std::env::temp_dir().join(format!("halv-runtime-approval-{}", storage::new_uuid_v4()));
        std::fs::create_dir_all(&root).expect("project root should create");
        let root = root
            .canonicalize()
            .expect("project root should canonicalize");
        let db_path = std::env::temp_dir().join(format!(
            "halv-runtime-approval-{}.sqlite",
            storage::new_uuid_v4()
        ));
        let storage = Storage::open(&db_path).expect("storage should open");
        let preferences = TestPreferences::new("runtime-approval");
        preferences
            .set_conversation_defaults(
                DEFAULT_MODEL.to_string(),
                DEFAULT_EFFORT.to_string(),
                true,
                Some(exec::ApprovalPolicy::Never),
                false,
            )
            .expect("global approval default should save");
        let project = storage
            .upsert_project("Runtime approval", &root.to_string_lossy())
            .expect("project should insert");
        let conversation_id = storage
            .ensure_conversation(None, "Run tools", DEFAULT_MODEL, Some(project.id))
            .expect("conversation should insert");

        assert_eq!(
            load_conversation_runtime(&storage, &preferences, &conversation_id)
                .expect("unknown trust policy should load")
                .effective_policy,
            exec::ApprovalPolicy::Ask
        );

        storage
            .set_project_trust(project.id, "untrusted")
            .expect("project should become untrusted");
        assert_eq!(
            load_conversation_runtime(&storage, &preferences, &conversation_id)
                .expect("untrusted policy should load")
                .effective_policy,
            exec::ApprovalPolicy::Ask
        );

        storage
            .set_project_trust(project.id, "trusted")
            .expect("project should become trusted");
        assert_eq!(
            load_conversation_runtime(&storage, &preferences, &conversation_id)
                .expect("trusted global policy should load")
                .effective_policy,
            exec::ApprovalPolicy::Never
        );

        storage
            .set_project_trust(project.id, "untrusted")
            .expect("project should become untrusted again");
        storage
            .set_project_approval_policy(project.id, "on_request")
            .expect("project policy should update");
        assert_eq!(
            load_conversation_runtime(&storage, &preferences, &conversation_id)
                .expect("project policy should load")
                .effective_policy,
            exec::ApprovalPolicy::OnRequest
        );

        storage
            .set_conversation_approval_policy(&conversation_id, Some("ask"))
            .expect("conversation policy should update");
        assert_eq!(
            load_conversation_runtime(&storage, &preferences, &conversation_id)
                .expect("conversation policy should load")
                .effective_policy,
            exec::ApprovalPolicy::Ask
        );

        let projectless_id = storage
            .ensure_conversation(None, "No project", DEFAULT_MODEL, None)
            .expect("projectless conversation should insert");
        assert_eq!(
            load_conversation_runtime(&storage, &preferences, &projectless_id)
                .expect("projectless policy should load")
                .effective_policy,
            exec::ApprovalPolicy::ReadOnly
        );

        drop(storage);
        let _ = std::fs::remove_dir_all(root);
        let _ = std::fs::remove_file(db_path);
    }

    #[test]
    fn live_turn_permissions_follow_policy_and_agent_mode_revocations() {
        let root = std::env::temp_dir().join(format!(
            "halv-live-turn-permissions-{}",
            storage::new_uuid_v4()
        ));
        std::fs::create_dir_all(&root).expect("project root should create");
        let root = root
            .canonicalize()
            .expect("project root should canonicalize");
        let root_string = root.to_string_lossy().to_string();
        let db_path = std::env::temp_dir().join(format!(
            "halv-live-turn-permissions-{}.sqlite",
            storage::new_uuid_v4()
        ));
        let storage = Storage::open(&db_path).expect("storage should open");
        let preferences = TestPreferences::new("live-turn-permissions");
        let project = storage
            .upsert_project("Live permissions", &root_string)
            .expect("project should insert");
        storage
            .set_project_approval_policy(project.id, "skip_all")
            .expect("project policy should update");
        let conversation_id = storage
            .ensure_conversation(None, "Run tools", DEFAULT_MODEL, Some(project.id))
            .expect("conversation should insert");
        storage
            .set_conversation_agent_mode(&conversation_id, true)
            .expect("agent mode should enable");

        assert_eq!(
            load_live_turn_permissions(
                &storage,
                &preferences,
                &conversation_id,
                Some(&root_string),
            )
            .expect("live permissions should load"),
            LiveTurnPermissions {
                agent_mode_enabled: true,
                effective_policy: exec::ApprovalPolicy::SkipAll,
            }
        );
        assert!(live_tool_permissions_match(
            &storage,
            &preferences,
            &conversation_id,
            &root_string,
            exec::ApprovalPolicy::SkipAll,
            ToolInvocation::Model,
        )
        .expect("unchanged tool permissions should match"));

        storage
            .set_conversation_approval_policy(&conversation_id, Some("read_only"))
            .expect("conversation policy should revoke writes");
        assert_eq!(
            load_live_turn_permissions(
                &storage,
                &preferences,
                &conversation_id,
                Some(&root_string),
            )
            .expect("revoked permissions should load")
            .effective_policy,
            exec::ApprovalPolicy::ReadOnly
        );
        assert!(!live_tool_permissions_match(
            &storage,
            &preferences,
            &conversation_id,
            &root_string,
            exec::ApprovalPolicy::SkipAll,
            ToolInvocation::Model,
        )
        .expect("changed tool permissions should not match"));

        storage
            .set_conversation_agent_mode(&conversation_id, false)
            .expect("agent mode should disable");
        assert!(
            !load_live_turn_permissions(
                &storage,
                &preferences,
                &conversation_id,
                Some(&root_string),
            )
            .expect("disabled permissions should load")
            .agent_mode_enabled
        );
        assert!(!live_tool_permissions_match(
            &storage,
            &preferences,
            &conversation_id,
            &root_string,
            exec::ApprovalPolicy::ReadOnly,
            ToolInvocation::Model,
        )
        .expect("disabled agent mode should invalidate tool permissions"));
        assert!(live_tool_permissions_match(
            &storage,
            &preferences,
            &conversation_id,
            &root_string,
            exec::ApprovalPolicy::ReadOnly,
            ToolInvocation::DirectUser,
        )
        .expect("an explicit shell command should not require agent mode"));

        drop(storage);
        let _ = std::fs::remove_dir_all(root);
        let _ = std::fs::remove_file(db_path);
    }

    #[test]
    fn direct_shell_message_uses_a_fence_longer_than_command_output() {
        let message = direct_shell_message("before\n```embedded```\nafter\n");

        assert!(message.starts_with("````text\n"));
        assert!(message.ends_with("\n````"));
        assert!(message.contains("```embedded```"));
    }

    #[test]
    fn direct_shell_registration_never_persists_an_orphaned_user_turn() {
        let db_path = std::env::temp_dir().join(format!(
            "halv-direct-shell-registration-{}.sqlite",
            storage::new_uuid_v4()
        ));
        let storage = Storage::open(&db_path).expect("storage should open");
        let conversation_id = storage
            .ensure_conversation(None, "!pwd", DEFAULT_MODEL, None)
            .expect("conversation should insert");
        let chat = ChatManager::new();
        let running = chat
            .try_register_request(&conversation_id, Some("already-running"))
            .expect("first request should register");

        let error =
            register_direct_shell_request(&chat, &storage, &conversation_id, "!pwd", DEFAULT_MODEL)
                .expect_err("a second direct shell turn must be rejected");
        assert_eq!(
            error,
            "A chat request is already in flight for this conversation"
        );
        assert!(storage
            .get_messages(&conversation_id)
            .expect("messages should load")
            .is_empty());

        chat.clear_request(&conversation_id, &running);
        let (turn_id, request) =
            register_direct_shell_request(&chat, &storage, &conversation_id, "!pwd", DEFAULT_MODEL)
                .expect("direct shell request should register and persist");
        assert_eq!(request.turn_id().as_deref(), Some(turn_id.as_str()));
        assert_eq!(
            storage
                .get_messages(&conversation_id)
                .expect("messages should load")
                .iter()
                .map(|message| (message.role.as_str(), message.content.as_str()))
                .collect::<Vec<_>>(),
            vec![("user", "!pwd")]
        );
        chat.clear_request(&conversation_id, &request);

        let missing_conversation = storage::new_uuid_v4();
        assert!(register_direct_shell_request(
            &chat,
            &storage,
            &missing_conversation,
            "!pwd",
            DEFAULT_MODEL,
        )
        .is_err());
        let replacement = chat
            .try_register_request(&missing_conversation, Some("replacement"))
            .expect("failed user persistence must release in-flight registration");
        chat.clear_request(&missing_conversation, &replacement);

        drop(storage);
        let _ = std::fs::remove_file(db_path);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn direct_shell_honors_never_policy_without_requiring_agent_mode() {
        let root =
            std::env::temp_dir().join(format!("halv-direct-never-{}", storage::new_uuid_v4()));
        std::fs::create_dir_all(&root).expect("project root should create");
        let root = root
            .canonicalize()
            .expect("project root should canonicalize");
        let marker = root.join("must-not-exist");
        let db_path = std::env::temp_dir().join(format!(
            "halv-direct-never-{}.sqlite",
            storage::new_uuid_v4()
        ));
        let storage = Storage::open(&db_path).expect("storage should open");
        let preferences = TestPreferences::new("direct-never");
        let project = storage
            .upsert_project("Direct never", &root.to_string_lossy())
            .expect("project should insert");
        storage
            .set_project_approval_policy(project.id, "never")
            .expect("project policy should update");
        let conversation_id = storage
            .ensure_conversation(None, "!touch", DEFAULT_MODEL, Some(project.id))
            .expect("conversation should insert");
        assert!(!storage
            .conversation_agent_mode(&conversation_id)
            .expect("agent mode should load"));
        let chat = ChatManager::new();
        let user_content = format!("!printf blocked > {}", marker.display());
        let (turn_id, request) = register_direct_shell_request(
            &chat,
            &storage,
            &conversation_id,
            &user_content,
            DEFAULT_MODEL,
        )
        .expect("direct shell request should register");
        let emitter = RecordingEmitter::default();

        let response = run_registered_shell_turn(
            &emitter,
            &chat,
            &storage,
            &preferences,
            &conversation_id,
            &turn_id,
            &root.to_string_lossy(),
            exec::ApprovalPolicy::Never,
            &user_content,
            format!("printf blocked > {}", marker.display()),
            DEFAULT_MODEL.to_string(),
            request,
        )
        .await
        .expect("policy denial should be returned as a completed shell result");

        assert!(!marker.exists());
        assert!(response
            .assistant_message
            .contains("Command denied by user approval policy"));
        let payloads = emitter.payloads.lock().unwrap();
        assert!(payloads.iter().any(|payload| {
            payload.kind == ChatDeltaKind::ToolResult && payload.status.as_deref() == Some("denied")
        }));
        assert_eq!(
            payloads.last().map(|payload| &payload.kind),
            Some(&ChatDeltaKind::Done)
        );
        assert!(!payloads.iter().any(|payload| {
            payload.kind == ChatDeltaKind::ToolCall
                && payload.status.as_deref() == Some("awaiting_approval")
        }));
        drop(payloads);
        assert!(chat
            .pending_tool_decisions
            .lock()
            .expect("tool decision lock should not be poisoned")
            .is_empty());

        drop(storage);
        let _ = std::fs::remove_dir_all(root);
        let _ = std::fs::remove_file(db_path);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn direct_shell_cancellation_while_awaiting_approval_clears_the_decision() {
        let root = std::env::temp_dir().join(format!(
            "halv-direct-approval-cancel-{}",
            storage::new_uuid_v4()
        ));
        std::fs::create_dir_all(&root).expect("project root should create");
        let root = root
            .canonicalize()
            .expect("project root should canonicalize");
        let marker = root.join("must-not-exist");
        let db_path = std::env::temp_dir().join(format!(
            "halv-direct-approval-cancel-{}.sqlite",
            storage::new_uuid_v4()
        ));
        let storage = Storage::open(&db_path).expect("storage should open");
        let preferences = TestPreferences::new("direct-approval-cancel");
        let project = storage
            .upsert_project("Direct approval cancel", &root.to_string_lossy())
            .expect("project should insert");
        storage
            .set_project_approval_policy(project.id, "ask")
            .expect("project policy should update");
        let conversation_id = storage
            .ensure_conversation(None, "!write", DEFAULT_MODEL, Some(project.id))
            .expect("conversation should insert");
        let chat = ChatManager::new();
        let user_content = format!("!printf blocked > {}", marker.display());
        let (turn_id, request) = register_direct_shell_request(
            &chat,
            &storage,
            &conversation_id,
            &user_content,
            DEFAULT_MODEL,
        )
        .expect("direct shell request should register");
        let emitter = RecordingEmitter::default();
        let root_string = root.to_string_lossy().to_string();
        let command = format!("printf blocked > {}", marker.display());

        let turn = run_registered_shell_turn(
            &emitter,
            &chat,
            &storage,
            &preferences,
            &conversation_id,
            &turn_id,
            &root_string,
            exec::ApprovalPolicy::Ask,
            &user_content,
            command,
            DEFAULT_MODEL.to_string(),
            request,
        );
        let cancel = async {
            for _ in 0..100 {
                if !chat
                    .pending_tool_decisions
                    .lock()
                    .expect("tool decision lock should not be poisoned")
                    .is_empty()
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            assert!(!chat
                .pending_tool_decisions
                .lock()
                .expect("tool decision lock should not be poisoned")
                .is_empty());
            assert!(chat.cancel(&conversation_id).is_some());
        };
        let (result, ()) = tokio::join!(turn, cancel);

        assert_eq!(result.unwrap_err(), "Chat request cancelled");
        assert!(!marker.exists());
        assert!(chat
            .pending_tool_decisions
            .lock()
            .expect("tool decision lock should not be poisoned")
            .is_empty());
        let payloads = emitter.payloads.lock().unwrap();
        assert!(payloads.iter().any(|payload| {
            payload.kind == ChatDeltaKind::ToolCall
                && payload.status.as_deref() == Some("awaiting_approval")
        }));
        assert!(payloads.iter().any(|payload| {
            payload.kind == ChatDeltaKind::ToolResult
                && payload.status.as_deref() == Some("cancelled")
        }));
        assert_eq!(
            payloads.last().map(|payload| &payload.kind),
            Some(&ChatDeltaKind::Error)
        );
        drop(payloads);
        let next = chat
            .try_register_request(&conversation_id, Some("next-turn"))
            .expect("approval cancellation must release the conversation");
        chat.clear_request(&conversation_id, &next);

        drop(storage);
        let _ = std::fs::remove_dir_all(root);
        let _ = std::fs::remove_file(db_path);
    }

    #[cfg(target_os = "macos")]
    #[tokio::test(flavor = "current_thread")]
    async fn direct_shell_reuses_rtk_executor_and_persists_terminal_success() {
        let root = std::env::temp_dir().join(format!("halv-direct-rtk-{}", storage::new_uuid_v4()));
        init_git_repo(&root);
        let root = root
            .canonicalize()
            .expect("project root should canonicalize");
        let db_path =
            std::env::temp_dir().join(format!("halv-direct-rtk-{}.sqlite", storage::new_uuid_v4()));
        let storage = Storage::open(&db_path).expect("storage should open");
        let preferences = TestPreferences::new("direct-rtk");
        let project = storage
            .upsert_project("Direct RTK", &root.to_string_lossy())
            .expect("project should insert");
        storage
            .set_project_approval_policy(project.id, "read_only")
            .expect("project policy should update");
        let conversation_id = storage
            .ensure_conversation(None, "!git status --short", DEFAULT_MODEL, Some(project.id))
            .expect("conversation should insert");
        let chat = ChatManager::new();
        let (turn_id, request) = register_direct_shell_request(
            &chat,
            &storage,
            &conversation_id,
            "!git status --short",
            DEFAULT_MODEL,
        )
        .expect("direct shell request should register");
        let before_rtk = direct_shell_rtk_command_count();
        let emitter = RecordingEmitter::default();

        let response = run_registered_shell_turn(
            &emitter,
            &chat,
            &storage,
            &preferences,
            &conversation_id,
            &turn_id,
            &root.to_string_lossy(),
            exec::ApprovalPolicy::ReadOnly,
            "!git status --short",
            "git status --short".to_string(),
            DEFAULT_MODEL.to_string(),
            request,
        )
        .await
        .expect("direct shell command should complete");

        if response.assistant_message.contains("Exit code: 71")
            && response
                .assistant_message
                .contains("sandbox_apply: Operation not permitted")
        {
            eprintln!(
                "sandbox-exec is blocked by the outer test environment; direct-shell RTK accounting assertion skipped"
            );
            drop(storage);
            let _ = std::fs::remove_dir_all(root);
            let _ = std::fs::remove_file(db_path);
            return;
        }

        let after_rtk = direct_shell_rtk_command_count();
        assert!(
            after_rtk > before_rtk,
            "RTK command count did not advance (before={before_rtk}, after={after_rtk}); response={:?}",
            response.assistant_message
        );
        let payloads = emitter.payloads.lock().unwrap();
        let running = payloads
            .iter()
            .position(|payload| {
                payload.kind == ChatDeltaKind::ToolCall
                    && payload.status.as_deref() == Some("running")
            })
            .expect("running tool event should be emitted");
        let completed = payloads
            .iter()
            .position(|payload| {
                payload.kind == ChatDeltaKind::ToolResult
                    && payload.status.as_deref() == Some("completed")
                    && payload.exit_code == Some(0)
            })
            .expect("completed tool event should be emitted");
        let done = payloads
            .iter()
            .position(|payload| payload.kind == ChatDeltaKind::Done)
            .expect("done event should be emitted");
        assert!(running < completed && completed < done);
        assert_eq!(done, payloads.len() - 1);
        drop(payloads);

        let messages = storage
            .get_messages(&conversation_id)
            .expect("messages should load");
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].role, "user");
        assert_eq!(messages[0].content, "!git status --short");
        assert_eq!(messages[1].role, "assistant");
        assert!(!messages[1].error);
        assert_eq!(messages[1].content, response.assistant_message);
        assert!(messages[1].content.starts_with("```text\nExit code: 0"));
        let next = chat
            .try_register_request(&conversation_id, Some("next-turn"))
            .expect("terminal success must release the conversation");
        chat.clear_request(&conversation_id, &next);

        drop(storage);
        let _ = std::fs::remove_dir_all(root);
        let _ = std::fs::remove_file(db_path);
    }

    #[cfg(target_os = "macos")]
    #[tokio::test(flavor = "current_thread")]
    async fn direct_shell_cancellation_cleans_up_and_persists_one_terminal_error() {
        let root =
            std::env::temp_dir().join(format!("halv-direct-cancel-{}", storage::new_uuid_v4()));
        init_git_repo(&root);
        let root = root
            .canonicalize()
            .expect("project root should canonicalize");
        let db_path = std::env::temp_dir().join(format!(
            "halv-direct-cancel-{}.sqlite",
            storage::new_uuid_v4()
        ));
        let storage = Storage::open(&db_path).expect("storage should open");
        let preferences = TestPreferences::new("direct-cancel");
        let project = storage
            .upsert_project("Direct cancel", &root.to_string_lossy())
            .expect("project should insert");
        storage
            .set_project_approval_policy(project.id, "skip_all")
            .expect("project policy should update");
        let conversation_id = storage
            .ensure_conversation(None, "!sleep 5", DEFAULT_MODEL, Some(project.id))
            .expect("conversation should insert");
        let chat = ChatManager::new();
        let (turn_id, request) = register_direct_shell_request(
            &chat,
            &storage,
            &conversation_id,
            "!sleep 5",
            DEFAULT_MODEL,
        )
        .expect("direct shell request should register");
        let emitter = RecordingEmitter::default();
        let started = Instant::now();
        let root_string = root.to_string_lossy().to_string();

        let turn = run_registered_shell_turn(
            &emitter,
            &chat,
            &storage,
            &preferences,
            &conversation_id,
            &turn_id,
            &root_string,
            exec::ApprovalPolicy::SkipAll,
            "!sleep 5",
            "sleep 5".to_string(),
            DEFAULT_MODEL.to_string(),
            request,
        );
        let cancel = async {
            tokio::time::sleep(Duration::from_millis(100)).await;
            assert!(chat.cancel(&conversation_id).is_some());
        };
        let (result, ()) = tokio::join!(turn, cancel);

        assert_eq!(result.unwrap_err(), "Chat request cancelled");
        assert!(started.elapsed() < Duration::from_secs(2));
        let payloads = emitter.payloads.lock().unwrap();
        assert!(payloads.iter().any(|payload| {
            payload.kind == ChatDeltaKind::ToolResult
                && payload.status.as_deref() == Some("cancelled")
        }));
        assert_eq!(
            payloads
                .iter()
                .filter(|payload| payload.kind == ChatDeltaKind::Error)
                .count(),
            1
        );
        assert!(!payloads
            .iter()
            .any(|payload| payload.kind == ChatDeltaKind::Done));
        assert_eq!(
            payloads.last().map(|payload| &payload.kind),
            Some(&ChatDeltaKind::Error)
        );
        drop(payloads);
        assert!(chat
            .pending_tool_decisions
            .lock()
            .expect("tool decision lock should not be poisoned")
            .is_empty());
        let messages = storage
            .get_messages(&conversation_id)
            .expect("messages should load");
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[1].role, "assistant");
        assert!(messages[1].error);
        assert_eq!(messages[1].content, "Chat request cancelled");
        let next = chat
            .try_register_request(&conversation_id, Some("next-turn"))
            .expect("cancelled direct shell turn must release the conversation");
        chat.clear_request(&conversation_id, &next);

        drop(storage);
        let _ = std::fs::remove_dir_all(root);
        let _ = std::fs::remove_file(db_path);
    }

    #[test]
    fn shell_sandbox_mode_policy_matrix_keeps_only_safe_on_request_commands_read_only() {
        fn request(command: &str) -> ShellCommandRequest {
            ShellCommandRequest {
                command: command.to_string(),
                workdir: None,
                timeout_ms: None,
                login: Some(false),
                sandbox_permissions: exec::SandboxPermissions::UseDefault,
                justification: None,
                prefix_rule: None,
                additional_permissions: None,
            }
        }

        fn mode(
            policy: exec::ApprovalPolicy,
            approved: bool,
            request: &ShellCommandRequest,
        ) -> SandboxMode {
            shell_sandbox_mode(policy, approved, exec::needs_approval(request))
        }

        let safe = request("git status --short");
        let unknown = request("cargo check");
        let write_capable = request("echo changed > state.txt");
        let mut safe_with_escalation = request("git status --short");
        safe_with_escalation.sandbox_permissions = exec::SandboxPermissions::RequireEscalated;

        assert!(!exec::needs_approval(&safe));
        assert!(exec::needs_approval(&unknown));
        assert!(exec::needs_approval(&write_capable));
        assert!(exec::needs_approval(&safe_with_escalation));

        assert!(!shell_needs_confirmation(
            exec::ApprovalPolicy::OnRequest,
            &safe
        ));
        assert!(!shell_needs_confirmation(
            exec::ApprovalPolicy::OnRequest,
            &write_capable
        ));
        assert!(shell_needs_confirmation(
            exec::ApprovalPolicy::OnRequest,
            &safe_with_escalation
        ));
        assert!(!shell_needs_confirmation(exec::ApprovalPolicy::Ask, &safe));
        assert!(shell_needs_confirmation(
            exec::ApprovalPolicy::Ask,
            &write_capable
        ));
        for request in [&safe, &write_capable, &safe_with_escalation] {
            assert!(!shell_needs_confirmation(
                exec::ApprovalPolicy::OnFailure,
                request
            ));
            assert!(!shell_needs_confirmation(
                exec::ApprovalPolicy::SkipAll,
                request
            ));
        }

        assert_eq!(
            mode(exec::ApprovalPolicy::OnRequest, false, &safe),
            SandboxMode::ReadOnly
        );
        for request in [&unknown, &write_capable, &safe_with_escalation] {
            assert_eq!(
                mode(exec::ApprovalPolicy::OnRequest, false, request),
                SandboxMode::WorkspaceWrite
            );
        }
        assert_eq!(
            mode(exec::ApprovalPolicy::OnFailure, false, &safe),
            SandboxMode::ReadOnly
        );
        for request in [&unknown, &write_capable, &safe_with_escalation] {
            assert_eq!(
                mode(exec::ApprovalPolicy::OnFailure, false, request),
                SandboxMode::WorkspaceWrite
            );
        }

        assert_eq!(
            mode(exec::ApprovalPolicy::Ask, false, &safe),
            SandboxMode::ReadOnly
        );
        assert_eq!(
            mode(exec::ApprovalPolicy::Ask, true, &safe),
            SandboxMode::ReadOnly
        );
        assert_eq!(
            mode(exec::ApprovalPolicy::Ask, false, &write_capable),
            SandboxMode::ReadOnly
        );
        assert_eq!(
            mode(exec::ApprovalPolicy::Ask, true, &write_capable),
            SandboxMode::WorkspaceWrite
        );

        for request in [&safe, &write_capable] {
            assert_eq!(
                mode(exec::ApprovalPolicy::Never, false, request),
                SandboxMode::ReadOnly
            );
            assert_eq!(
                mode(exec::ApprovalPolicy::ReadOnly, true, request),
                SandboxMode::ReadOnly
            );
            assert_eq!(
                mode(exec::ApprovalPolicy::SkipAll, false, request),
                SandboxMode::DangerFullAccess
            );
        }
    }

    #[test]
    fn on_failure_non_zero_sandboxed_exit_requests_unsandboxed_retry_approval() {
        for sandbox_mode in [SandboxMode::ReadOnly, SandboxMode::WorkspaceWrite] {
            assert_eq!(
                on_failure_retry_action(
                    exec::ApprovalPolicy::OnFailure,
                    sandbox_mode,
                    Some(1),
                    false,
                    false,
                ),
                OnFailureRetryAction::RequestApproval
            );
        }
    }

    #[test]
    fn on_failure_cancelled_or_timed_out_command_does_not_prompt() {
        assert_eq!(
            on_failure_retry_action(
                exec::ApprovalPolicy::OnFailure,
                SandboxMode::WorkspaceWrite,
                None,
                false,
                false,
            ),
            OnFailureRetryAction::ReturnSandboxedResult
        );
        assert_eq!(
            on_failure_retry_action(
                exec::ApprovalPolicy::OnFailure,
                SandboxMode::WorkspaceWrite,
                Some(124),
                true,
                false,
            ),
            OnFailureRetryAction::ReturnSandboxedResult
        );
    }

    #[test]
    fn on_failure_session_approval_cache_hit_retries_without_reprompting() {
        let request = ShellCommandRequest {
            command: "cargo test".to_string(),
            workdir: None,
            timeout_ms: None,
            login: Some(false),
            sandbox_permissions: exec::SandboxPermissions::UseDefault,
            justification: None,
            prefix_rule: None,
            additional_permissions: None,
        };
        let approval_key = exec::approval_key_for(&request, "conversation", "/repo", "/repo");
        let chat = ChatManager::new();
        chat.approval_cache
            .approve_for_session(approval_key.clone());

        assert_eq!(
            on_failure_retry_action(
                exec::ApprovalPolicy::OnFailure,
                SandboxMode::WorkspaceWrite,
                Some(1),
                false,
                chat.approval_cache.contains(&approval_key),
            ),
            OnFailureRetryAction::RetryUnsandboxed
        );
    }

    #[test]
    fn first_write_confirmation_policy_matrix_keeps_sandbox_autonomous() {
        assert!(skips_first_write_confirmation(
            exec::ApprovalPolicy::OnRequest
        ));
        assert!(skips_first_write_confirmation(
            exec::ApprovalPolicy::OnFailure
        ));
        assert!(skips_first_write_confirmation(exec::ApprovalPolicy::Ask));
        assert!(skips_first_write_confirmation(
            exec::ApprovalPolicy::SkipAll
        ));

        for policy in [exec::ApprovalPolicy::ReadOnly, exec::ApprovalPolicy::Never] {
            assert!(!skips_first_write_confirmation(policy));
        }
    }

    #[test]
    fn patch_confirmation_matrix_distinguishes_ask_from_sandbox() {
        assert!(!patch_needs_confirmation(
            exec::ApprovalPolicy::OnRequest,
            false,
            false
        ));
        assert!(patch_needs_confirmation(
            exec::ApprovalPolicy::OnRequest,
            true,
            false
        ));
        assert!(patch_needs_confirmation(
            exec::ApprovalPolicy::Ask,
            false,
            false
        ));
        assert!(!patch_needs_confirmation(
            exec::ApprovalPolicy::Ask,
            false,
            true
        ));
        assert!(patch_needs_confirmation(
            exec::ApprovalPolicy::Ask,
            true,
            true
        ));
        assert!(!patch_needs_confirmation(
            exec::ApprovalPolicy::SkipAll,
            false,
            false
        ));
    }

    #[test]
    fn ask_patch_session_approval_is_cached_for_only_that_conversation() {
        let root =
            std::env::temp_dir().join(format!("halv-ask-patch-session-{}", storage::new_uuid_v4()));
        std::fs::create_dir_all(&root).expect("project should create");
        let roots = WorkspaceRoots::for_cwd(&root);
        let session_key = exec::patch_approval_key_for("conversation-a", &root, &roots, ".")
            .expect("session approval key should resolve");
        let other_conversation_key =
            exec::patch_approval_key_for("conversation-b", &root, &roots, ".")
                .expect("other conversation key should resolve");
        let chat = ChatManager::new();

        assert!(patch_needs_confirmation(
            exec::ApprovalPolicy::Ask,
            false,
            chat.approval_cache.contains_patch_path(&session_key)
        ));
        chat.approval_cache
            .approve_patch_paths([session_key.clone()]);
        assert!(!patch_needs_confirmation(
            exec::ApprovalPolicy::Ask,
            false,
            chat.approval_cache.contains_patch_path(&session_key)
        ));
        assert!(!chat
            .approval_cache
            .contains_patch_path(&other_conversation_key));

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn denied_shell_action_can_be_preapproved_once_for_retry() {
        let chat = ChatManager::new();
        let approval_key = exec::ApprovalKey::new_scoped(
            "conversation-a",
            "/projects/a",
            "cargo test",
            "/projects/a",
            exec::SandboxPermissions::UseDefault,
            false,
            exec::DEFAULT_TIMEOUT_MS,
            Vec::new(),
        );
        let _receiver = chat
            .register_tool_decision("call-a")
            .expect("decision should register");
        chat.register_denied_action(
            "call-a",
            DeniedToolAction::Shell {
                conversation_id: "conversation-a".to_string(),
                command: "cargo test".to_string(),
                cwd: "/projects/a".to_string(),
                approval_key: approval_key.clone(),
            },
        );

        assert!(chat.resolve_tool_decision("call-a", ReviewDecision::Denied));
        assert!(chat.approve_last_denied("conversation-a"));
        assert!(chat.approval_cache.contains(&approval_key));
        assert!(!chat.approve_last_denied("conversation-a"));
        assert!(!chat.approve_last_denied("conversation-b"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn first_write_guard_adds_no_second_prompt_for_sandbox_or_ask() {
        let non_git = std::env::temp_dir().join(format!(
            "halv-sandbox-first-write-non-git-{}",
            storage::new_uuid_v4()
        ));
        std::fs::create_dir_all(&non_git).expect("non-git project should create");

        let dirty_repo = std::env::temp_dir().join(format!(
            "halv-sandbox-first-write-dirty-{}",
            storage::new_uuid_v4()
        ));
        init_git_repo(&dirty_repo);
        std::fs::write(dirty_repo.join("README.md"), "clean\n")
            .expect("tracked fixture should write");
        run_git(&dirty_repo, &["add", "README.md"]);
        run_git(&dirty_repo, &["commit", "-m", "initial"]);
        std::fs::write(dirty_repo.join("dirty.txt"), "existing user work\n")
            .expect("dirty fixture should write");

        for (policy_label, policy) in [
            ("sandbox", exec::ApprovalPolicy::OnRequest),
            ("ask", exec::ApprovalPolicy::Ask),
        ] {
            for (project_label, root, warning_fragment) in [
                ("non-git", &non_git, "not a git repository"),
                ("dirty", &dirty_repo, "dirty"),
            ] {
                let emitter = RecordingEmitter::default();
                let chat = ChatManager::new();
                let conversation_id = format!("{policy_label}-{project_label}");
                let allowed = ensure_first_write_guard(
                    &emitter,
                    &chat,
                    &conversation_id,
                    &format!("call-{policy_label}-{project_label}"),
                    &root.to_string_lossy(),
                    "shell command",
                    "project write",
                    None,
                    skips_first_write_confirmation(policy),
                    Arc::new(AtomicBool::new(false)),
                )
                .await
                .expect("first-write guard should complete");

                assert!(allowed);
                assert!(chat
                    .pending_tool_decisions
                    .lock()
                    .expect("tool decision lock should not be poisoned")
                    .is_empty());
                assert!(chat
                    .write_guards
                    .lock()
                    .expect("write guard lock should not be poisoned")
                    .get(&conversation_id)
                    .is_some_and(|state| state.confirmed));
                let payloads = emitter
                    .payloads
                    .lock()
                    .expect("recording emitter lock should not be poisoned");
                assert!(payloads.iter().any(|payload| {
                    payload.kind == ChatDeltaKind::BranchStatus
                        && payload
                            .warning
                            .as_deref()
                            .is_some_and(|warning| warning.contains(warning_fragment))
                }));
                assert!(!payloads.iter().any(|payload| {
                    payload.kind == ChatDeltaKind::ToolCall
                        && payload.status.as_deref() == Some("awaiting_approval")
                }));
            }
        }

        let _ = std::fs::remove_dir_all(non_git);
        let _ = std::fs::remove_dir_all(dirty_repo);
    }

    #[test]
    fn proxy_port_accepts_listener_spawned_this_run() {
        let port = proxy_port_from_status(running_sidecar_status(true, false, 4301))
            .expect("this process spawned the listener");

        assert_eq!(port, 4301);
    }

    #[test]
    fn proxy_port_accepts_marker_detected_adopted_listener() {
        let port = proxy_port_from_status(running_sidecar_status(false, true, 4302))
            .expect("Halv detected the adopted listener");

        assert_eq!(port, 4302);
    }

    #[test]
    fn proxy_port_reports_an_engine_that_is_off() {
        let error = proxy_port_from_status(SidecarStatus::Off { port: 4304 })
            .expect_err("an engine that is off has no port");

        assert_eq!(error.message, "Halv Engine is off");
    }

    // The app asks for the model list at launch and on each window focus.
    // While the engine is off, the list must not start it.
    #[tokio::test]
    async fn the_model_list_does_not_start_an_engine_that_is_off() {
        let chat = ChatManager::new();
        let sidecar = SidecarManager::new(4306);
        sidecar.set_enabled(false);

        let error = fetch_openai_models(&chat, &sidecar, &chatgpt_bearer())
            .await
            .expect_err("an engine that is off gives no model registry");

        // `chat_list_models` turns this error into the built-in models.
        assert_eq!(error, "Halv Engine is off");
        assert!(matches!(
            sidecar.refresh_status().await,
            SidecarStatus::Off { port: 4306 }
        ));
    }

    #[test]
    fn proxy_port_refuses_unmanaged_running_listener() {
        let error = proxy_port_from_status(running_sidecar_status(false, false, 4303))
            .expect_err("chat credentials must never be sent to an unmanaged listener");

        assert!(error
            .message
            .contains("Refusing to send credentials to an unmanaged listener"));
    }

    fn stored_message(role: &str, content: &str) -> StoredMessage {
        stored_message_at(1, role, content)
    }

    fn stored_message_at(sequence: i64, role: &str, content: &str) -> StoredMessage {
        StoredMessage {
            sequence,
            id: storage::new_uuid_v4(),
            conversation_id: "11111111-1111-4111-8111-111111111111".to_string(),
            role: role.to_string(),
            content: content.to_string(),
            created_at: "2026-07-01T00:00:00Z".to_string(),
            model: Some(DEFAULT_MODEL.to_string()),
            input_tokens: None,
            output_tokens: None,
            error: false,
            mentions: None,
            attachments: Vec::new(),
            tokens_saved: None,
            savings_usd: None,
            pricing_source: None,
            crux_tokens_saved: None,
            crux_included_in_total: false,
            cached_tokens: None,
            savings_source: None,
            usd_method: None,
        }
    }

    fn stored_image_message(content: &str) -> StoredMessage {
        let mut message = stored_message("user", content);
        message.attachments.push(storage::MessageAttachment {
            id: storage::new_uuid_v4(),
            name: "screen.png".to_string(),
            media_type: "image/png".to_string(),
            size_bytes: 8,
            data: STANDARD.encode([0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]),
        });
        message
    }

    #[test]
    fn compaction_threshold_uses_eighty_percent_of_each_model_window() {
        let usage = |input_tokens| TokenUsage {
            input_tokens,
            output_tokens: 0,
            total_tokens: input_tokens,
            cached_tokens: 0,
            reasoning_tokens: 0,
        };

        assert_eq!(model_context_window_tokens("gpt-5.6-sol"), 272_000);
        assert!(!should_compact(&usage(217_600), "gpt-5.6-sol"));
        assert!(should_compact(&usage(217_601), "gpt-5.6-sol"));

        assert_eq!(model_context_window_tokens("claude-sonnet-5"), 200_000);
        assert!(!should_compact(&usage(160_000), "claude-sonnet-5"));
        assert!(should_compact(&usage(160_001), "claude-sonnet-5"));

        assert_eq!(model_context_window_tokens("future-model"), 200_000);
        assert!(should_compact(&usage(160_001), "future-model"));
    }

    #[test]
    fn compacted_replay_replaces_watermarked_messages_after_project_context() {
        let messages = vec![
            stored_message_at(1, "user", "old user message"),
            stored_message_at(2, "assistant", "old assistant message"),
            stored_message_at(3, "user", "remaining user message"),
            stored_message_at(4, "assistant", "remaining assistant message"),
        ];
        let context = ProjectPromptContext {
            environment_context: "environment context".to_string(),
            agents_instructions: Some("AGENTS instructions".to_string()),
        };
        let compaction = storage::ConversationCompaction {
            id: 1,
            conversation_id: messages[0].conversation_id.clone(),
            upto_message_id: 2,
            summary: "Prior decisions and current state.".to_string(),
            created_at: "2026-07-01T00:00:00Z".to_string(),
        };
        let summary = compacted_summary_message(&compaction.summary);

        let mut openai = build_response_input_items_with_compaction(
            &messages,
            Some(&context),
            Some("referenced files"),
            Some(&compaction),
        );
        assert_eq!(openai[0], user_input_message("environment context"));
        assert_eq!(openai[1], user_input_message("AGENTS instructions"));
        assert_eq!(openai[2], user_input_message(&summary));
        assert_eq!(openai[3], user_input_message("referenced files"));
        assert_eq!(openai[4], user_input_message("remaining user message"));
        assert_eq!(
            openai[5].pointer("/content/0/text").and_then(Value::as_str),
            Some("remaining assistant message")
        );
        openai.push(json!({
            "type": "function_call",
            "name": "shell_command",
            "call_id": "call_after_compaction",
            "arguments": "{\"command\":\"pwd\"}"
        }));
        openai.push(function_call_output_item(
            "call_after_compaction",
            "Exit code: 0",
        ));
        let summary_index = openai
            .iter()
            .position(|item| item == &user_input_message(&summary))
            .expect("summary should be present");
        let function_call_index = openai
            .iter()
            .position(|item| item.get("type").and_then(Value::as_str) == Some("function_call"))
            .expect("function call should be present");
        let function_output_index = openai
            .iter()
            .position(|item| {
                item.get("type").and_then(Value::as_str) == Some("function_call_output")
            })
            .expect("function output should be present");
        assert!(summary_index < function_call_index);
        assert_eq!(function_output_index, function_call_index + 1);
        let serialized = serde_json::to_string(&openai).expect("input should serialize");
        assert!(!serialized.contains("old user message"));
        assert!(!serialized.contains("old assistant message"));

        let claude = build_claude_messages_with_compaction(
            &messages,
            Some(&context),
            Some("referenced files"),
            Some(&compaction),
        );
        assert_eq!(
            claude[0],
            json!({ "role": "user", "content": "environment context" })
        );
        assert_eq!(
            claude[1],
            json!({ "role": "user", "content": "AGENTS instructions" })
        );
        assert_eq!(claude[2], json!({ "role": "user", "content": summary }));
        assert_eq!(
            claude[3],
            json!({ "role": "user", "content": "referenced files" })
        );
        assert_eq!(
            claude[4],
            json!({ "role": "user", "content": "remaining user message" })
        );
        assert_eq!(
            claude[5],
            json!({ "role": "assistant", "content": "remaining assistant message" })
        );
    }

    #[test]
    fn compaction_watermark_is_only_a_completed_turn_boundary() {
        let mut failed = stored_message_at(4, "assistant", "failed response");
        failed.error = true;
        let messages = vec![
            stored_message_at(1, "user", "first question"),
            stored_message_at(2, "assistant", "first answer"),
            stored_message_at(3, "user", "failed question"),
            failed,
            stored_message_at(5, "user", "second question"),
            stored_message_at(6, "assistant", "second answer after tool calls and outputs"),
            stored_message_at(7, "user", "incomplete next turn"),
        ];

        let completed = completed_turn_stats(&messages);
        assert_eq!(completed.count, 2);
        assert_eq!(completed.last_message_sequence, Some(6));
        assert_eq!(messages[5].role, "assistant");
        assert!(!messages[5].error);
        // Function-call/output items exist only inside the in-memory assistant
        // turn. A persisted successful assistant row is therefore outside the
        // pair, while the incomplete next turn remains after the watermark.
        assert!(messages[6].sequence > completed.last_message_sequence.unwrap());
    }

    #[test]
    fn responses_history_serializes_images_as_native_input_parts() {
        let item = message_to_response_item(&stored_image_message("What is this?"))
            .expect("image message should serialize");

        assert_eq!(
            item.pointer("/content/0/type").and_then(Value::as_str),
            Some("input_image")
        );
        assert_eq!(
            item.pointer("/content/0/image_url").and_then(Value::as_str),
            Some("data:image/png;base64,iVBORw0KGgo=")
        );
        assert_eq!(
            item.pointer("/content/1/text").and_then(Value::as_str),
            Some("What is this?")
        );

        let image_only = message_to_response_item(&stored_image_message(""))
            .expect("image-only message should serialize");
        assert_eq!(
            image_only
                .get("content")
                .and_then(Value::as_array)
                .map(Vec::len),
            Some(1)
        );
    }

    #[test]
    fn claude_history_serializes_images_as_base64_source_blocks() {
        let messages = build_claude_messages(&[stored_image_message("Describe it")], None, None);
        assert_eq!(messages.len(), 1);
        assert_eq!(
            messages[0]
                .pointer("/content/0/type")
                .and_then(Value::as_str),
            Some("image")
        );
        assert_eq!(
            messages[0]
                .pointer("/content/0/source/type")
                .and_then(Value::as_str),
            Some("base64")
        );
        assert_eq!(
            messages[0]
                .pointer("/content/0/source/media_type")
                .and_then(Value::as_str),
            Some("image/png")
        );
        assert_eq!(
            messages[0]
                .pointer("/content/0/source/data")
                .and_then(Value::as_str),
            Some("iVBORw0KGgo=")
        );
        assert_eq!(
            messages[0]
                .pointer("/content/1/text")
                .and_then(Value::as_str),
            Some("Describe it")
        );
    }

    #[test]
    fn image_input_validation_checks_magic_type_size_and_count() {
        let png_data = STANDARD.encode([0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]);
        let valid = ImageAttachmentInput {
            name: "screen.png".to_string(),
            media_type: "image/png".to_string(),
            size_bytes: 8,
            data: png_data.clone(),
        };
        let normalized =
            normalize_image_attachments(vec![valid.clone()]).expect("valid PNG should normalize");
        assert_eq!(normalized[0].data.len(), 8);

        let mismatch = ImageAttachmentInput {
            media_type: "image/jpeg".to_string(),
            ..valid.clone()
        };
        assert!(normalize_image_attachments(vec![mismatch])
            .expect_err("mismatched type should fail")
            .contains("does not match"));

        let wrong_size = ImageAttachmentInput {
            size_bytes: 7,
            ..valid.clone()
        };
        assert!(normalize_image_attachments(vec![wrong_size])
            .expect_err("wrong size should fail")
            .contains("inconsistent image size"));

        assert!(
            normalize_image_attachments(vec![valid; MAX_IMAGE_ATTACHMENTS_PER_MESSAGE + 1])
                .expect_err("too many images should fail")
                .contains("Attach at most")
        );

        let valid_history = stored_image_message("Inspect");
        validate_image_attachment_history(std::slice::from_ref(&valid_history))
            .expect("valid stored image should pass history validation");
        let mut corrupt_history = valid_history;
        corrupt_history.attachments[0].media_type = "image/jpeg".to_string();
        assert!(validate_image_attachment_history(&[corrupt_history])
            .expect_err("corrupt stored metadata should fail")
            .contains("inconsistent metadata"));

        let full_history = (0..MAX_IMAGE_ATTACHMENTS_PER_HISTORY)
            .map(|_| stored_image_message("Prior image"))
            .collect::<Vec<_>>();
        assert!(
            validate_new_attachments_against_history(&full_history, &normalized)
                .expect_err("prospective history overflow should fail before persistence")
                .contains("would include more than")
        );
    }

    fn dropped_image_test_path(name: &str) -> PathBuf {
        let directory =
            std::env::temp_dir().join(format!("halv-dropped-image-{}", storage::new_uuid_v4()));
        std::fs::create_dir_all(&directory).expect("image fixture directory should create");
        directory.join(name)
    }

    #[test]
    fn composer_read_dropped_image_reads_png() {
        let bytes = [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
        let path = dropped_image_test_path("screen.png");
        std::fs::write(&path, bytes).expect("PNG fixture should write");

        let image = composer_read_dropped_image(path.to_string_lossy().into_owned())
            .expect("PNG fixture should load");
        assert_eq!(image.name, "screen.png");
        assert_eq!(image.media_type, "image/png");
        assert_eq!(image.size_bytes, bytes.len() as u64);
        assert_eq!(image.data, STANDARD.encode(bytes));
        let serialized = serde_json::to_value(image).expect("image response should serialize");
        assert_eq!(
            serialized.get("mediaType").and_then(Value::as_str),
            Some("image/png")
        );
        assert_eq!(
            serialized.get("sizeBytes").and_then(Value::as_u64),
            Some(bytes.len() as u64)
        );
        assert!(serialized.get("media_type").is_none());

        let _ = std::fs::remove_dir_all(path.parent().expect("fixture should have a parent"));
    }

    #[test]
    fn composer_read_dropped_image_reads_jpeg() {
        let bytes = [0xff, 0xd8, 0xff, 0xe0];
        let path = dropped_image_test_path("photo.JPEG");
        std::fs::write(&path, bytes).expect("JPEG fixture should write");

        let image = composer_read_dropped_image(path.to_string_lossy().into_owned())
            .expect("JPEG fixture should load");
        assert_eq!(image.name, "photo.JPEG");
        assert_eq!(image.media_type, "image/jpeg");
        assert_eq!(image.size_bytes, bytes.len() as u64);
        assert_eq!(image.data, STANDARD.encode(bytes));

        let _ = std::fs::remove_dir_all(path.parent().expect("fixture should have a parent"));
    }

    #[test]
    fn composer_read_dropped_image_reads_webp() {
        let bytes = *b"RIFF\x04\x00\x00\x00WEBP";
        let path = dropped_image_test_path("preview.webp");
        std::fs::write(&path, bytes).expect("WebP fixture should write");

        let image = composer_read_dropped_image(path.to_string_lossy().into_owned())
            .expect("WebP fixture should load");
        assert_eq!(image.name, "preview.webp");
        assert_eq!(image.media_type, "image/webp");
        assert_eq!(image.size_bytes, bytes.len() as u64);
        assert_eq!(image.data, STANDARD.encode(bytes));

        let _ = std::fs::remove_dir_all(path.parent().expect("fixture should have a parent"));
    }

    #[test]
    fn composer_read_dropped_image_rejects_oversized_file() {
        let path = dropped_image_test_path("large.png");
        let file = std::fs::File::create(&path).expect("oversized fixture should create");
        file.set_len((MAX_IMAGE_ATTACHMENT_BYTES + 1) as u64)
            .expect("oversized fixture length should set");

        let error = composer_read_dropped_image(path.to_string_lossy().into_owned())
            .expect_err("oversized fixture should fail");
        assert!(error.contains("exceeds the 5 MiB per-image limit"));

        let _ = std::fs::remove_dir_all(path.parent().expect("fixture should have a parent"));
    }

    #[test]
    fn composer_read_dropped_image_rejects_magic_mismatch() {
        let path = dropped_image_test_path("photo.png");
        std::fs::write(&path, [0xff, 0xd8, 0xff]).expect("mismatch fixture should write");

        let error = composer_read_dropped_image(path.to_string_lossy().into_owned())
            .expect_err("mismatched fixture should fail");
        assert!(error.contains("content does not match its declared image type"));

        let _ = std::fs::remove_dir_all(path.parent().expect("fixture should have a parent"));
    }

    #[test]
    fn composer_read_dropped_image_rejects_missing_file() {
        let path = dropped_image_test_path("missing.png");

        let error = composer_read_dropped_image(path.to_string_lossy().into_owned())
            .expect_err("missing fixture should fail");
        assert!(error.contains("was not found"));

        let _ = std::fs::remove_dir_all(path.parent().expect("fixture should have a parent"));
    }

    #[test]
    fn parses_responses_sse_fixture_deltas_reasoning_and_usage() {
        let mut parser = SseParser::default();
        let mut events = parser
            .push(SSE_FIXTURE.as_bytes(), MAX_STREAM_EVENTS)
            .expect("fixture should parse");

        events.retain(|event| !matches!(event, ParsedSseEvent::Ignored));

        assert_eq!(
            events,
            vec![
                ParsedSseEvent::Delta("Hello".to_string()),
                ParsedSseEvent::ReasoningBoundary,
                ParsedSseEvent::Reasoning("I should answer directly.".to_string()),
                ParsedSseEvent::Reasoning("Hidden reasoning sample.".to_string()),
                ParsedSseEvent::Delta(", world".to_string()),
                ParsedSseEvent::FinalMessage {
                    text: "Hello, world".to_string(),
                    item: json!({
                        "type": "message",
                        "role": "assistant",
                        "content": [{
                            "type": "output_text",
                            "text": "Hello, world"
                        }]
                    }),
                },
                ParsedSseEvent::Completed {
                    usage: Some(TokenUsage {
                        input_tokens: 11,
                        output_tokens: 7,
                        total_tokens: 18,
                        cached_tokens: 3,
                        reasoning_tokens: 2,
                    }),
                    service_tier: None,
                },
            ]
        );
        assert!(parser.is_empty());
    }

    #[test]
    fn completed_response_reports_actual_service_tier() {
        assert_eq!(
            parse_responses_event(json!({
                "type": "response.completed",
                "response": {
                    "service_tier": "priority"
                }
            }))
            .expect("completed event should parse"),
            ParsedSseEvent::Completed {
                usage: None,
                service_tier: Some("priority".to_string()),
            }
        );
    }

    #[test]
    fn reasoning_summary_part_boundaries_prevent_markdown_fragments_running_together() {
        let input = concat!(
            "data: {\"type\":\"response.reasoning_summary_part.added\",\"summary_index\":0}\n\n",
            "data: {\"type\":\"response.reasoning_summary_text.delta\",\"summary_index\":0,\"delta\":\"**Planning the change**\"}\n\n",
            "data: {\"type\":\"response.reasoning_summary_part.added\",\"summary_index\":1}\n\n",
            "data: {\"type\":\"response.reasoning_summary_text.delta\",\"summary_index\":1,\"delta\":\"**Designing the fix**\"}\n\n",
        );
        let events = SseParser::default()
            .push(input.as_bytes(), MAX_STREAM_EVENTS)
            .expect("reasoning summary stream should parse");
        let displayed = events.into_iter().fold(String::new(), |mut text, event| {
            match event {
                ParsedSseEvent::Reasoning(delta) => text.push_str(&delta),
                ParsedSseEvent::ReasoningBoundary => text.push_str(REASONING_FRAGMENT_SEPARATOR),
                _ => {}
            }
            text
        });

        assert_eq!(
            displayed,
            "\n\n**Planning the change**\n\n**Designing the fix**"
        );
        assert!(!displayed.contains("****"));
    }

    #[test]
    fn sse_parser_rejects_oversized_complete_and_incomplete_frames() {
        let mut parser = SseParser::default();
        let incomplete = vec![b'x'; MAX_SSE_FRAME_BYTES + 1];
        let split = MAX_SSE_FRAME_BYTES / 2;
        parser
            .push(&incomplete[..split], MAX_STREAM_EVENTS)
            .expect("first partial frame should fit");
        let error = parser
            .push(&incomplete[split..], MAX_STREAM_EVENTS)
            .expect_err("slow oversized frame should be rejected");
        assert!(error.message.contains("incomplete SSE frame"));

        let mut parser = SseParser::default();
        let mut complete = vec![b'x'; MAX_SSE_FRAME_BYTES + 1];
        complete.extend_from_slice(b"\n\n");
        let error = parser
            .push(&complete, MAX_STREAM_EVENTS)
            .expect_err("complete oversized frame should be rejected");
        assert!(error.message.contains("SSE frame exceeds"));
    }

    #[test]
    fn sse_parser_accepts_a_large_chunk_of_small_frames_without_front_draining() {
        let frame = b": keepalive\n\n";
        let repeats = MAX_SSE_FRAME_BYTES / frame.len() + 2;
        let chunk = frame.repeat(repeats);
        assert!(chunk.len() > MAX_SSE_FRAME_BYTES);
        let mut parser = SseParser::default();

        let events = parser
            .push(&chunk, MAX_STREAM_EVENTS)
            .expect("individually small frames should parse");

        assert!(events.is_empty());
        assert!(parser.is_empty());
    }

    #[test]
    fn sse_parser_enforces_the_remaining_event_budget() {
        let frame = b"data: {\"type\":\"response.created\"}\n\n";
        let chunk = frame.repeat(2);
        let mut parser = SseParser::default();

        let error = parser
            .push(&chunk, 1)
            .expect_err("second parsed event should exceed the budget");

        assert!(error.message.contains("event count"));
    }

    #[test]
    fn stream_budget_rejects_multibyte_text_and_item_overflow_before_mutation() {
        let mut budget = StreamBudget {
            assistant_bytes: MAX_STREAM_ASSISTANT_BYTES - 1,
            output_items: MAX_STREAM_OUTPUT_ITEMS,
            ..StreamBudget::default()
        };

        assert!(budget.charge_assistant("é").is_err());
        assert_eq!(budget.assistant_bytes, MAX_STREAM_ASSISTANT_BYTES - 1);
        assert!(budget.charge_output_item().is_err());
        assert_eq!(budget.output_items, MAX_STREAM_OUTPUT_ITEMS);

        budget.raw_bytes = MAX_STREAM_RAW_BYTES - 1;
        budget.charge_raw(1).expect("last raw byte should fit");
        let error = budget
            .charge_raw(1)
            .expect_err("next raw byte should exceed the request limit");
        assert!(error.message.contains("per-request safety limit"));
        assert_eq!(budget.raw_bytes, MAX_STREAM_RAW_BYTES);
    }

    #[test]
    fn agent_round_starts_with_a_fresh_stream_budget() {
        let mut first_request = request_stream_budget();
        first_request
            .charge_raw(MAX_STREAM_RAW_BYTES)
            .expect("first request should accept its full raw byte budget");
        first_request
            .charge_events(MAX_STREAM_EVENTS)
            .expect("first request should accept its full event budget");

        let second_request = request_stream_budget();

        assert_eq!(second_request.raw_bytes, 0);
        assert_eq!(second_request.events, 0);
        assert_eq!(second_request.assistant_bytes, 0);
        assert_eq!(second_request.reasoning_bytes, 0);
        assert_eq!(second_request.output_items, 0);
    }

    #[test]
    fn main_turn_tool_budget_is_unlimited() {
        let mut budget = ToolBudget::Unlimited;

        for _ in 0..1_000 {
            budget
                .charge_tool_call()
                .expect("main turn tool calls should stay unlimited");
        }
    }

    #[test]
    fn subagent_tool_budget_keeps_its_hard_limit() {
        let mut budget = HardToolBudget {
            tool_calls: MAX_SUBAGENT_TOOL_CALLS_PER_TURN,
        };

        assert!(budget.charge_tool_call().is_err());
        assert_eq!(budget.tool_calls, MAX_SUBAGENT_TOOL_CALLS_PER_TURN);
    }

    #[test]
    fn agent_round_usage_is_summed_and_rejects_invalid_values_atomically() {
        let mut usage = None;
        merge_token_usage(
            &mut usage,
            Some(TokenUsage {
                input_tokens: 10,
                output_tokens: 4,
                total_tokens: 14,
                cached_tokens: 2,
                reasoning_tokens: 1,
            }),
        )
        .expect("first usage should merge");
        merge_token_usage(
            &mut usage,
            Some(TokenUsage {
                input_tokens: 20,
                output_tokens: 6,
                total_tokens: 26,
                cached_tokens: 3,
                reasoning_tokens: 2,
            }),
        )
        .expect("second usage should merge");
        assert_eq!(
            usage,
            Some(TokenUsage {
                input_tokens: 30,
                output_tokens: 10,
                total_tokens: 40,
                cached_tokens: 5,
                reasoning_tokens: 3,
            })
        );

        let before_invalid = usage.clone();
        assert!(merge_token_usage(
            &mut usage,
            Some(TokenUsage {
                input_tokens: -1,
                output_tokens: 0,
                total_tokens: 0,
                cached_tokens: 0,
                reasoning_tokens: 0,
            }),
        )
        .is_err());
        assert_eq!(usage, before_invalid);

        let mut overflow = Some(TokenUsage {
            input_tokens: i64::MAX,
            output_tokens: 0,
            total_tokens: i64::MAX,
            cached_tokens: 0,
            reasoning_tokens: 0,
        });
        let before_overflow = overflow.clone();
        assert!(merge_token_usage(
            &mut overflow,
            Some(TokenUsage {
                input_tokens: 1,
                output_tokens: 0,
                total_tokens: 1,
                cached_tokens: 0,
                reasoning_tokens: 0,
            }),
        )
        .is_err());
        assert_eq!(overflow, before_overflow);
    }

    #[test]
    fn later_agent_errors_preserve_prior_and_current_partial_text() {
        let error = ChatError::with_partial("later round failed", "current".to_string());

        let error = prepend_stream_partial(error, "prior");

        assert_eq!(error.message, "later round failed");
        assert_eq!(error.partial_text, "priorcurrent");
    }

    #[test]
    fn transient_empty_stream_failure_retries_twice_then_exhausts() {
        let error = ChatError::transient("temporary transport failure");
        let progress = StreamAttemptProgress::default();

        assert_eq!(
            stream_retry_delay(0, &error, progress),
            Some(Duration::from_millis(500))
        );
        assert_eq!(
            stream_retry_delay(1, &error, progress),
            Some(Duration::from_millis(1_500))
        );
        assert_eq!(stream_retry_delay(2, &error, progress), None);
    }

    #[test]
    fn parses_openai_capacity_headers_without_inventing_missing_values() {
        let captured_at = Utc
            .with_ymd_and_hms(2026, 8, 16, 12, 0, 0)
            .single()
            .expect("capture time should be valid");
        let mut complete = HeaderMap::new();
        complete.insert(
            "x-ratelimit-limit-requests",
            HeaderValue::from_static("100"),
        );
        complete.insert(
            "x-ratelimit-remaining-requests",
            HeaderValue::from_static("75"),
        );
        complete.insert("x-ratelimit-reset-requests", HeaderValue::from_static("1s"));
        complete.insert(
            "x-ratelimit-limit-tokens",
            HeaderValue::from_static("20000"),
        );
        complete.insert(
            "x-ratelimit-remaining-tokens",
            HeaderValue::from_static("12000"),
        );
        complete.insert("x-ratelimit-reset-tokens", HeaderValue::from_static("6m0s"));
        complete.insert("retry-after", HeaderValue::from_static("3"));

        let records =
            parse_openai_usage_headers(&complete, Some("profile"), "gpt-5.5", captured_at);
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].window_kind, "requests");
        assert_eq!(records[0].limit_value, Some(100));
        assert_eq!(records[0].remaining, Some(75));
        assert_eq!(records[0].used, Some(25));
        assert_eq!(
            records[0].resets_at.as_deref(),
            Some("2026-08-16T12:00:01+00:00")
        );
        assert_eq!(records[0].retry_after_seconds, Some(3));
        assert_eq!(records[0].source, "headers");
        assert_eq!(records[0].confidence, "known");
        assert_eq!(records[1].window_kind, "tokens");
        assert_eq!(records[1].used, Some(8_000));
        assert_eq!(
            records[1].resets_at.as_deref(),
            Some("2026-08-16T12:06:00+00:00")
        );

        let mut partial = HeaderMap::new();
        partial.insert(
            "x-ratelimit-remaining-tokens",
            HeaderValue::from_static("400"),
        );
        let records = parse_openai_usage_headers(&partial, None, "gpt-5.5", captured_at);
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].window_kind, "tokens");
        assert_eq!(records[0].limit_value, None);
        assert_eq!(records[0].remaining, Some(400));
        assert_eq!(records[0].used, None);

        assert!(
            parse_openai_usage_headers(&HeaderMap::new(), None, "gpt-5.5", captured_at).is_empty()
        );

        let mut garbage = HeaderMap::new();
        garbage.insert(
            "x-ratelimit-limit-requests",
            HeaderValue::from_static("many"),
        );
        garbage.insert(
            "x-ratelimit-reset-requests",
            HeaderValue::from_static("eventually"),
        );
        let records = parse_openai_usage_headers(&garbage, None, "gpt-5.5", captured_at);
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].limit_value, None);
        assert_eq!(records[0].remaining, None);
        assert_eq!(records[0].resets_at, None);
    }

    #[test]
    fn parses_anthropic_capacity_headers_without_aggregating_token_windows() {
        let captured_at = Utc
            .with_ymd_and_hms(2026, 8, 16, 12, 0, 0)
            .single()
            .expect("capture time should be valid");
        let mut complete = HeaderMap::new();
        for (name, value) in [
            ("anthropic-ratelimit-requests-limit", "60"),
            ("anthropic-ratelimit-requests-remaining", "50"),
            ("anthropic-ratelimit-requests-reset", "2026-08-16T12:01:00Z"),
            ("anthropic-ratelimit-input-tokens-limit", "10000"),
            ("anthropic-ratelimit-input-tokens-remaining", "7000"),
            (
                "anthropic-ratelimit-input-tokens-reset",
                "2026-08-16T12:02:00Z",
            ),
            ("anthropic-ratelimit-output-tokens-limit", "4000"),
            ("anthropic-ratelimit-output-tokens-remaining", "3500"),
            (
                "anthropic-ratelimit-output-tokens-reset",
                "2026-08-16T12:03:00Z",
            ),
        ] {
            complete.insert(
                HeaderName::from_bytes(name.as_bytes()).expect("header name should be valid"),
                HeaderValue::from_str(value).expect("header value should be valid"),
            );
        }
        complete.insert("retry-after", HeaderValue::from_static("4"));

        let records = parse_anthropic_usage_headers(
            &complete,
            Some("profile"),
            "claude-sonnet-5",
            captured_at,
        );
        assert_eq!(records.len(), 3);
        assert_eq!(records[0].window_kind, "requests");
        assert_eq!(records[0].used, Some(10));
        assert_eq!(records[1].window_kind, "input_tokens");
        assert_eq!(records[1].used, Some(3_000));
        assert_eq!(records[2].window_kind, "output_tokens");
        assert_eq!(records[2].used, Some(500));
        assert_eq!(
            records[2].resets_at.as_deref(),
            Some("2026-08-16T12:03:00+00:00")
        );
        assert!(records
            .iter()
            .all(|record| record.retry_after_seconds == Some(4)));

        let mut partial = HeaderMap::new();
        partial.insert(
            "anthropic-ratelimit-output-tokens-remaining",
            HeaderValue::from_static("123"),
        );
        let records = parse_anthropic_usage_headers(&partial, None, "claude-sonnet-5", captured_at);
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].window_kind, "output_tokens");
        assert_eq!(records[0].limit_value, None);
        assert_eq!(records[0].remaining, Some(123));

        assert!(parse_anthropic_usage_headers(
            &HeaderMap::new(),
            None,
            "claude-sonnet-5",
            captured_at
        )
        .is_empty());

        let mut garbage = HeaderMap::new();
        garbage.insert(
            "anthropic-ratelimit-input-tokens-limit",
            HeaderValue::from_static("unlimited"),
        );
        garbage.insert(
            "anthropic-ratelimit-input-tokens-reset",
            HeaderValue::from_static("tomorrow"),
        );
        let records = parse_anthropic_usage_headers(&garbage, None, "claude-sonnet-5", captured_at);
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].window_kind, "input_tokens");
        assert_eq!(records[0].limit_value, None);
        assert_eq!(records[0].resets_at, None);
    }

    #[test]
    fn parses_provider_reset_durations_leniently() {
        assert_eq!(
            parse_rate_limit_duration("1s").map(|duration| duration.num_milliseconds()),
            Some(1_000)
        );
        assert_eq!(
            parse_rate_limit_duration("6m0s").map(|duration| duration.num_milliseconds()),
            Some(360_000)
        );
        assert_eq!(
            parse_rate_limit_duration("250ms").map(|duration| duration.num_milliseconds()),
            Some(250)
        );
        assert_eq!(parse_rate_limit_duration("junk"), None);
    }

    #[test]
    fn rate_limit_error_sets_cooldown_and_success_clears_it() {
        let path = std::env::temp_dir().join(format!(
            "halv-chat-capacity-cooldown-{}.sqlite3",
            storage::new_uuid_v4()
        ));
        let storage = Storage::open(&path).expect("test storage should open");
        let capture = CapacityCaptureContext::openai(&storage, None, "gpt-5.5");
        let recorded_at = Utc
            .with_ymd_and_hms(2026, 8, 16, 12, 0, 0)
            .single()
            .expect("recorded time should be valid");
        capture.capture_rate_limit_error(
            StatusCode::TOO_MANY_REQUESTS,
            r#"{"error":{"type":"usage_limit_reached","resets_in_seconds":60}}"#,
            Some(10),
            "You've hit your ChatGPT plan's usage limit.",
            recorded_at,
        );

        let cooldowns = storage
            .list_profile_cooldowns()
            .expect("cooldown should list");
        assert_eq!(cooldowns.len(), 1);
        assert_eq!(cooldowns[0].provider, "openai");
        assert_eq!(cooldowns[0].credential_profile_id, None);
        assert_eq!(cooldowns[0].reason, "usage_limit_reached");
        assert_eq!(
            cooldowns[0].next_attempt_at.as_deref(),
            Some("2026-08-16T12:01:00+00:00")
        );
        assert_eq!(
            cooldowns[0].last_error.as_deref(),
            Some("You've hit your ChatGPT plan's usage limit.")
        );

        capture.clear_cooldown();
        assert!(storage
            .list_profile_cooldowns()
            .expect("successful response should clear cooldown")
            .is_empty());

        drop(storage);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn formats_usage_limit_upstream_error_without_raw_json() {
        let body = r#"{"error":{"type":"usage_limit_reached","message":"The usage limit has been reached","plan_type":"pro","resets_at":1783741922,"eligible_promo":null,"resets_in_seconds":6386}}"#;

        let message = format_upstream_error_message(StatusCode::TOO_MANY_REQUESTS, body);

        assert_eq!(
            message,
            "You've hit your ChatGPT plan's usage limit. Resets in about 1h 46m. (HTTP 429 · usage_limit_reached)"
        );
        assert!(!message.contains("{\"error\""));
    }

    #[test]
    fn formats_typed_upstream_error_from_flat_envelope_message() {
        let body = r#"{"message":"The request was invalid.","type":"invalid_request_error"}"#;

        let message = format_upstream_error_message(StatusCode::BAD_REQUEST, body);

        assert_eq!(
            message,
            "The request was invalid. (HTTP 400 · invalid_request_error)"
        );
        assert!(!message.contains("{\"message\""));
    }

    #[test]
    fn parses_string_fastapi_detail_as_upstream_error_message() {
        let details =
            parse_upstream_error_details(r#"{"detail":"Unsupported service_tier: fast"}"#)
                .expect("FastAPI error body should parse");
        let non_string_details = parse_upstream_error_details(r#"{"detail":{"reason":"bad"}}"#)
            .expect("FastAPI error body should parse");

        assert_eq!(
            details.message.as_deref(),
            Some("Unsupported service_tier: fast")
        );
        assert_eq!(non_string_details.message, None);
    }

    #[test]
    fn cleans_and_truncates_non_json_upstream_error() {
        let body = format!("  gateway \n\t unavailable   {} trailing", "x".repeat(240));

        let message = format_upstream_error_message(StatusCode::BAD_GATEWAY, &body);

        assert_eq!(
            message,
            format!("gateway unavailable {}… (HTTP 502)", "x".repeat(180))
        );
        assert!(!message.contains('\n'));
        assert!(!message.contains('\t'));
        assert!(!message.contains(&body));
    }

    #[test]
    fn formats_unauthorized_upstream_error_as_fresh_sign_in() {
        let body = r#"{"error":{"type":"invalid_authentication","message":"Token expired."}}"#;

        let message = format_upstream_error_message(StatusCode::UNAUTHORIZED, body);

        assert_eq!(
            message,
            "Your ChatGPT session expired — reconnect it in Settings. Token expired. (HTTP 401 · invalid_authentication)"
        );
        assert!(!message.contains("{\"error\""));
    }

    #[test]
    fn formats_generic_upstream_errors_with_model_facing_recovery_copy() {
        let typed = format_upstream_error_message(
            StatusCode::BAD_GATEWAY,
            r#"{"error":{"type":"server_error"}}"#,
        );
        let empty = format_upstream_error_message(StatusCode::BAD_GATEWAY, "   ");

        assert_eq!(
            typed,
            "Something went wrong while talking to the model. This is usually temporary — try again. (HTTP 502 · server_error)"
        );
        assert_eq!(
            empty,
            "Something went wrong while talking to the model. This is usually temporary — try again. (HTTP 502)"
        );
    }

    #[test]
    fn classifies_transient_upstream_http_statuses_for_retry() {
        for status in [400, 408, 409, 425, 500, 502, 599] {
            let error = upstream_http_chat_error(
                StatusCode::from_u16(status).expect("test status should be valid"),
                r#"{"error":{"type":"server_error"}}"#,
                String::new(),
            );

            assert!(error.retryable, "HTTP {status} should be retryable");
            assert_eq!(
                stream_retry_delay(0, &error, StreamAttemptProgress::default()),
                Some(Duration::from_millis(500))
            );
        }
    }

    #[test]
    fn keeps_auth_and_usage_limit_upstream_http_errors_terminal() {
        for status in [401, 403, 429] {
            let error = upstream_http_chat_error(
                StatusCode::from_u16(status).expect("test status should be valid"),
                r#"{"error":{"type":"rate_limit_error"}}"#,
                String::new(),
            );

            assert!(!error.retryable, "HTTP {status} should not be retryable");
            assert_eq!(
                stream_retry_delay(0, &error, StreamAttemptProgress::default()),
                None
            );
        }

        let usage_limit_400 = upstream_http_chat_error(
            StatusCode::BAD_REQUEST,
            r#"{"error":{"type":"usage_limit_reached"}}"#,
            String::new(),
        );
        assert!(!usage_limit_400.retryable);
    }

    #[test]
    fn retryable_upstream_http_errors_preserve_partial_text() {
        let error = upstream_http_chat_error(
            StatusCode::BAD_GATEWAY,
            r#"{"error":{"type":"server_error"}}"#,
            "partial response".to_string(),
        );

        assert!(error.retryable);
        assert_eq!(error.partial_text, "partial response");
        assert_eq!(
            stream_retry_delay(0, &error, StreamAttemptProgress::default()),
            None
        );
    }

    #[test]
    fn persisted_upstream_error_log_scrubs_secrets_and_truncates_bodies() {
        let app_data_dir = std::env::temp_dir().join(format!(
            "halv-upstream-error-log-{}",
            storage::new_uuid_v4()
        ));
        let body = format!(
            r#"{{"error":{{"type":"invalid_request_error","authorization":"Bearer auth-secret","api_key":"api-secret","message":"token sk-live-secret {}"}}}}"#,
            "x".repeat(UPSTREAM_ERROR_LOG_BODY_MAX_CHARS + 100)
        );
        let line =
            format_upstream_error_log_line("2026-08-11T12:00:00Z", StatusCode::BAD_REQUEST, &body);

        append_upstream_error_log(&app_data_dir, &line)
            .expect("scrubbed upstream error should be persisted");
        let persisted =
            fs::read_to_string(app_data_dir.join("logs").join(UPSTREAM_ERROR_LOG_FILE_NAME))
                .expect("persisted upstream error log should be readable");

        assert!(
            persisted.starts_with("2026-08-11T12:00:00Z HTTP 400 type=invalid_request_error body=")
        );
        assert!(persisted.contains("[REDACTED]"));
        assert!(persisted.contains("[truncated by Halv]"));
        assert!(!persisted.contains("auth-secret"));
        assert!(!persisted.contains("api-secret"));
        assert!(!persisted.contains("sk-live-secret"));
        assert!(persisted.len() < UPSTREAM_ERROR_LOG_MAX_BYTES as usize);

        fs::remove_dir_all(app_data_dir)
            .expect("upstream error log test directory should clean up");
    }

    #[test]
    fn upstream_error_log_truncates_existing_file_at_the_size_cap() {
        let app_data_dir = std::env::temp_dir().join(format!(
            "halv-upstream-error-cap-{}",
            storage::new_uuid_v4()
        ));
        let logs_dir = app_data_dir.join("logs");
        fs::create_dir_all(&logs_dir).expect("test logs directory should be created");
        let path = logs_dir.join(UPSTREAM_ERROR_LOG_FILE_NAME);
        fs::write(&path, vec![b'x'; UPSTREAM_ERROR_LOG_MAX_BYTES as usize])
            .expect("oversized test log should be written");

        append_upstream_error_log(&app_data_dir, "new bounded record")
            .expect("upstream error log should reset at its cap");
        let persisted = fs::read_to_string(&path).expect("bounded log should be readable");

        assert_eq!(persisted, "new bounded record\n");
        assert!(persisted.len() <= UPSTREAM_ERROR_LOG_MAX_BYTES as usize);

        fs::remove_dir_all(app_data_dir)
            .expect("upstream error cap test directory should clean up");
    }

    #[test]
    fn transient_stream_failure_with_partial_text_does_not_retry() {
        let error = ChatError::transient_with_partial(
            "SSE stream failed: connection reset",
            "partial response".to_string(),
        );

        assert_eq!(
            stream_retry_delay(0, &error, StreamAttemptProgress::default()),
            None
        );
    }

    #[test]
    fn transient_stream_failure_after_emitted_delta_does_not_retry() {
        let error = ChatError::transient("SSE stream failed: connection reset");
        let progress = StreamAttemptProgress {
            any_delta_emitted: true,
            ..StreamAttemptProgress::default()
        };

        assert_eq!(stream_retry_delay(0, &error, progress), None);
    }

    #[test]
    fn transient_stream_failure_after_parsed_tool_call_does_not_retry() {
        let error = ChatError::transient("SSE stream failed: connection reset");
        let progress = StreamAttemptProgress {
            tool_call_parsed: true,
            ..StreamAttemptProgress::default()
        };

        assert_eq!(stream_retry_delay(0, &error, progress), None);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn retry_backoff_aborts_promptly_when_cancelled() {
        let cancel = Arc::new(AtomicBool::new(false));
        let cancel_signal = cancel.clone();
        let signal = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(10)).await;
            cancel_signal.store(true, Ordering::SeqCst);
        });

        let result = tokio::time::timeout(
            Duration::from_secs(1),
            wait_for_stream_retry_backoff(Duration::from_secs(5), cancel),
        )
        .await
        .expect("cancellation should interrupt the five-second backoff");
        signal.await.expect("cancellation task should finish");

        let error = result.expect_err("backoff should return cancellation");
        assert_eq!(error.message, "Chat request cancelled");
        assert!(!error.retryable);
    }

    #[test]
    fn stream_request_sets_headroom_compression_headers() {
        let request =
            add_headroom_compression_headers(Client::new().post("http://127.0.0.1/v1/responses"))
                .build()
                .expect("request should build");

        assert_eq!(
            request
                .headers()
                .get("x-headroom-compress-messages")
                .and_then(|value| value.to_str().ok()),
            Some("true")
        );
        assert_eq!(
            request
                .headers()
                .get("x-headroom-protect-recent")
                .and_then(|value| value.to_str().ok()),
            Some("0")
        );
    }

    #[test]
    fn builds_claude_request_headers_for_each_request_shape() {
        let base = build_claude_request_headers("anthropic-key")
            .expect("Claude request headers should build");
        assert_eq!(base.len(), 2);
        assert_eq!(
            base.get("x-api-key").and_then(|value| value.to_str().ok()),
            Some("anthropic-key")
        );
        assert_eq!(
            base.get("anthropic-version")
                .and_then(|value| value.to_str().ok()),
            Some("2023-06-01")
        );

        let model_headers = base.clone();
        assert_eq!(model_headers.len(), 2);

        let mut title_headers = base.clone();
        title_headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        assert_eq!(title_headers.len(), 3);
        assert_eq!(
            title_headers
                .get(CONTENT_TYPE)
                .and_then(|value| value.to_str().ok()),
            Some("application/json")
        );

        let mut stream_headers = base;
        stream_headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        stream_headers.insert(ACCEPT, HeaderValue::from_static("text/event-stream"));
        assert_eq!(stream_headers.len(), 4);
        assert_eq!(
            stream_headers
                .get(CONTENT_TYPE)
                .and_then(|value| value.to_str().ok()),
            Some("application/json")
        );
        assert_eq!(
            stream_headers
                .get(ACCEPT)
                .and_then(|value| value.to_str().ok()),
            Some("text/event-stream")
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn pinned_disabled_profile_chat_error_names_profile_and_no_fallback() {
        let path = std::env::temp_dir().join(format!(
            "halv-chat-credential-profile-{}.sqlite3",
            storage::new_uuid_v4()
        ));
        let storage = Storage::open(&path).expect("test storage should open");
        let profile = storage::CredentialProfile {
            id: "disabled-profile".to_string(),
            provider: "openai".to_string(),
            name: "Disabled team profile".to_string(),
            kind: "api_key".to_string(),
            priority: 0,
            status: "disabled".to_string(),
            last_validated_at: None,
            metadata_json: None,
            created_at: "2026-08-16T12:00:00Z".to_string(),
            updated_at: "2026-08-16T12:00:00Z".to_string(),
        };
        storage
            .insert_credential_profile(&profile)
            .expect("disabled profile should insert");
        let conversation_id = storage
            .ensure_conversation(None, "Pinned profile", DEFAULT_MODEL, None)
            .expect("conversation should create");
        storage
            .set_conversation_credential_profile(&conversation_id, Some(profile.id))
            .expect("credential profile should pin");
        let credential_profile_id = storage
            .conversation_credential_profile(&conversation_id)
            .expect("credential profile pin should read");

        let error = resolve_openai_bearer(&storage, credential_profile_id.as_deref())
            .await
            .expect_err("disabled profile should fail before network access");

        assert!(error.contains("Disabled team profile"));
        assert!(error.contains("No fallback credential was used."));
        drop(storage);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn builds_chatgpt_request_headers_and_body_exactly() {
        let headers = build_request_headers(
            &chatgpt_bearer(),
            "22222222-2222-4222-8222-222222222222",
            "11111111-1111-4111-8111-111111111111",
        )
        .expect("headers should build");
        assert_eq!(
            headers
                .get(AUTHORIZATION)
                .and_then(|value| value.to_str().ok()),
            Some("Bearer chatgpt-token")
        );
        assert_eq!(
            headers
                .get("chatgpt-account-id")
                .and_then(|value| value.to_str().ok()),
            Some("acct_123")
        );
        assert_eq!(
            headers
                .get("originator")
                .and_then(|value| value.to_str().ok()),
            Some(ORIGINATOR)
        );
        assert_eq!(
            headers
                .get("session-id")
                .and_then(|value| value.to_str().ok()),
            Some("22222222-2222-4222-8222-222222222222")
        );
        assert_eq!(
            headers
                .get("thread-id")
                .and_then(|value| value.to_str().ok()),
            Some("11111111-1111-4111-8111-111111111111")
        );
        assert_eq!(
            headers
                .get("x-client-request-id")
                .and_then(|value| value.to_str().ok()),
            Some("11111111-1111-4111-8111-111111111111")
        );
        assert_eq!(
            headers.get(ACCEPT).and_then(|value| value.to_str().ok()),
            Some("text/event-stream")
        );
        assert_eq!(
            headers
                .get(CONTENT_TYPE)
                .and_then(|value| value.to_str().ok()),
            Some("application/json")
        );
        assert!(headers.get("openai-beta").is_none());

        let body = build_responses_request_body(
            "11111111-1111-4111-8111-111111111111",
            &[stored_message("user", "Hello")],
            "gpt-5.5",
            "medium",
            None,
            None,
            None,
            false,
            false,
            exec::ApprovalPolicy::ReadOnly,
        );
        assert_eq!(
            body,
            json!({
                "model": "gpt-5.5",
                "input": [{
                    "type": "message",
                    "role": "user",
                    "content": [{
                        "type": "input_text",
                        "text": "Hello"
                    }]
                }],
                "tools": [{ "type": "web_search" }],
                "tool_choice": "auto",
                "parallel_tool_calls": true,
                "reasoning": {
                    "effort": "medium",
                    "summary": "auto"
                },
                "store": false,
                "stream": true,
                "include": ["reasoning.encrypted_content", "web_search_call.action.sources"],
                "prompt_cache_key": "11111111-1111-4111-8111-111111111111",
                "instructions": format!("{CHAT_MODE_ENVIRONMENT_INSTRUCTIONS}\n\n{WEB_SEARCH_ENVIRONMENT_INSTRUCTIONS}"),
                "text": {
                    "verbosity": "low"
                },
                "client_metadata": {}
            })
        );
        assert!(body.get("temperature").is_none());
        assert!(body.get("top_p").is_none());
        assert!(body.get("max_output_tokens").is_none());
        assert!(body.get("service_tier").is_none());
        assert!(
            body.get("instructions")
                .and_then(Value::as_str)
                .is_some_and(
                    |instructions| instructions.contains(WEB_SEARCH_ENVIRONMENT_INSTRUCTIONS)
                )
        );
    }

    #[test]
    fn title_generation_request_is_non_streaming_low_effort_and_truncated() {
        let long_user_message = "a".repeat(TITLE_GENERATION_INPUT_CHARS + 64);
        let body = build_title_generation_request_body(
            "11111111-1111-4111-8111-111111111111",
            "gpt-5.5",
            &long_user_message,
        );

        assert_eq!(body.get("model").and_then(Value::as_str), Some("gpt-5.5"));
        assert_eq!(body.get("stream").and_then(Value::as_bool), Some(false));
        assert_eq!(
            body.get("instructions").and_then(Value::as_str),
            Some(TITLE_GENERATION_INSTRUCTIONS)
        );
        assert_eq!(
            body.pointer("/reasoning/effort").and_then(Value::as_str),
            Some("low")
        );
        assert_eq!(
            body.get("max_output_tokens").and_then(Value::as_i64),
            Some(TITLE_GENERATION_MAX_OUTPUT_TOKENS)
        );
        let input_text = body
            .pointer("/input/0/content/0/text")
            .and_then(Value::as_str)
            .expect("title input should exist");
        assert_eq!(input_text.chars().count(), TITLE_GENERATION_INPUT_CHARS);
        assert_eq!(
            body.pointer("/client_metadata/purpose")
                .and_then(Value::as_str),
            Some("conversation_title")
        );
    }

    #[test]
    fn title_sanitization_strips_quotes_newlines_and_caps_length() {
        assert_eq!(
            sanitize_generated_title("  \"Build Search UX\"\n"),
            Some("Build Search UX".to_string())
        );
        assert_eq!(
            sanitize_generated_title("`First line\nSecond line`"),
            Some("First line Second line".to_string())
        );
        assert_eq!(sanitize_generated_title(" \n\t "), None);

        let long_title = "a".repeat(TITLE_GENERATION_MAX_CHARS + 8);
        let sanitized = sanitize_generated_title(&long_title).expect("title should sanitize");
        assert_eq!(
            sanitized.chars().count(),
            TITLE_GENERATION_MAX_CHARS + "...".chars().count()
        );
        assert!(sanitized.ends_with("..."));
    }

    #[test]
    fn title_generation_seed_uses_first_user_only_before_successful_assistant() {
        let long_user_message = "u".repeat(TITLE_GENERATION_INPUT_CHARS + 1);
        let seed = title_generation_user_message(&[stored_message("user", &long_user_message)])
            .expect("first user message should seed title generation");
        assert_eq!(seed.chars().count(), TITLE_GENERATION_INPUT_CHARS);

        let messages = vec![
            stored_message("user", "First prompt"),
            stored_message("assistant", "Successful answer"),
        ];
        assert_eq!(title_generation_user_message(&messages), None);

        let mut errored_assistant = stored_message("assistant", "Failed answer");
        errored_assistant.error = true;
        let messages = vec![stored_message("user", "Retry prompt"), errored_assistant];
        assert_eq!(
            title_generation_user_message(&messages).as_deref(),
            Some("Retry prompt")
        );
    }

    #[test]
    fn agent_request_includes_shell_tool_schema() {
        let body = build_responses_request_body(
            "11111111-1111-4111-8111-111111111111",
            &[stored_message("user", "List files")],
            "gpt-5.5",
            "medium",
            None,
            None,
            None,
            true,
            false,
            exec::ApprovalPolicy::ReadOnly,
        );

        let tools = body
            .get("tools")
            .and_then(Value::as_array)
            .expect("tools should be an array");
        assert_eq!(tools.first(), Some(&json!({ "type": "web_search" })));
        assert_eq!(tools.get(1), Some(&exec::shell_command_tool_schema()));
        let names = tools
            .iter()
            .filter_map(|tool| tool.get("name").and_then(Value::as_str))
            .collect::<Vec<_>>();
        assert!(names.contains(&"update_plan"));
        assert!(names.contains(&"view_image"));
        assert!(names.contains(&"spawn_agent"));
        assert!(names.contains(&"wait_agent"));
        assert!(names.contains(&"list_agents"));
        assert_eq!(
            body.get("tool_choice").and_then(Value::as_str),
            Some("auto")
        );
        assert_eq!(body.get("store").and_then(Value::as_bool), Some(false));
        assert_eq!(
            body.get("include"),
            Some(&json!([
                "reasoning.encrypted_content",
                "web_search_call.action.sources"
            ]))
        );
    }

    #[test]
    fn worker_profile_omits_coordination_tools_to_prevent_nested_agents() {
        let body = build_responses_request_body_for_profile(
            "worker-thread",
            vec![user_input_message("Inspect the project")],
            "gpt-5.5",
            "medium",
            None,
            true,
            false,
            exec::ApprovalPolicy::ReadOnly,
            &[],
            false,
        );
        let names = body["tools"]
            .as_array()
            .expect("tools should be present")
            .iter()
            .filter_map(|tool| tool.get("name").and_then(Value::as_str))
            .collect::<Vec<_>>();
        assert!(names.contains(&"shell_command"));
        assert!(names.contains(&"view_image"));
        assert!(names.contains(&"browser_snapshot"));
        assert!(names.contains(&"browser_screenshot"));
        assert!(!names.contains(&"update_plan"));
        assert!(!names.contains(&"spawn_agent"));
        assert!(!names.contains(&"wait_agent"));
    }

    #[test]
    fn update_plan_emits_structured_task_list_and_rejects_parallel_active_steps() {
        let emitter = RecordingEmitter::default();
        let db_path =
            std::env::temp_dir().join(format!("halv-plan-tool-{}.sqlite", storage::new_uuid_v4()));
        let storage = Storage::open(&db_path).expect("storage should open");
        storage
            .ensure_conversation(
                Some("conversation-1".to_string()),
                "Plan this work",
                DEFAULT_MODEL,
                None,
            )
            .expect("conversation should exist");
        let valid = ToolCallEvent {
            call_id: "plan-1".to_string(),
            name: "update_plan".to_string(),
            input: json!({
                "explanation": "Implementation started",
                "plan": [
                    { "step": "Inspect", "status": "completed" },
                    { "step": "Implement", "status": "in_progress" }
                ]
            })
            .to_string(),
            kind: ToolCallKind::Function,
            item: Value::Null,
        };
        let output = execute_update_plan_call(&emitter, &storage, "conversation-1", valid)
            .expect("valid plan should execute");
        assert_eq!(output["output"], "Plan updated");
        let payloads = emitter.payloads.lock().expect("payload lock");
        assert_eq!(payloads.len(), 1);
        assert_eq!(payloads[0].kind, ChatDeltaKind::TaskList);
        assert_eq!(payloads[0].plan.as_ref().map(Vec::len), Some(2));
        drop(payloads);
        let persisted = storage
            .conversation_task_plan("conversation-1")
            .expect("plan should load")
            .expect("plan should exist");
        assert_eq!(persisted.plan.len(), 2);
        assert_eq!(
            persisted.explanation.as_deref(),
            Some("Implementation started")
        );

        let invalid = ToolCallEvent {
            call_id: "plan-2".to_string(),
            name: "update_plan".to_string(),
            input: json!({
                "plan": [
                    { "step": "One", "status": "in_progress" },
                    { "step": "Two", "status": "in_progress" }
                ]
            })
            .to_string(),
            kind: ToolCallKind::Function,
            item: Value::Null,
        };
        let output = execute_update_plan_call(&emitter, &storage, "conversation-1", invalid)
            .expect("invalid plan should return a model-visible result");
        assert!(output["output"]
            .as_str()
            .is_some_and(|value| value.contains("at most one")));
        assert_eq!(emitter.payloads.lock().expect("payload lock").len(), 1);
        drop(storage);
        let _ = std::fs::remove_file(db_path);
    }

    #[test]
    fn subagent_names_and_fork_modes_are_strict() {
        assert!(validate_subagent_task_name("api_tests_2").is_ok());
        assert!(validate_subagent_task_name("API-tests").is_err());
        assert_eq!(normalize_fork_turns(None).as_deref(), Ok("all"));
        assert_eq!(normalize_fork_turns(Some("3")).as_deref(), Ok("3"));
        assert!(normalize_fork_turns(Some("0")).is_err());
    }

    #[test]
    fn subagent_wait_defaults_to_long_wait_and_preserves_explicit_deadlines() {
        let omitted: WaitAgentArgs = serde_json::from_str("{}").unwrap();
        assert_eq!(omitted.timeout().unwrap(), Duration::from_secs(3600));
        for millis in [10_000, 60_000, 3_600_000] {
            let args: WaitAgentArgs =
                serde_json::from_value(json!({"timeout_ms": millis})).unwrap();
            assert_eq!(args.timeout().unwrap(), Duration::from_millis(millis));
        }
        for millis in [0, 9999, 3_600_001, u64::MAX] {
            assert!(WaitAgentArgs {
                timeout_ms: Some(millis)
            }
            .timeout()
            .is_err());
        }
        let schema = subagent_tool_schemas()
            .into_iter()
            .find(|tool| tool["name"] == "wait_agent")
            .unwrap();
        assert_eq!(
            schema["parameters"]["properties"]["timeout_ms"]["default"],
            3_600_000
        );
    }

    #[test]
    fn subagent_scheduler_only_has_permits_when_a_limit_is_configured() {
        assert!(subagent_semaphore(None).is_none());

        let semaphore = subagent_semaphore(Some(3)).expect("configured scheduler");
        assert_eq!(semaphore.available_permits(), 3);
        let first = semaphore.clone().try_acquire_owned().expect("first permit");
        let second = semaphore
            .clone()
            .try_acquire_owned()
            .expect("second permit");
        let third = semaphore.clone().try_acquire_owned().expect("third permit");
        assert!(semaphore.clone().try_acquire_owned().is_err());
        drop((first, second, third));
        assert_eq!(semaphore.available_permits(), 3);
    }

    #[test]
    fn write_policy_request_includes_apply_patch_custom_tool_schema() {
        let body = build_responses_request_body(
            "11111111-1111-4111-8111-111111111111",
            &[stored_message("user", "Edit files")],
            "gpt-5.5",
            "medium",
            None,
            None,
            None,
            true,
            false,
            exec::ApprovalPolicy::OnRequest,
        );
        let tools = body
            .get("tools")
            .and_then(Value::as_array)
            .expect("tools should be an array");
        assert_eq!(tools[0], json!({ "type": "web_search" }));
        assert_eq!(tools[1], exec::shell_command_tool_schema());
        let apply_patch = tools
            .iter()
            .find(|tool| tool.get("name").and_then(Value::as_str) == Some("apply_patch"))
            .expect("apply_patch should be present");
        assert_eq!(apply_patch, &exec::apply_patch::apply_patch_tool_schema());
        assert_eq!(
            apply_patch.get("type").and_then(Value::as_str),
            Some("custom")
        );
    }

    #[test]
    fn chat_mode_without_goal_includes_environment_instructions_and_web_search() {
        let body = build_responses_request_body(
            "11111111-1111-4111-8111-111111111111",
            &[stored_message("user", "Explain this code")],
            "gpt-5.5",
            "medium",
            None,
            None,
            None,
            false,
            false,
            exec::ApprovalPolicy::ReadOnly,
        );

        let instructions = body
            .get("instructions")
            .and_then(Value::as_str)
            .expect("instructions should always be present");
        assert!(instructions.contains("inside Halv in Chat only mode"));
        assert!(instructions.contains("choose Sandbox or Ask in the chat bar"));
        assert!(!instructions.contains("standing goal"));
        assert!(!instructions.contains(AGENT_MODE_ENVIRONMENT_INSTRUCTIONS));
        assert!(instructions.contains(WEB_SEARCH_ENVIRONMENT_INSTRUCTIONS));
        assert_eq!(body["tools"], json!([{ "type": "web_search" }]));
    }

    #[test]
    fn personality_directives_extend_provider_system_instructions() {
        let mut responses = json!({ "instructions": "Base instructions" });
        apply_personality_directive(&mut responses, Personality::Friendly);
        assert_eq!(
            responses["instructions"],
            "Base instructions\n\nBe warm, encouraging, and conversational in your explanations."
        );

        let mut claude = json!({ "system": [{ "type": "text", "text": "Base instructions" }] });
        apply_personality_directive(&mut claude, Personality::Pragmatic);
        assert_eq!(
            claude["system"][0]["text"],
            "Base instructions\n\nBe terse, avoid filler, and lead with actions and outcomes."
        );

        let unchanged = responses.clone();
        apply_personality_directive(&mut responses, Personality::None);
        assert_eq!(responses, unchanged);
    }

    #[test]
    fn ponytail_directives_extend_provider_system_instructions() {
        let directive = ponytail::instructions(PonytailMode::Full).expect("full instructions");

        let mut responses = json!({ "instructions": "Base instructions" });
        apply_instruction_directive(&mut responses, &directive);
        assert!(responses["instructions"]
            .as_str()
            .is_some_and(|text| text.contains("PONYTAIL MODE ACTIVE — level: full")));

        let mut claude = json!({ "system": [{ "type": "text", "text": "Base instructions" }] });
        apply_instruction_directive(&mut claude, &directive);
        assert!(claude["system"][0]["text"]
            .as_str()
            .is_some_and(|text| text.contains("PONYTAIL MODE ACTIVE — level: full")));

        let unchanged = responses.clone();
        if let Some(text) = ponytail::instructions(PonytailMode::Off) {
            apply_instruction_directive(&mut responses, &text);
        }
        assert_eq!(responses, unchanged);
    }

    #[test]
    fn memories_effective_enablement_respects_global_preference_and_mode() {
        for (global_enabled, mode, expected) in [
            (false, None, false),
            (false, Some("off"), false),
            (false, Some("on"), false),
            (true, None, true),
            (true, Some("off"), false),
            (true, Some("on"), true),
        ] {
            assert_eq!(
                memories_effectively_enabled(global_enabled, mode),
                expected,
                "global={global_enabled} mode={mode:?}"
            );
        }
    }

    #[test]
    fn shared_memory_generation_entry_point_skips_a_disabled_conversation() {
        let _entry_point: fn(&AppHandle, &str, &str, &str, &str, &str) =
            schedule_agent_memory_generation_for_turn;
        let path = std::env::temp_dir().join(format!(
            "halv-disabled-memory-generation-{}.sqlite3",
            storage::new_uuid_v4()
        ));
        let storage = Storage::open(&path).expect("test storage should open");
        let conversation_id = storage
            .ensure_conversation(None, "Do not remember", DEFAULT_MODEL, None)
            .expect("conversation should create");
        storage
            .set_conversation_memories_mode(&conversation_id, Some("off"))
            .expect("conversation memories should disable");
        let preferences = TestPreferences::new("disabled-memory-generation");

        let context = agent_memory_generation_context(&preferences, &storage, &conversation_id)
            .expect("memory generation state should load");

        assert!(context.is_none());
        drop(storage);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn memory_directives_extend_both_provider_instruction_shapes() {
        let directive =
            "## Memories\n\nMemories are background context that may be outdated.\n- Use pnpm.";
        let mut responses = json!({ "instructions": "Base instructions" });
        apply_instruction_directive(&mut responses, directive);
        assert_eq!(
            responses["instructions"],
            format!("Base instructions\n\n{directive}")
        );

        let mut claude = json!({ "system": [{ "type": "text", "text": "Base instructions" }] });
        apply_instruction_directive(&mut claude, directive);
        assert_eq!(
            claude["system"][0]["text"],
            format!("Base instructions\n\n{directive}")
        );
    }

    #[test]
    fn formatted_memory_instructions_obey_the_hard_character_cap() {
        let memories = vec![storage::AgentMemory {
            id: 1,
            project_id: None,
            project_name: None,
            content: "x".repeat(MEMORY_CONTEXT_MAX_CHARS * 2),
            source_conversation_id: None,
            created_at: 1,
            updated_at: 1,
        }];
        let instructions =
            format_memory_instructions(&memories).expect("memory instructions should exist");
        assert!(instructions.starts_with("## Memories"));
        assert!(instructions.contains(MEMORY_CONTEXT_CAVEAT));
        assert_eq!(instructions.chars().count(), MEMORY_CONTEXT_MAX_CHARS);
    }

    #[test]
    fn generated_memory_parser_caps_facts_and_refuses_sensitive_material() {
        let parsed = parse_generated_memories(
            r#"["Use pnpm.", "API key is sk-secret", "Keep tests serialized.", "Use pnpm.", "Fourth fact."]"#,
        )
        .expect("memory JSON should parse");
        assert_eq!(
            parsed,
            vec![
                "Use pnpm.".to_string(),
                "Keep tests serialized.".to_string(),
                "Fourth fact.".to_string()
            ]
        );
    }

    #[test]
    fn plan_mode_is_read_only_and_exposes_only_inspection_and_plan_tools() {
        let chat_only = build_responses_request_body(
            "11111111-1111-4111-8111-111111111111",
            &[stored_message("user", "Plan the refactor")],
            "gpt-5.5",
            "high",
            Some("Ship the refactor"),
            None,
            None,
            false,
            true,
            exec::ApprovalPolicy::ReadOnly,
        );
        let instructions = chat_only["instructions"]
            .as_str()
            .expect("instructions should exist");
        assert!(instructions.contains(PLAN_MODE_INSTRUCTIONS));
        assert!(instructions.contains("standing goal is the planning objective"));
        assert!(!instructions.contains(GOAL_PROTOCOL_INSTRUCTIONS));
        let names = chat_only["tools"]
            .as_array()
            .expect("tools should exist")
            .iter()
            .filter_map(|tool| tool.get("name").and_then(Value::as_str))
            .collect::<Vec<_>>();
        assert_eq!(names, vec!["update_plan"]);

        let crux = extensions::McpToolDescriptor {
            server_id: "builtin.crux".to_string(),
            server_name: "Crux code navigation".to_string(),
            name: "scip_map".to_string(),
            wire_name: "mcp__builtin-crux__scip_map__123".to_string(),
            description: "Map indexed symbols".to_string(),
            input_schema: json!({ "type": "object" }),
            approval_mode: "approve".to_string(),
        };
        let agent = build_responses_request_body_from_input_with_mcp(
            "11111111-1111-4111-8111-111111111111",
            build_response_input_items(&[stored_message("user", "Inspect and plan")], None, None),
            "gpt-5.5",
            "high",
            None,
            true,
            true,
            exec::ApprovalPolicy::ReadOnly,
            &[crux],
        );
        let names = agent["tools"]
            .as_array()
            .expect("tools should exist")
            .iter()
            .filter_map(|tool| tool.get("name").and_then(Value::as_str))
            .collect::<Vec<_>>();
        assert_eq!(
            names,
            vec![
                "shell_command",
                "view_image",
                "update_plan",
                "mcp__builtin-crux__scip_map__123"
            ]
        );
        assert!(!names.contains(&"apply_patch"));
        assert!(!names.contains(&"spawn_agent"));
    }

    #[test]
    fn chat_mode_with_goal_includes_blocking_goal_protocol_and_web_search() {
        let body = build_responses_request_body(
            "11111111-1111-4111-8111-111111111111",
            &[stored_message("user", "Ship the UI")],
            "gpt-5.5",
            "high",
            Some(" Keep the response focused on TASK UI-2 "),
            None,
            None,
            false,
            false,
            exec::ApprovalPolicy::ReadOnly,
        );

        let instructions = body
            .get("instructions")
            .and_then(Value::as_str)
            .expect("instructions should always be present");
        assert!(instructions.contains(CHAT_MODE_ENVIRONMENT_INSTRUCTIONS));
        assert!(instructions.contains(
            "The user's standing goal for this conversation: Keep the response focused on TASK UI-2"
        ));
        assert!(instructions.contains(GOAL_CHAT_MODE_INSTRUCTIONS));
        assert!(instructions.contains(GOAL_BLOCKED_MARKER));
        assert!(!instructions.contains(AGENT_MODE_ENVIRONMENT_INSTRUCTIONS));
        assert!(!instructions.contains("keep working toward it autonomously"));
        assert!(instructions.contains(WEB_SEARCH_ENVIRONMENT_INSTRUCTIONS));
        assert_eq!(body["tools"], json!([{ "type": "web_search" }]));
        assert_eq!(
            body.get("input")
                .and_then(Value::as_array)
                .and_then(|items| items.first())
                .and_then(|item| item.get("content"))
                .and_then(Value::as_array)
                .and_then(|content| content.first())
                .and_then(|part| part.get("text"))
                .and_then(Value::as_str),
            Some("Ship the UI")
        );
    }

    #[test]
    fn agent_mode_without_goal_includes_environment_instructions_and_shell_tool() {
        let body = build_responses_request_body(
            "11111111-1111-4111-8111-111111111111",
            &[stored_message("user", "List files")],
            "gpt-5.5",
            "medium",
            None,
            None,
            None,
            true,
            false,
            exec::ApprovalPolicy::ReadOnly,
        );

        let instructions = body
            .get("instructions")
            .and_then(Value::as_str)
            .expect("instructions should always be present");
        assert!(instructions.contains(AGENT_MODE_ENVIRONMENT_INSTRUCTIONS));
        assert!(!instructions.contains(CHAT_MODE_ENVIRONMENT_INSTRUCTIONS));
        assert!(!instructions.contains("standing goal"));
        assert!(!instructions.contains(GOAL_BLOCKED_MARKER));
        let tools = body
            .get("tools")
            .and_then(Value::as_array)
            .expect("tools should be an array");
        assert_eq!(tools.first(), Some(&json!({ "type": "web_search" })));
        assert_eq!(tools.get(1), Some(&exec::shell_command_tool_schema()));
        assert!(tools
            .iter()
            .any(|tool| tool.get("name") == Some(&json!("update_plan"))));
        assert!(tools
            .iter()
            .any(|tool| tool.get("name") == Some(&json!("spawn_agent"))));
    }

    #[test]
    fn agent_mode_with_goal_includes_autonomous_goal_protocol_and_shell_tool() {
        let body = build_responses_request_body(
            "11111111-1111-4111-8111-111111111111",
            &[stored_message("user", "Ship the UI")],
            "gpt-5.5",
            "medium",
            Some("Ship TASK UI-2"),
            None,
            None,
            true,
            false,
            exec::ApprovalPolicy::ReadOnly,
        );

        let instructions = body
            .get("instructions")
            .and_then(Value::as_str)
            .expect("instructions should always be present");
        assert!(instructions.contains(AGENT_MODE_ENVIRONMENT_INSTRUCTIONS));
        assert!(
            instructions.contains("The user's standing goal for this conversation: Ship TASK UI-2")
        );
        assert!(instructions.contains(GOAL_PROTOCOL_INSTRUCTIONS));
        assert!(!instructions.contains(CHAT_MODE_ENVIRONMENT_INSTRUCTIONS));
        assert!(!instructions.contains(GOAL_CHAT_MODE_INSTRUCTIONS));
        assert!(!instructions.contains(GOAL_BLOCKED_MARKER));
        let tools = body
            .get("tools")
            .and_then(Value::as_array)
            .expect("tools should be an array");
        assert_eq!(tools.first(), Some(&json!({ "type": "web_search" })));
        assert_eq!(tools.get(1), Some(&exec::shell_command_tool_schema()));
        assert!(tools
            .iter()
            .any(|tool| tool.get("name") == Some(&json!("update_plan"))));
        assert!(tools
            .iter()
            .any(|tool| tool.get("name") == Some(&json!("spawn_agent"))));
    }

    #[test]
    fn builds_api_key_headers_without_chatgpt_headers() {
        let headers = build_request_headers(
            &api_key_bearer(),
            "22222222-2222-4222-8222-222222222222",
            "11111111-1111-4111-8111-111111111111",
        )
        .expect("headers should build");

        assert_eq!(
            headers
                .get(AUTHORIZATION)
                .and_then(|value| value.to_str().ok()),
            Some("Bearer sk-test")
        );
        assert_eq!(
            headers.get(ACCEPT).and_then(|value| value.to_str().ok()),
            Some("text/event-stream")
        );
        assert_eq!(
            headers
                .get(CONTENT_TYPE)
                .and_then(|value| value.to_str().ok()),
            Some("application/json")
        );
        assert!(headers.get("chatgpt-account-id").is_none());
        assert!(headers.get("originator").is_none());
        assert!(headers.get("session-id").is_none());
        assert!(headers.get("thread-id").is_none());
        assert!(headers.get("x-client-request-id").is_none());
        assert!(headers.get("openai-beta").is_none());
    }

    #[test]
    fn parses_function_call_items_and_strips_upstream_ids() {
        let event = parse_responses_event(json!({
            "type": "response.output_item.done",
            "item": {
                "id": "fc_123",
                "type": "function_call",
                "name": "shell_command",
                "call_id": "call_123",
                "arguments": "{\"command\":\"echo hi\"}"
            }
        }))
        .expect("function_call event should parse");

        assert_eq!(
            event,
            ParsedSseEvent::ToolCall(ToolCallEvent {
                call_id: "call_123".to_string(),
                name: "shell_command".to_string(),
                input: "{\"command\":\"echo hi\"}".to_string(),
                kind: ToolCallKind::Function,
                item: json!({
                    "type": "function_call",
                    "name": "shell_command",
                    "call_id": "call_123",
                    "arguments": "{\"command\":\"echo hi\"}"
                }),
            })
        );
    }

    #[test]
    fn parses_openai_web_search_activity_and_formats_clickable_citations() {
        let event = parse_responses_event(json!({
            "type": "response.output_item.done",
            "item": {
                "id": "ws_123",
                "type": "web_search_call",
                "status": "completed",
                "action": {
                    "type": "search",
                    "query": "latest Halv release",
                    "sources": [{
                        "type": "url",
                        "title": "Halv releases",
                        "url": "https://example.com/releases"
                    }]
                }
            }
        }))
        .expect("web search event should parse");

        assert_eq!(
            event,
            ParsedSseEvent::WebSearchCall {
                call_id: "ws_123".to_string(),
                command: "latest Halv release".to_string(),
                status: "completed".to_string(),
                item: Some(json!({
                    "type": "web_search_call",
                    "status": "completed",
                    "action": {
                        "type": "search",
                        "query": "latest Halv release",
                        "sources": [{
                            "type": "url",
                            "title": "Halv releases",
                            "url": "https://example.com/releases"
                        }]
                    }
                })),
                tail: Some("Halv releases\nhttps://example.com/releases".to_string()),
            }
        );

        let mut citations = Vec::new();
        collect_openai_web_citations(
            &json!({
                "type": "message",
                "content": [{
                    "type": "output_text",
                    "text": "Current information",
                    "annotations": [{
                        "type": "url_citation",
                        "title": "Halv [releases]",
                        "url": "https://example.com/releases"
                    }]
                }]
            }),
            &mut citations,
        );
        assert_eq!(
            web_citations_markdown(&citations),
            "\n\nSources:\n\n- [Halv \\[releases\\]](https://example.com/releases)"
        );
        let mut goal_response = "Done. [HALV_GOAL_DONE]".to_string();
        let suffix = web_citations_markdown(&citations);
        append_citation_suffix_preserving_goal_marker(&mut goal_response, &suffix);
        let (visible, signal) = assistant_text_without_goal_marker(&goal_response, Some("Ship"));
        assert_eq!(signal, Some("done"));
        assert!(visible.contains("Sources:"));
        assert!(!visible.contains(GOAL_DONE_MARKER));
    }

    #[test]
    fn parses_custom_tool_call_items_and_builds_custom_output_shape() {
        let event = parse_responses_event(json!({
            "type": "response.output_item.done",
            "item": {
                "id": "ctc_123",
                "type": "custom_tool_call",
                "name": "apply_patch",
                "call_id": "call_patch",
                "input": "*** Begin Patch\n*** Add File: hi.txt\n+hi\n*** End Patch\n"
            }
        }))
        .expect("custom_tool_call event should parse");

        let ParsedSseEvent::ToolCall(call) = event else {
            panic!("expected custom tool call");
        };
        assert_eq!(call.kind, ToolCallKind::Custom);
        assert_eq!(
            call.item,
            json!({
                "type": "custom_tool_call",
                "name": "apply_patch",
                "call_id": "call_patch",
                "input": "*** Begin Patch\n*** Add File: hi.txt\n+hi\n*** End Patch\n"
            })
        );
        assert_eq!(
            tool_call_output_item(&call, "{\"filesChanged\":1}"),
            json!({
                "type": "custom_tool_call_output",
                "call_id": "call_patch",
                "output": "{\"filesChanged\":1}"
            })
        );
    }

    #[test]
    fn function_call_output_replay_shape_is_full_history_input_item() {
        let mut input = vec![
            user_input_message("Run a read-only command"),
            json!({
                "type": "function_call",
                "name": "shell_command",
                "call_id": "call_123",
                "arguments": "{\"command\":\"echo hi\"}"
            }),
        ];
        input.push(function_call_output_item(
            "call_123",
            "Exit code: 0\nWall time: 0.001 seconds\n[Total output lines: 1]\nOutput:\nhi",
        ));
        let body = build_responses_request_body_from_input(
            "11111111-1111-4111-8111-111111111111",
            input.clone(),
            "gpt-5.5",
            "medium",
            None,
            true,
            false,
            exec::ApprovalPolicy::ReadOnly,
        );

        assert_eq!(body.get("input"), Some(&Value::Array(input)));
        assert_eq!(
            body.get("input")
                .and_then(Value::as_array)
                .and_then(|items| items.last()),
            Some(&json!({
                "type": "function_call_output",
                "call_id": "call_123",
                "output": "Exit code: 0\nWall time: 0.001 seconds\n[Total output lines: 1]\nOutput:\nhi"
            }))
        );
    }

    #[test]
    fn mcp_image_results_replay_to_openai_and_claude_as_vision_content() {
        let call = ToolCallEvent {
            call_id: "call_visual".to_string(),
            name: "mcp__computer_use__get_app_state".to_string(),
            input: "{}".to_string(),
            kind: ToolCallKind::Function,
            item: json!({}),
        };
        let item = mcp_tool_call_output_item(
            &call,
            &extensions::McpToolCallResult {
                text: "Window accessibility tree".to_string(),
                images: vec![extensions::McpToolImage {
                    media_type: "image/png".to_string(),
                    data: "iVBORw0KGgo=".to_string(),
                }],
                is_error: false,
            },
        );
        assert_eq!(
            item.pointer("/output/0/type").and_then(Value::as_str),
            Some("input_text")
        );
        assert_eq!(
            item.pointer("/output/1/image_url").and_then(Value::as_str),
            Some("data:image/png;base64,iVBORw0KGgo=")
        );
        let claude = tool_output_to_claude_content(&item);
        assert_eq!(
            claude.pointer("/0/type").and_then(Value::as_str),
            Some("text")
        );
        assert_eq!(
            claude
                .pointer("/1/source/media_type")
                .and_then(Value::as_str),
            Some("image/png")
        );
        assert_eq!(
            claude.pointer("/1/source/data").and_then(Value::as_str),
            Some("iVBORw0KGgo=")
        );
    }

    #[test]
    fn browser_screenshots_replay_to_openai_and_claude_as_vision_content() {
        let call = ToolCallEvent {
            call_id: "call_browser_visual".to_string(),
            name: "browser_screenshot".to_string(),
            input: "{}".to_string(),
            kind: ToolCallKind::Function,
            item: json!({}),
        };
        let item = browser_tool_call_output_item(
            &call,
            &browser::BrowserToolResult {
                text: "Captured the active tab.".to_string(),
                screenshot_png: Some("iVBORw0KGgo=".to_string()),
            },
        );
        assert_eq!(
            item.pointer("/output/1/image_url").and_then(Value::as_str),
            Some("data:image/png;base64,iVBORw0KGgo=")
        );
        assert_eq!(
            item.pointer("/output/1/detail").and_then(Value::as_str),
            Some("original")
        );
        let claude = tool_output_to_claude_content(&item);
        assert_eq!(
            claude.pointer("/1/type").and_then(Value::as_str),
            Some("image")
        );
        assert_eq!(
            claude
                .pointer("/1/source/media_type")
                .and_then(Value::as_str),
            Some("image/png")
        );
    }

    #[test]
    fn view_image_rejects_unsupported_extensions_and_oversized_files() {
        let root = std::env::temp_dir().join(format!("halv-view-image-{}", storage::new_uuid_v4()));
        std::fs::create_dir_all(&root).expect("project root should create");
        std::fs::write(root.join("notes.txt"), b"not an image")
            .expect("unsupported file should create");
        let extension_error = load_local_image(&root, "notes.txt")
            .expect_err("unsupported extension should be rejected");
        assert!(extension_error.contains("unsupported image extension"));

        let oversized = root.join("oversized.png");
        let file = std::fs::File::create(&oversized).expect("oversized image should create");
        file.set_len((MAX_VIEW_IMAGE_BYTES + 1) as u64)
            .expect("oversized image length should set");
        let size_error = load_local_image(&root, "oversized.png")
            .expect_err("oversized image should be rejected");
        assert!(size_error.contains("exceeds the 5 MiB limit"));

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn view_image_output_replays_as_openai_and_claude_vision_content() {
        let root =
            std::env::temp_dir().join(format!("halv-view-image-replay-{}", storage::new_uuid_v4()));
        std::fs::create_dir_all(&root).expect("project root should create");
        std::fs::write(
            root.join("pixel.png"),
            [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a],
        )
        .expect("image should create");
        let image = load_local_image(&root, "pixel.png").expect("image should load");
        let call = ToolCallEvent {
            call_id: "call_view_image".to_string(),
            name: "view_image".to_string(),
            input: json!({ "path": "pixel.png" }).to_string(),
            kind: ToolCallKind::Function,
            item: json!({
                "type": "function_call",
                "name": "view_image",
                "call_id": "call_view_image",
                "arguments": "{\"path\":\"pixel.png\"}"
            }),
        };
        let confirmation = format!("attached local image {}", image.path.display());
        let output = view_image_tool_call_output_item(&call, &image, &confirmation);
        let input = vec![call.item.clone(), output.clone()];
        let body = build_responses_request_body_from_input(
            "11111111-1111-4111-8111-111111111111",
            input.clone(),
            "gpt-5.5",
            "medium",
            None,
            true,
            false,
            exec::ApprovalPolicy::ReadOnly,
        );

        assert_eq!(body.get("input"), Some(&Value::Array(input)));
        assert_eq!(
            body.pointer("/input/1/output/0/text")
                .and_then(Value::as_str),
            Some(confirmation.as_str())
        );
        assert_eq!(
            body.pointer("/input/1/output/1/type")
                .and_then(Value::as_str),
            Some("input_image")
        );
        assert_eq!(
            body.pointer("/input/1/output/1/image_url")
                .and_then(Value::as_str),
            Some("data:image/png;base64,iVBORw0KGgo=")
        );
        let claude = tool_output_to_claude_content(&output);
        assert_eq!(
            claude
                .pointer("/1/source/media_type")
                .and_then(Value::as_str),
            Some("image/png")
        );
        assert_eq!(
            claude.pointer("/1/source/data").and_then(Value::as_str),
            Some("iVBORw0KGgo=")
        );

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn reasoning_output_item_survives_tool_loop_replay_without_upstream_id() {
        let reasoning = parse_responses_event(json!({
            "type": "response.output_item.done",
            "item": {
                "id": "rs_123",
                "type": "reasoning",
                "summary": [],
                "encrypted_content": "encrypted-reasoning"
            }
        }))
        .expect("reasoning item should parse");
        let function_call = parse_responses_event(json!({
            "type": "response.output_item.done",
            "item": {
                "id": "fc_123",
                "type": "function_call",
                "name": "shell_command",
                "call_id": "call_123",
                "arguments": "{\"command\":\"echo hi\"}"
            }
        }))
        .expect("function_call should parse");

        let mut input = vec![user_input_message("Run a read-only command")];
        match reasoning {
            ParsedSseEvent::OutputItem(item) => input.push(item),
            other => panic!("expected reasoning output item, got {other:?}"),
        }
        match function_call {
            ParsedSseEvent::ToolCall(call) => {
                let call_id = call.call_id.clone();
                input.push(call.item);
                input.push(function_call_output_item(
                    &call_id,
                    "Exit code: 0\nWall time: 0.001 seconds\n[Total output lines: 1]\nOutput:\nhi",
                ));
            }
            other => panic!("expected function call, got {other:?}"),
        }

        let body = build_responses_request_body_from_input(
            "11111111-1111-4111-8111-111111111111",
            input.clone(),
            "gpt-5.5",
            "medium",
            None,
            true,
            false,
            exec::ApprovalPolicy::ReadOnly,
        );

        assert_eq!(body.get("input"), Some(&Value::Array(input)));
        assert_eq!(
            body.get("input")
                .and_then(Value::as_array)
                .and_then(|items| items.get(1)),
            Some(&json!({
                "type": "reasoning",
                "summary": [],
                "encrypted_content": "encrypted-reasoning"
            }))
        );
        assert_eq!(
            body.get("input")
                .and_then(Value::as_array)
                .and_then(|items| items.get(2))
                .and_then(|item| item.get("type"))
                .and_then(Value::as_str),
            Some("function_call")
        );
    }

    #[test]
    fn first_turn_project_context_is_injected_with_exact_environment_shape() {
        let context = ProjectPromptContext {
            environment_context: render_environment_context(
                "/Users/demo/programming/halv",
                "zsh",
                "2026-07-02",
                "America/Sao_Paulo",
                false,
                exec::ApprovalPolicy::ReadOnly,
            ),
            agents_instructions: Some(
                "# AGENTS.md instructions for /Users/demo/programming/halv\n\n<INSTRUCTIONS>\nKeep changes narrow.\n</INSTRUCTIONS>"
                    .to_string(),
            ),
        };

        let body = build_responses_request_body(
            "11111111-1111-4111-8111-111111111111",
            &[stored_message("user", "Hello project")],
            "gpt-5.5",
            "medium",
            None,
            Some(&context),
            None,
            false,
            false,
            exec::ApprovalPolicy::ReadOnly,
        );

        let input = body
            .get("input")
            .and_then(Value::as_array)
            .expect("input should be an array");
        assert_eq!(input.len(), 3);
        assert_eq!(
            input[0],
            json!({
                "type": "message",
                "role": "user",
                "content": [{
                    "type": "input_text",
                    "text": "<environment_context>\n  <cwd>/Users/demo/programming/halv</cwd>\n  <shell>zsh</shell>\n  <current_date>2026-07-02</current_date>\n  <timezone>America/Sao_Paulo</timezone>\n  <!-- Project context only; shell tools are disabled for this conversation. -->\n</environment_context>"
                }]
            })
        );
        assert_eq!(
            input[1],
            json!({
                "type": "message",
                "role": "user",
                "content": [{
                    "type": "input_text",
                    "text": "# AGENTS.md instructions for /Users/demo/programming/halv\n\n<INSTRUCTIONS>\nKeep changes narrow.\n</INSTRUCTIONS>"
                }]
            })
        );
        assert_eq!(
            input[2]
                .get("content")
                .and_then(Value::as_array)
                .and_then(|content| content.first())
                .and_then(|part| part.get("text"))
                .and_then(Value::as_str),
            Some("Hello project")
        );
    }

    #[test]
    fn agents_instructions_use_override_precedence_and_budget_truncation() {
        let root = std::env::temp_dir().join(format!("halv-agents-{}", storage::new_uuid_v4()));
        std::fs::create_dir_all(&root).expect("project root should create");
        let global_path = root.join("global-AGENTS.md");
        std::fs::write(&global_path, "global instructions").expect("global agents should write");
        std::fs::write(root.join("AGENTS.md"), "base instructions")
            .expect("base agents should write");
        std::fs::write(root.join("AGENTS.override.md"), "override")
            .expect("override agents should write");

        let rendered = render_agents_instructions_with_global(&root, Some(&global_path))
            .expect("agents should render");
        assert!(rendered.contains(&format!(
            "<INSTRUCTIONS>\nglobal instructions{PROJECT_DOC_SEPARATOR}override\n</INSTRUCTIONS>"
        )));
        assert!(!rendered.contains("base instructions"));

        std::fs::remove_file(root.join("AGENTS.override.md"))
            .expect("override agents should remove");
        std::fs::write(
            root.join("AGENTS.md"),
            "x".repeat(AGENTS_DOC_MAX_BYTES + 128),
        )
        .expect("large agents should write");

        let body = collect_agents_doc_body_with_global(&root, None).expect("body should collect");
        assert_eq!(body.len(), AGENTS_DOC_MAX_BYTES);
        assert!(body.chars().all(|value| value == 'x'));

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn confined_project_file_rejects_parent_paths() {
        let base = std::env::temp_dir().join(format!("halv-confine-{}", storage::new_uuid_v4()));
        let root = base.join("repo");
        std::fs::create_dir_all(root.join("src")).expect("project root should create");
        std::fs::write(root.join("src/main.rs"), "fn main() {}").expect("file should write");
        std::fs::write(base.join("outside.txt"), "secret").expect("outside should write");

        let confined =
            confined_project_file(&root, "src/main.rs").expect("inside file should resolve");
        assert!(confined.ends_with("src/main.rs"));
        assert!(confined_project_file(&root, "../outside.txt").is_none());
        assert!(
            confined_project_file(&root, &base.join("outside.txt").to_string_lossy()).is_none()
        );

        let _ = std::fs::remove_dir_all(base);
    }

    #[cfg(unix)]
    #[test]
    fn confined_project_file_rejects_symlink_escapes() {
        let base = std::env::temp_dir().join(format!("halv-symlink-{}", storage::new_uuid_v4()));
        let root = base.join("repo");
        std::fs::create_dir_all(&root).expect("project root should create");
        std::fs::write(base.join("outside.txt"), "secret").expect("outside should write");
        std::os::unix::fs::symlink(base.join("outside.txt"), root.join("leak.txt"))
            .expect("symlink should create");

        assert!(confined_project_file(&root, "leak.txt").is_none());

        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn referenced_file_context_dedupes_skips_binary_and_truncates() {
        let base = std::env::temp_dir().join(format!("halv-mentions-{}", storage::new_uuid_v4()));
        let root = base.join("repo");
        std::fs::create_dir_all(root.join("src")).expect("project root should create");
        let long_text = "a".repeat(FILE_MENTION_MAX_BYTES + 1024);
        std::fs::write(root.join("src/large.txt"), long_text).expect("large file should write");
        let sparse_path = root.join("src/sparse.txt");
        std::fs::write(&sparse_path, "s".repeat(FILE_MENTION_MAX_BYTES))
            .expect("sparse prefix should write");
        std::fs::OpenOptions::new()
            .write(true)
            .open(&sparse_path)
            .expect("sparse file should open")
            .set_len(1024 * 1024 * 1024)
            .expect("sparse file should extend");
        std::fs::write(root.join("src/binary.bin"), [0xff, 0xfe, 0xfd])
            .expect("binary file should write");
        std::fs::write(base.join("outside.txt"), "secret").expect("outside should write");

        let mentions = vec![
            "src/large.txt".to_string(),
            "src/large.txt".to_string(),
            "src/sparse.txt".to_string(),
            "src/binary.bin".to_string(),
            "../outside.txt".to_string(),
        ];
        let referenced = build_referenced_files_context(Some(&root), Some(&mentions));
        let context = referenced
            .context
            .expect("context should include the text mention");

        assert_eq!(context.matches("Referenced file: src/large.txt").count(), 1);
        assert_eq!(
            context.matches("Referenced file: src/sparse.txt").count(),
            1
        );
        assert!(context.contains(FILE_MENTION_TRUNCATED_SUFFIX.trim()));
        assert!(!context.contains("src/binary.bin"));
        assert!(!context.contains("outside.txt"));
        assert_eq!(
            referenced.skipped_mentions,
            vec!["src/binary.bin".to_string(), "../outside.txt".to_string()]
        );

        let truncated =
            truncate_referenced_file_text(&"é".repeat(FILE_MENTION_MAX_BYTES), 128, false);
        assert!(truncated.len() <= 128);
        assert!(truncated.ends_with(FILE_MENTION_TRUNCATED_SUFFIX));

        let _ = std::fs::remove_dir_all(base);
    }

    fn test_savings_point(
        tokens_saved: u64,
        savings_usd: Option<f64>,
        rtk_tokens_saved: u64,
    ) -> SavingsPoint {
        SavingsPoint {
            project: ProjectSavingsTotals {
                tokens_saved,
                savings_usd,
            },
            rtk_tokens_saved,
        }
    }

    fn turn_project() -> GuiHeadroomProject {
        gui_turn_headroom_project("conversation", "turn")
    }

    fn settled_test_sample(
        before: Option<SavingsPoint>,
        samples: &[Option<SavingsPoint>],
    ) -> (Option<SavingsPoint>, usize, bool) {
        settled_test_sample_with_grace(before, samples, SAVINGS_SETTLE_ZERO_GRACE)
    }

    fn settled_test_sample_with_grace(
        before: Option<SavingsPoint>,
        samples: &[Option<SavingsPoint>],
        zero_grace: Duration,
    ) -> (Option<SavingsPoint>, usize, bool) {
        let mut tracker = SavingsSettleTracker::default();
        let mut after = None;
        for (index, sample) in samples.iter().copied().enumerate() {
            after = sample;
            let elapsed = SAVINGS_SETTLE_DELAY * u32::try_from(index).expect("sample index");
            if tracker.observe(&before, &after, elapsed, zero_grace) {
                return (after, index + 1, true);
            }
        }
        (after, samples.len(), false)
    }

    #[test]
    fn savings_delta_is_nullable_without_both_snapshots() {
        let point = Some(test_savings_point(10, Some(0.5), 0));

        assert_eq!(savings_delta(&turn_project(), None, None), (None, None));
        assert_eq!(savings_delta(&turn_project(), point, None), (None, None));
        assert_eq!(savings_delta(&turn_project(), None, point), (None, None));
    }

    #[test]
    fn overlapping_turn_projects_record_only_their_own_deltas() {
        let before_a = Some(test_savings_point(0, None, 0));
        let before_b = Some(test_savings_point(0, None, 0));
        let after_a = Some(test_savings_point(20, Some(0.2), 0));
        let after_b = Some(test_savings_point(20, Some(0.2), 0));
        let project_a = gui_turn_headroom_project("conversation", "turn-a");
        let project_b = gui_turn_headroom_project("conversation", "turn-b");

        assert_eq!(
            savings_delta_for_model(&project_a, before_a, after_a, DEFAULT_MODEL),
            (Some(20), Some(0.2), Some(PricingSource::Headroom))
        );
        assert_eq!(
            savings_delta_for_model(&project_b, before_b, after_b, DEFAULT_MODEL),
            (Some(20), Some(0.2), Some(PricingSource::Headroom))
        );
    }

    #[test]
    fn savings_delta_prices_rtk_filtering() {
        let before = Some(test_savings_point(100, Some(0.5), 40));
        let after = Some(test_savings_point(125, Some(0.75), 70));

        let (tokens_saved, savings_usd, source) =
            savings_delta_for_model(&turn_project(), before, after, DEFAULT_MODEL);
        assert_eq!(tokens_saved, Some(55));
        assert!((savings_usd.expect("savings should be priced") - 0.250_15).abs() < 1e-12);
        assert_eq!(source, Some(PricingSource::ModelTable));
    }

    #[test]
    fn missing_headroom_usd_defers_pricing_to_storage() {
        let before = Some(test_savings_point(0, None, 0));
        let after = Some(test_savings_point(25, None, 0));

        assert_eq!(
            savings_delta_for_model(&turn_project(), before, after, DEFAULT_MODEL),
            (Some(25), None, None)
        );
    }

    #[test]
    fn background_project_never_produces_message_savings() {
        let project = gui_background_headroom_project("conversation", "title");
        let before = Some(test_savings_point(0, None, 0));
        let after = Some(test_savings_point(25, Some(0.25), 0));

        assert_eq!(
            savings_delta_for_model(&project, before, after, DEFAULT_MODEL),
            (None, None, None)
        );
    }

    #[test]
    fn savings_delta_clamps_decreasing_project_counters() {
        let before = Some(test_savings_point(125, Some(0.75), 0));
        let after = Some(test_savings_point(100, Some(0.5), 0));

        assert_eq!(
            savings_delta(&turn_project(), before, after),
            (Some(0), Some(0.0))
        );
    }

    #[test]
    fn savings_settle_waits_for_stable_zero() {
        let before = Some(test_savings_point(100, Some(0.5), 0));
        let zero = before;

        let (early_after, early_samples, early_settled) =
            settled_test_sample(before, &[zero, zero, zero, zero, zero]);
        assert!(!early_settled);
        assert_eq!(early_samples, 5);
        assert_eq!(
            savings_delta(&turn_project(), before, early_after),
            (Some(0), Some(0.0))
        );

        let (after, samples_used, settled) =
            settled_test_sample(before, &[zero, zero, zero, zero, zero, zero]);

        assert!(settled);
        assert_eq!(samples_used, 6);
        assert_eq!(
            savings_delta(&turn_project(), before, after),
            (Some(0), Some(0.0))
        );
    }

    #[test]
    fn cancelled_savings_settle_uses_short_zero_grace() {
        let before = Some(test_savings_point(100, Some(0.5), 0));
        let zero = before;

        let (_, early_samples, early_settled) =
            settled_test_sample_with_grace(before, &[zero, zero], SAVINGS_CANCEL_SETTLE_GRACE);
        assert!(!early_settled);
        assert_eq!(early_samples, 2);

        let (_, samples_used, settled) = settled_test_sample_with_grace(
            before,
            &[zero, zero, zero],
            SAVINGS_CANCEL_SETTLE_GRACE,
        );
        assert!(settled);
        assert_eq!(samples_used, SAVINGS_SETTLE_STABLE_SAMPLES);
    }

    #[test]
    fn savings_settle_waits_through_delayed_partial_increments() {
        let before = Some(test_savings_point(100, Some(0.5), 0));
        let zero = before;
        let partial = Some(test_savings_point(110, Some(0.6), 0));
        let final_point = Some(test_savings_point(125, Some(0.75), 0));

        let (after, samples_used, settled) = settled_test_sample(
            before,
            &[
                zero,
                partial,
                partial,
                final_point,
                final_point,
                final_point,
            ],
        );

        assert!(settled);
        assert_eq!(samples_used, 6);
        assert_eq!(
            savings_delta(&turn_project(), before, after),
            (Some(25), Some(0.25))
        );
    }

    #[test]
    fn savings_settle_stops_after_a_positive_value_is_stable() {
        let before = Some(test_savings_point(100, Some(0.5), 0));
        let stable = Some(test_savings_point(125, Some(0.75), 0));
        let later = Some(test_savings_point(150, Some(1.0), 0));

        let (after, samples_used, settled) =
            settled_test_sample(before, &[stable, stable, stable, later]);

        assert!(settled);
        assert_eq!(samples_used, SAVINGS_SETTLE_STABLE_SAMPLES);
        assert_eq!(
            savings_delta(&turn_project(), before, after),
            (Some(25), Some(0.25))
        );
    }

    #[test]
    fn savings_settle_does_not_wait_without_a_baseline() {
        let after = Some(test_savings_point(25, Some(0.25), 0));

        let (_, samples_used, settled) = settled_test_sample(None, &[after, after]);

        assert!(settled);
        assert_eq!(samples_used, 1);
    }

    #[test]
    fn project_header_and_keys_use_safe_bounded_values() {
        assert_eq!(
            gui_turn_headroom_project("conversation-id", "message-id").key,
            "gui:conversation-id:message-id"
        );
        let project = gui_turn_headroom_project("conversation\nunsafe", &"t".repeat(200));
        let background = gui_background_headroom_project("conversation", "memory extraction");
        let mut headers = HeaderMap::new();
        insert_headroom_project_header(&mut headers, &project.key)
            .expect("project header should build");

        let value = headers
            .get(HEADROOM_PROJECT_HEADER)
            .and_then(|value| value.to_str().ok())
            .expect("project header should exist");
        assert_eq!(value, project.key);
        assert!(value.starts_with("gui:conversation_unsafe:"));
        assert!(value.len() <= HEADROOM_PROJECT_MAX_CHARS);
        assert!(value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b':' | b'-' | b'_')));
        assert_eq!(background.key, "gui-bg:conversation:memory_extraction");
        assert_eq!(background.scope, GuiHeadroomProjectScope::Background);
    }

    fn persist_success_fixture(
        assistant_text: &str,
        active_goal: Option<&str>,
    ) -> (ChatDeltaPayload, String) {
        let emitter = RecordingEmitter::default();
        let db_path = std::env::temp_dir().join(format!(
            "halv-goal-marker-{}.sqlite",
            storage::new_uuid_v4()
        ));
        let storage = Storage::open(&db_path).expect("storage should open");
        let conversation_id = storage
            .ensure_conversation(None, "Hello", DEFAULT_MODEL, None)
            .expect("conversation should insert");

        persist_success_before_terminal_delta(
            &emitter,
            &storage,
            &conversation_id,
            DEFAULT_MODEL,
            &StreamSuccess {
                assistant_text: assistant_text.to_string(),
                usage: None,
                service_tier: None,
                fast_mode_fallback: false,
            },
            active_goal,
            None,
            TurnSavings::default(),
        )
        .expect("success should persist");

        let payload = emitter
            .payloads
            .lock()
            .expect("recording emitter lock should not be poisoned")
            .first()
            .expect("done payload should be emitted")
            .clone();
        let assistant_content = storage
            .get_messages(&conversation_id)
            .expect("messages should load")
            .into_iter()
            .find(|message| message.role == "assistant")
            .expect("assistant message should be persisted")
            .content;

        drop(storage);
        let _ = std::fs::remove_file(db_path);

        (payload, assistant_content)
    }

    #[test]
    fn goal_done_marker_is_stripped_and_signaled() {
        let (payload, assistant_content) =
            persist_success_fixture("All set. [HALV_GOAL_DONE]\n\n", Some("Ship it"));

        assert_eq!(payload.kind, ChatDeltaKind::Done);
        assert_eq!(payload.goal_signal.as_deref(), Some("done"));
        assert_eq!(assistant_content, "All set.");
    }

    #[test]
    fn goal_continue_marker_is_stripped_and_signaled() {
        let (payload, assistant_content) =
            persist_success_fixture("Made progress.\n[HALV_GOAL_CONTINUE]  ", Some("Ship it"));

        assert_eq!(payload.kind, ChatDeltaKind::Done);
        assert_eq!(payload.goal_signal.as_deref(), Some("continue"));
        assert_eq!(assistant_content, "Made progress.");
    }

    #[test]
    fn goal_blocked_marker_is_stripped_without_continue_signal() {
        let (payload, assistant_content) = persist_success_fixture(
            "Enable Agent mode to continue. [HALV_GOAL_BLOCKED]\n",
            Some("Ship it"),
        );

        assert_eq!(payload.kind, ChatDeltaKind::Done);
        assert_eq!(payload.goal_signal, None);
        assert_eq!(assistant_content, "Enable Agent mode to continue.");
    }

    #[test]
    fn active_goal_without_marker_defaults_to_continue_signal() {
        let (payload, assistant_content) =
            persist_success_fixture("No marker here.", Some("Ship it"));

        assert_eq!(payload.goal_signal.as_deref(), Some("continue"));
        assert_eq!(assistant_content, "No marker here.");
    }

    #[test]
    fn goal_marker_is_only_stripped_at_the_end() {
        let text = "This mentions [HALV_GOAL_DONE] before the end.";
        let (payload, assistant_content) = persist_success_fixture(text, Some("Ship it"));

        assert_eq!(payload.goal_signal.as_deref(), Some("continue"));
        assert_eq!(assistant_content, text);
    }

    #[test]
    fn no_active_goal_without_marker_has_no_goal_signal() {
        let text = "No marker here.";
        let (payload, assistant_content) = persist_success_fixture(text, None);

        assert_eq!(payload.goal_signal, None);
        assert_eq!(assistant_content, text);
    }

    #[test]
    fn goal_marker_is_ignored_without_an_active_goal() {
        let text = "All set. [HALV_GOAL_DONE]";
        let (payload, assistant_content) = persist_success_fixture(text, None);

        assert_eq!(payload.goal_signal, None);
        assert_eq!(assistant_content, text);
    }

    #[cfg(target_os = "macos")]
    fn direct_shell_rtk_command_count() -> i64 {
        let path = exec::rtk_database_path();
        if !path.is_file() {
            return 0;
        }
        rusqlite::Connection::open(path)
            .and_then(|connection| {
                connection.query_row("SELECT COUNT(*) FROM commands", [], |row| row.get(0))
            })
            .unwrap_or(0)
    }

    #[test]
    fn revert_registration_refuses_live_turn_and_blocks_new_turn() {
        let chat = ChatManager::new();
        let chat_cancel = chat
            .try_register_request("conversation-1", Some("turn-1"))
            .expect("chat request should register");
        let error = chat
            .try_register_revert("conversation-1")
            .expect_err("revert should refuse a live chat turn");
        assert_eq!(error, "A turn is still running — stop it before reverting");
        chat.clear_request("conversation-1", &chat_cancel);

        let revert_cancel = chat
            .try_register_revert("conversation-1")
            .expect("revert should register");
        assert!(chat
            .try_register_request("conversation-1", Some("turn-2"))
            .is_err());
        chat.clear_request("conversation-1", &revert_cancel);
        assert!(chat
            .try_register_request("conversation-1", Some("turn-2"))
            .is_ok());
    }

    #[test]
    fn cancel_and_revert_reservation_is_exclusive_until_release() {
        let chat = ChatManager::new();
        let request = chat
            .try_register_request("conversation-1", Some("turn-1"))
            .expect("chat request should register");
        let reserved = chat
            .reserve_cancel_and_revert("conversation-1")
            .expect("reservation should not fail")
            .expect("live request should reserve");
        assert!(chat.reserve_cancel_and_revert("conversation-1").is_err());

        chat.clear_request("conversation-1", &request);
        assert!(chat
            .try_register_request("conversation-1", Some("racing-turn"))
            .is_err());
        chat.release_request("conversation-1", &reserved);
        assert!(chat
            .try_register_request("conversation-1", Some("next-turn"))
            .is_ok());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn truncate_waits_for_turn_persistence() {
        let db_path = std::env::temp_dir().join(format!(
            "halv-truncate-wait-{}.sqlite",
            storage::new_uuid_v4()
        ));
        let storage = Storage::open(&db_path).expect("storage should open");
        let conversation_id = storage
            .ensure_conversation(None, "First", DEFAULT_MODEL, None)
            .expect("conversation should insert");
        let anchor = storage
            .insert_message(NewMessage {
                conversation_id: &conversation_id,
                role: "user",
                content: "Edit me",
                model: Some(DEFAULT_MODEL),
                input_tokens: None,
                output_tokens: None,
                error: false,
                mentions: None,
            })
            .expect("anchor should insert");
        let chat = ChatManager::new();
        let request = chat
            .try_register_request(&conversation_id, Some("turn-1"))
            .expect("request should register");

        let truncate = truncate_conversation_from(&chat, &storage, &conversation_id, &anchor.id);
        let finish_turn = async {
            assert_eq!(
                storage
                    .get_messages(&conversation_id)
                    .expect("messages should list")
                    .len(),
                1,
                "truncate must wait while the request is active"
            );
            persist_error_before_terminal_delta(
                &RecordingEmitter::default(),
                &storage,
                &conversation_id,
                DEFAULT_MODEL,
                &ChatError::with_partial("Cancelled", "Partial answer".to_string()),
                None,
                TurnSavings::default(),
            )
            .expect("partial response should persist");
            chat.clear_request(&conversation_id, &request);
            assert_eq!(
                chat.try_register_request(&conversation_id, Some("racing-turn"))
                    .expect_err("truncation should block a racing turn"),
                "Conversation messages are being changed; try again"
            );
        };

        let (result, ()) = tokio::join!(truncate, finish_turn);
        result.expect("truncate should finish after persistence");
        assert!(storage
            .get_messages(&conversation_id)
            .expect("messages should list")
            .is_empty());
        let next_request = chat
            .try_register_request(&conversation_id, Some("next-turn"))
            .expect("truncation should release the conversation");
        chat.clear_request(&conversation_id, &next_request);
        drop(storage);
        let _ = std::fs::remove_file(db_path);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn cancelled_turn_with_writes_becomes_revertible_and_revert_restores_content() {
        let root = std::env::temp_dir().join(format!(
            "halv-cancel-revert-write-{}",
            storage::new_uuid_v4()
        ));
        init_git_repo(&root);
        let root = root
            .canonicalize()
            .expect("project root should canonicalize");
        let tracked_path = root.join("tracked.txt");
        std::fs::write(&tracked_path, "before\n").expect("tracked file should write");
        run_git(&root, &["add", "tracked.txt"]);
        let root_string = root.to_string_lossy().to_string();
        let preimages = snapshot_git_shell_preimages(&root_string);
        std::fs::write(&tracked_path, "partial\n").expect("partial write should land");

        let db_path = std::env::temp_dir().join(format!(
            "halv-cancel-revert-write-{}.sqlite",
            storage::new_uuid_v4()
        ));
        let storage = Storage::open(&db_path).expect("storage should open");
        let project = storage
            .upsert_project("Cancel revert", &root_string)
            .expect("project should insert");
        let conversation_id = storage
            .ensure_conversation(
                None,
                "Cancel partial write",
                DEFAULT_MODEL,
                Some(project.id),
            )
            .expect("conversation should insert");
        let turn_id = "turn-with-write";
        let chat = ChatManager::new();
        let request = chat
            .try_register_request(&conversation_id, Some(turn_id))
            .expect("turn should register");
        let emitter = RecordingEmitter::default();

        let cancel_and_revert =
            chat_cancel_and_revert_inner(&emitter, &chat, &storage, &conversation_id);
        let finish_cancelled_turn = async {
            wait_for_cancel(request.cancel.clone()).await;
            finalize_shell_turn_diff(
                &emitter,
                &storage,
                &conversation_id,
                turn_id,
                &root_string,
                preimages,
            )
            .await
            .expect("cancelled shell diff should finalize");
            let diffs = storage
                .list_turn_diffs(&conversation_id)
                .expect("turn diffs should list");
            assert_eq!(diffs.len(), 1);
            assert!(diffs[0].revertible);
            assert!(diffs[0].diff.contains("tracked.txt"));
            assert!(diffs[0].diff.contains("partial"));
            assert_eq!(
                storage
                    .get_turn_snapshots(&conversation_id, turn_id)
                    .expect("turn snapshots should list")
                    .len(),
                1
            );
            chat.clear_request(&conversation_id, &request);
            assert!(chat
                .try_register_request(&conversation_id, Some("racing-turn"))
                .is_err());
        };
        let (result, ()) = tokio::join!(cancel_and_revert, finish_cancelled_turn);
        let result = result.expect("cancel and revert should succeed");

        assert_eq!(result.reverted_files, 1);
        assert_eq!(
            std::fs::read_to_string(&tracked_path).expect("restored file should read"),
            "before\n"
        );
        assert!(storage
            .list_turn_diffs(&conversation_id)
            .expect("turn diffs should list")
            .is_empty());
        let next_request = chat
            .try_register_request(&conversation_id, Some("next-turn"))
            .expect("cancel-and-revert should release the conversation");
        chat.clear_request(&conversation_id, &next_request);

        drop(storage);
        let _ = std::fs::remove_dir_all(root);
        let _ = std::fs::remove_file(db_path);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn cancel_and_revert_without_writes_returns_plain_cancel() {
        let db_path = std::env::temp_dir().join(format!(
            "halv-cancel-revert-empty-{}.sqlite",
            storage::new_uuid_v4()
        ));
        let storage = Storage::open(&db_path).expect("storage should open");
        let conversation_id = storage
            .ensure_conversation(None, "Cancel no writes", DEFAULT_MODEL, None)
            .expect("conversation should insert");
        let chat = ChatManager::new();
        let request = chat
            .try_register_request(&conversation_id, Some("turn-without-writes"))
            .expect("turn should register");
        let emitter = RecordingEmitter::default();

        let cancel_and_revert =
            chat_cancel_and_revert_inner(&emitter, &chat, &storage, &conversation_id);
        let finish_cancelled_turn = async {
            wait_for_cancel(request.cancel.clone()).await;
            chat.clear_request(&conversation_id, &request);
        };
        let (result, ()) = tokio::join!(cancel_and_revert, finish_cancelled_turn);
        let result = result.expect("plain cancellation should succeed");

        assert_eq!(result.reverted_files, 0);
        assert!(storage
            .list_turn_diffs(&conversation_id)
            .expect("turn diffs should list")
            .is_empty());
        let next_request = chat
            .try_register_request(&conversation_id, Some("next-turn"))
            .expect("plain cancel should release the conversation");
        chat.clear_request(&conversation_id, &next_request);

        drop(storage);
        let _ = std::fs::remove_file(db_path);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn cancel_and_revert_without_new_writes_preserves_unrelated_turn_diff() {
        let db_path = std::env::temp_dir().join(format!(
            "halv-cancel-revert-existing-{}.sqlite",
            storage::new_uuid_v4()
        ));
        let storage = Storage::open(&db_path).expect("storage should open");
        let conversation_id = storage
            .ensure_conversation(None, "Regenerate no writes", DEFAULT_MODEL, None)
            .expect("conversation should insert");
        let historical_turn_id = "historical-turn";
        let current_turn_id = "regenerated-turn";
        storage
            .upsert_turn_diff(
                &conversation_id,
                historical_turn_id,
                Path::new("/historical/project"),
                "historical diff\n",
                true,
            )
            .expect("historical diff should insert");
        let chat = ChatManager::new();
        let request = chat
            .try_register_request(&conversation_id, Some(current_turn_id))
            .expect("regeneration should register");
        let emitter = RecordingEmitter::default();

        let cancel_and_revert =
            chat_cancel_and_revert_inner(&emitter, &chat, &storage, &conversation_id);
        let finish_cancelled_turn = async {
            wait_for_cancel(request.cancel.clone()).await;
            chat.clear_request(&conversation_id, &request);
        };
        let (result, ()) = tokio::join!(cancel_and_revert, finish_cancelled_turn);
        let result = result.expect("no-new-write regeneration should only cancel");

        assert_eq!(result.reverted_files, 0);
        let diffs = storage
            .list_turn_diffs(&conversation_id)
            .expect("turn diffs should list");
        assert_eq!(diffs.len(), 1);
        assert_eq!(diffs[0].diff, "historical diff\n");

        drop(storage);
        let _ = std::fs::remove_file(db_path);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn cancel_and_revert_surfaces_finalization_error_without_deleting_diff() {
        let db_path = std::env::temp_dir().join(format!(
            "halv-cancel-revert-finalization-{}.sqlite",
            storage::new_uuid_v4()
        ));
        let storage = Storage::open(&db_path).expect("storage should open");
        let conversation_id = storage
            .ensure_conversation(None, "Finalization error", DEFAULT_MODEL, None)
            .expect("conversation should insert");
        let turn_id = "turn-finalization-error";
        storage
            .upsert_turn_diff(
                &conversation_id,
                turn_id,
                Path::new("/historical/project"),
                "recorded diff\n",
                true,
            )
            .expect("recorded diff should insert");
        let chat = ChatManager::new();
        let request = chat
            .try_register_request(&conversation_id, Some(turn_id))
            .expect("turn should register");
        request.set_finalization_error("postimage capture failed");
        let emitter = RecordingEmitter::default();

        let cancel_and_revert =
            chat_cancel_and_revert_inner(&emitter, &chat, &storage, &conversation_id);
        let finish_cancelled_turn = async {
            wait_for_cancel(request.cancel.clone()).await;
            chat.clear_request(&conversation_id, &request);
        };
        let (result, ()) = tokio::join!(cancel_and_revert, finish_cancelled_turn);
        let error = result.expect_err("incomplete finalization must block revert");

        assert!(error.contains("could not finalize its file snapshots"));
        assert_eq!(
            storage
                .list_turn_diffs(&conversation_id)
                .expect("turn diff should remain")
                .len(),
            1
        );
        let next_request = chat
            .try_register_request(&conversation_id, Some("next-turn"))
            .expect("failed finalization should release the conversation");
        chat.clear_request(&conversation_id, &next_request);

        drop(storage);
        let _ = std::fs::remove_file(db_path);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn cancel_and_revert_deletes_oversized_file_created_by_turn() {
        let root = std::env::temp_dir().join(format!(
            "halv-cancel-revert-budget-{}",
            storage::new_uuid_v4()
        ));
        init_git_repo(&root);
        let root = root
            .canonicalize()
            .expect("project root should canonicalize");
        let root_string = root.to_string_lossy().to_string();
        let preimages = snapshot_git_shell_preimages(&root_string);
        let oversized_path = root.join("oversized.bin");
        std::fs::File::create(&oversized_path)
            .and_then(|file| file.set_len(9 * 1024 * 1024))
            .expect("oversized partial write should land");
        let db_path = std::env::temp_dir().join(format!(
            "halv-cancel-revert-budget-{}.sqlite",
            storage::new_uuid_v4()
        ));
        let storage = Storage::open(&db_path).expect("storage should open");
        let project = storage
            .upsert_project("Cancel budget", &root_string)
            .expect("project should insert");
        let conversation_id = storage
            .ensure_conversation(
                None,
                "Cancel oversized write",
                DEFAULT_MODEL,
                Some(project.id),
            )
            .expect("conversation should insert");
        let turn_id = "turn-over-budget";
        let chat = ChatManager::new();
        let request = chat
            .try_register_request(&conversation_id, Some(turn_id))
            .expect("turn should register");
        let emitter = RecordingEmitter::default();

        let cancel_and_revert =
            chat_cancel_and_revert_inner(&emitter, &chat, &storage, &conversation_id);
        let finish_cancelled_turn = async {
            wait_for_cancel(request.cancel.clone()).await;
            finalize_shell_turn_diff(
                &emitter,
                &storage,
                &conversation_id,
                turn_id,
                &root_string,
                preimages,
            )
            .await
            .expect("oversized created file should finalize");
            let diffs = storage
                .list_turn_diffs(&conversation_id)
                .expect("turn diffs should list");
            assert_eq!(diffs.len(), 1);
            assert!(diffs[0].revertible);
            assert!(diffs[0].diff.contains("oversized.bin"));
            assert!(diffs[0]
                .diff
                .contains("Halv omitted a binary or oversized file diff."));
            chat.clear_request(&conversation_id, &request);
        };
        let (result, ()) = tokio::join!(cancel_and_revert, finish_cancelled_turn);
        let result = result.expect("oversized created file should revert");

        assert_eq!(result.reverted_files, 1);
        assert!(!oversized_path.exists());
        assert!(storage
            .list_turn_diffs(&conversation_id)
            .expect("turn diffs should list")
            .is_empty());
        let next_request = chat
            .try_register_request(&conversation_id, Some("next-turn"))
            .expect("successful revert should release conversation");
        chat.clear_request(&conversation_id, &next_request);

        drop(storage);
        let _ = std::fs::remove_dir_all(root);
        let _ = std::fs::remove_file(db_path);
    }

    #[test]
    fn turn_revert_result_serializes_file_count_as_camel_case() {
        assert_eq!(
            serde_json::to_value(TurnRevertResult { reverted_files: 2 })
                .expect("revert result should serialize"),
            json!({ "revertedFiles": 2 })
        );
    }

    #[test]
    fn terminal_deltas_require_assistant_persistence() {
        let emitter = RecordingEmitter::default();
        let db_path = std::env::temp_dir().join(format!(
            "halv-terminal-delta-{}.sqlite",
            storage::new_uuid_v4()
        ));
        let storage = Storage::open(&db_path).expect("storage should open");
        let conversation_id = storage
            .ensure_conversation(None, "Hello", DEFAULT_MODEL, None)
            .expect("conversation should insert");
        storage
            .delete_conversation(&conversation_id)
            .expect("conversation should delete");

        let success_result = persist_success_before_terminal_delta(
            &emitter,
            &storage,
            &conversation_id,
            DEFAULT_MODEL,
            &StreamSuccess {
                assistant_text: "Hello back".to_string(),
                usage: Some(TokenUsage {
                    input_tokens: 1,
                    output_tokens: 2,
                    total_tokens: 3,
                    cached_tokens: 0,
                    reasoning_tokens: 0,
                }),
                service_tier: None,
                fast_mode_fallback: false,
            },
            None,
            None,
            TurnSavings::default(),
        );
        assert!(success_result.is_err());

        let error_result = persist_error_before_terminal_delta(
            &emitter,
            &storage,
            &conversation_id,
            DEFAULT_MODEL,
            &ChatError::with_partial("stream failed", "partial text".to_string()),
            None,
            TurnSavings::default(),
        );
        assert!(error_result.is_err());

        let payloads = emitter
            .payloads
            .lock()
            .expect("recording emitter lock should not be poisoned");
        assert!(payloads.is_empty());
        drop(payloads);

        drop(storage);
        let _ = std::fs::remove_file(db_path);
    }

    #[test]
    fn turn_revert_refuses_when_later_turn_touched_same_path() {
        let db_path = std::env::temp_dir().join(format!(
            "halv-overlap-revert-{}.sqlite",
            storage::new_uuid_v4()
        ));
        let storage = Storage::open(&db_path).expect("storage should open");
        let conversation_id = storage
            .ensure_conversation(None, "Track overlapping turns", DEFAULT_MODEL, None)
            .expect("conversation should insert");
        let cwd =
            std::env::temp_dir().join(format!("halv-overlap-revert-{}", storage::new_uuid_v4()));
        std::fs::create_dir_all(cwd.join("src")).expect("cwd should create");
        let absolute_path = cwd.join("src/lib.rs");

        storage
            .insert_turn_snapshot(
                &conversation_id,
                "turn-1",
                "src/lib.rs",
                &absolute_path,
                Some(b"old-before"),
            )
            .expect("old turn snapshot should insert");
        storage
            .upsert_turn_diff(&conversation_id, "turn-1", &cwd, "old diff\n", true)
            .expect("old turn diff should insert");
        storage
            .insert_turn_snapshot(
                &conversation_id,
                "turn-2",
                "./src/lib.rs",
                &absolute_path,
                Some(b"new-before"),
            )
            .expect("new turn snapshot should insert");
        storage
            .upsert_turn_diff(&conversation_id, "turn-2", &cwd, "new diff\n", true)
            .expect("new turn diff should insert");

        let old_snapshots = storage
            .get_turn_snapshots(&conversation_id, "turn-1")
            .expect("old snapshots should load");
        let error = ensure_turn_revert_has_no_later_overlap(
            &storage,
            &conversation_id,
            "turn-1",
            &old_snapshots,
        )
        .expect_err("old turn should be blocked by later overlap");
        assert!(error.contains("later turn"));
        assert!(error.contains("src/lib.rs"));

        let new_snapshots = storage
            .get_turn_snapshots(&conversation_id, "turn-2")
            .expect("new snapshots should load");
        ensure_turn_revert_has_no_later_overlap(
            &storage,
            &conversation_id,
            "turn-2",
            &new_snapshots,
        )
        .expect("latest overlapping turn should be revertable");

        drop(storage);
        let _ = std::fs::remove_dir_all(cwd);
        let _ = std::fs::remove_file(db_path);
    }

    #[test]
    fn claude_sse_parser_handles_text_thinking_and_tool_input_events() {
        let fixture = concat!(
            "event: content_block_delta\n",
            "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"Hello\"}}\n\n",
            "event: content_block_delta\r\n",
            "data: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"thinking_delta\",\"thinking\":\"Check\"}}\r\n\r\n",
            "event: content_block_delta\n",
            "data: {\"type\":\"content_block_delta\",\"index\":2,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"command\\\":\\\"pwd\\\"}\"}}\n\n"
        );
        let mut parser = ClaudeSseParser::default();
        let split = fixture.len() / 2;
        let mut events = parser
            .push(&fixture.as_bytes()[..split], 10)
            .expect("first chunk should parse");
        events.extend(
            parser
                .push(&fixture.as_bytes()[split..], 10)
                .expect("second chunk should parse"),
        );
        assert_eq!(events.len(), 3);
        assert_eq!(events[0]["delta"]["text"], "Hello");
        assert_eq!(events[1]["delta"]["thinking"], "Check");
        assert_eq!(events[2]["delta"]["partial_json"], "{\"command\":\"pwd\"}");
        assert!(parser.is_empty());
    }

    #[test]
    fn claude_stream_rejects_non_object_content_block_without_panic() {
        let start = json!({
            "type": "content_block_start",
            "index": 0,
            "content_block": "x"
        });
        let delta = json!({
            "type": "content_block_delta",
            "index": 0,
            "delta": { "type": "signature_delta", "signature": "sig" }
        });

        let error = claude_content_block_from_start(&start)
            .expect_err("string content_block should fail cleanly");
        assert!(error.message.contains("content_block"));

        // Defense in depth: the delta path must also fail without a panic.
        let mut block = start["content_block"].clone();
        assert_eq!(delta["delta"]["signature"], "sig");
        let error = claude_block_object_mut(&mut block)
            .expect_err("signature delta on a non-object block should fail cleanly");
        assert!(error.message.contains("JSON object"));
    }

    #[test]
    fn claude_request_exposes_native_and_mcp_tools_with_adaptive_effort() {
        let mcp = extensions::McpToolDescriptor {
            server_id: "server-1".to_string(),
            server_name: "Example".to_string(),
            name: "lookup".to_string(),
            wire_name: "mcp__server__lookup__123".to_string(),
            description: "Lookup an item".to_string(),
            input_schema: json!({ "type": "object", "properties": { "id": { "type": "string" } } }),
            approval_mode: "prompt".to_string(),
        };
        let body = build_claude_request_body(
            "claude-opus-4-8",
            "xhigh",
            None,
            vec![json!({ "role": "user", "content": "Inspect" })],
            true,
            false,
            exec::ApprovalPolicy::OnRequest,
            std::slice::from_ref(&mcp),
        );
        assert_eq!(body["thinking"]["type"], "adaptive");
        assert_eq!(body["output_config"]["effort"], "xhigh");
        assert_eq!(body["max_tokens"], 64_000);
        let tools = body["tools"].as_array().expect("tools should be present");
        let names = tools
            .iter()
            .filter_map(|tool| tool.get("name").and_then(Value::as_str))
            .collect::<Vec<_>>();
        assert!(names.contains(&"shell_command"));
        assert!(names.contains(&"view_image"));
        assert!(names.contains(&"update_plan"));
        assert!(names.contains(&"spawn_agent"));
        assert!(names.contains(&"apply_patch"));
        assert!(names.contains(&"mcp__server__lookup__123"));

        let plan = build_claude_request_body(
            "claude-opus-4-8",
            "high",
            None,
            vec![json!({ "role": "user", "content": "Plan" })],
            true,
            true,
            exec::ApprovalPolicy::ReadOnly,
            &[mcp],
        );
        let plan_names = plan["tools"]
            .as_array()
            .expect("plan tools should be present")
            .iter()
            .filter_map(|tool| tool.get("name").and_then(Value::as_str))
            .collect::<Vec<_>>();
        assert!(plan_names.contains(&"mcp__server__lookup__123"));
        assert!(!plan_names.contains(&"apply_patch"));
    }

    #[test]
    fn claude_chat_mode_keeps_native_web_search_and_web_citations() {
        let body = build_claude_request_body(
            "claude-sonnet-4-6",
            "medium",
            None,
            vec![json!({ "role": "user", "content": "Search the web" })],
            false,
            false,
            exec::ApprovalPolicy::ReadOnly,
            &[],
        );
        assert_eq!(
            body["tools"],
            json!([{
                "type": "web_search_20250305",
                "name": "web_search"
            }])
        );

        let mut citations = Vec::new();
        collect_claude_web_citation(
            &json!({
                "type": "web_search_result_location",
                "title": "Claude web search",
                "url": "https://example.com/claude",
                "cited_text": "Current information",
                "encrypted_index": "opaque"
            }),
            &mut citations,
        );
        assert_eq!(citations.len(), 1);
        assert_eq!(citations[0].title, "Claude web search");
        assert_eq!(citations[0].url, "https://example.com/claude");
    }

    #[test]
    fn claude_plan_mode_keeps_read_only_shell_and_task_plan_without_writers() {
        let body = build_claude_request_body(
            "claude-sonnet-4-6",
            "high",
            Some("Plan a migration"),
            vec![json!({ "role": "user", "content": "Inspect first" })],
            true,
            true,
            exec::ApprovalPolicy::ReadOnly,
            &[],
        );
        let names = body["tools"]
            .as_array()
            .expect("tools should be present")
            .iter()
            .filter_map(|tool| tool.get("name").and_then(Value::as_str))
            .collect::<Vec<_>>();
        assert_eq!(
            names,
            vec!["web_search", "shell_command", "view_image", "update_plan"]
        );
        assert!(!names.contains(&"apply_patch"));
        assert!(!names.contains(&"spawn_agent"));
        assert!(body["system"][0]["text"]
            .as_str()
            .is_some_and(|instructions| instructions.contains(PLAN_MODE_INSTRUCTIONS)));
    }

    #[test]
    fn claude_tool_replay_and_usage_preserve_native_shapes() {
        let call = claude_tool_call(
            &json!({ "type": "tool_use", "id": "toolu_1", "name": "apply_patch" }),
            json!({ "patch": "*** Begin Patch\n*** End Patch" }),
        )
        .expect("tool call should parse");
        assert_eq!(call.call_id, "toolu_1");
        assert_eq!(call.kind, ToolCallKind::Custom);
        assert_eq!(call.input, "*** Begin Patch\n*** End Patch");

        let usage = claude_usage(
            &json!({
                "input_tokens": 100,
                "cache_creation_input_tokens": 20,
                "cache_read_input_tokens": 30,
                "output_tokens": 7
            }),
            None,
        )
        .expect("usage should parse");
        assert_eq!(usage.input_tokens, 150);
        assert_eq!(usage.cached_tokens, 30);
        assert_eq!(usage.output_tokens, 7);
        assert_eq!(usage.total_tokens, 157);
    }

    #[test]
    fn workspace_lease_allows_parallel_worktrees_but_serializes_one_root() {
        let manager = ChatManager::new();
        let first = manager
            .try_register_request_for_workspace("conversation-a", None, Some("/tmp/worktree-a"))
            .expect("first workspace lease should register");
        let busy = manager
            .try_register_request_for_workspace("conversation-b", None, Some("/tmp/worktree-a"))
            .expect_err("same workspace should serialize turns");
        assert!(busy.contains("Workspace is busy"));

        let parallel = manager
            .try_register_request_for_workspace("conversation-b", None, Some("/tmp/worktree-b"))
            .expect("separate worktree should run in parallel");
        manager.clear_request("conversation-a", &first);
        manager.clear_request("conversation-b", &parallel);

        let after_release = manager
            .try_register_request_for_workspace("conversation-c", None, Some("/tmp/worktree-a"))
            .expect("workspace lease should release with the request");
        manager.clear_request("conversation-c", &after_release);
    }
}
