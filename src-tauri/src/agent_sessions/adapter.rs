use super::{
    claude, codex, google, kimi, AgentEvent, AgentKind, AgentSessionResult, LaunchSpec,
    SessionLane, TurnUsage,
};
use crate::{extensions, ponytail::PonytailMode};
use serde::Serialize;
use serde_json::Value;
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    process::{Child, ChildStderr, ChildStdin, ChildStdout},
    time::{Instant, SystemTime},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CanonicalMode {
    Ask,
    ReadOnly,
    OnRequest,
    OnFailure,
    Never,
    SkipAll,
}

impl CanonicalMode {
    pub(super) const fn as_str(self) -> &'static str {
        match self {
            Self::Ask => "ask",
            Self::ReadOnly => "read_only",
            Self::OnRequest => "on_request",
            Self::OnFailure => "on_failure",
            Self::Never => "never",
            Self::SkipAll => "skip_all",
        }
    }
}

impl std::fmt::Display for CanonicalMode {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct CodexApprovalSettings {
    pub approval_policy: &'static str,
    pub sandbox: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct PermissionFlags {
    pub args: Vec<OsString>,
    pub config_overrides: Vec<OsString>,
    pub codex_settings: Option<CodexApprovalSettings>,
}

impl PermissionFlags {
    pub(super) fn empty() -> Self {
        Self {
            args: Vec::new(),
            config_overrides: Vec::new(),
            codex_settings: None,
        }
    }

    pub fn cli_args(&self) -> Vec<OsString> {
        let mut args = self.args.clone();
        for config_override in &self.config_overrides {
            args.push(OsString::from("-c"));
            args.push(config_override.clone());
        }
        args
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct AgentProbe {
    pub found: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub binary: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// The coordinator instructions that Halv injects when task routing is on.
pub(super) const COORDINATOR_INSTRUCTIONS: &str = include_str!("coordinator_instructions.md");

/// The Workflow routing section (ADR 0014). It goes after "Select the flow".
pub(super) const WORKFLOW_ROUTING_INSTRUCTIONS: &str =
    include_str!("workflow_routing_instructions.md");

const WORKFLOW_ROUTING_ANCHOR: &str = "\n## Codex hierarchy";

/// What the routing MCP tools and the coordinator instructions need from the
/// session.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct TaskRoutingContext<'a> {
    pub enabled: bool,
    pub coordinator_model: Option<&'a str>,
    pub conversation_id: Option<&'a str>,
    pub worker_lane: crate::preferences::WorkerLanePreference,
    pub tier_agents: crate::task_routing::TierAgentAssignments,
    /// Claude Code Workflow agents get JEV routes (ADR 0014).
    pub workflow_routing: bool,
}

impl<'a> TaskRoutingContext<'a> {
    /// Returns the coordinator instructions when the preference is on.
    pub(super) fn instructions(&self) -> Option<String> {
        if !self.enabled {
            return None;
        }
        let base = COORDINATOR_INSTRUCTIONS.trim_end();
        if !self.workflow_routing {
            return Some(base.to_string());
        }
        let workflow = WORKFLOW_ROUTING_INSTRUCTIONS.trim_end();
        Some(match base.split_once(WORKFLOW_ROUTING_ANCHOR) {
            Some((head, tail)) => {
                format!("{head}\n\n{workflow}\n{WORKFLOW_ROUTING_ANCHOR}{tail}")
            }
            None => format!("{base}\n\n{workflow}"),
        })
    }

    /// Keeps only the routing that the vendor of the pane can use. A
    /// conversation model of the other vendor is not a valid coordinator
    /// hint. A Codex pane has no Workflow tool.
    pub(super) fn for_lane(self, lane: crate::task_routing::Lane) -> Self {
        Self {
            coordinator_model: self
                .coordinator_model
                .filter(|model| crate::task_routing::lane_for_model(model) == lane),
            workflow_routing: self.workflow_routing && lane == crate::task_routing::Lane::Claude,
            ..self
        }
    }
}

pub(super) struct LaunchContext<'a> {
    pub cwd: &'a Path,
    pub lane: &'a SessionLane,
    pub permission_mode: CanonicalMode,
    pub resume_cli_session_id: Option<&'a str>,
    pub proxy_port: Option<u16>,
    pub project: &'a str,
    pub savings_database_path: &'a Path,
    pub model: Option<&'a str>,
    pub effort: Option<&'a str>,
    pub nested_codex_home: Option<&'a Path>,
    pub ponytail_mode: PonytailMode,
    pub extension_manager: &'a extensions::ExtensionManager,
}

pub(super) struct InteractiveLaunchContext<'a> {
    pub cwd: &'a Path,
    pub lane: &'a SessionLane,
    pub permission_mode: CanonicalMode,
    pub resume_cli_session_id: Option<&'a str>,
    pub assigned_cli_session_id: Option<&'a str>,
    pub proxy_port: Option<u16>,
    pub project: &'a str,
    pub savings_database_path: &'a Path,
    pub nested_codex_home: Option<&'a Path>,
    pub ponytail_mode: PonytailMode,
    pub claude_subscription_proxy: bool,
    pub model: Option<&'a str>,
    pub conversation_id: Option<&'a str>,
    pub task_routing: bool,
    pub worker_lane: crate::preferences::WorkerLanePreference,
    pub tier_agents: crate::task_routing::TierAgentAssignments,
    /// JEV routes Claude Code Workflow agents (ADR 0014).
    pub workflow_routing: bool,
    /// A Claude pane starts with ultracode on.
    pub claude_ultracode: bool,
    pub extension_manager: &'a extensions::ExtensionManager,
    /// The launch runs in a Halv PTY pane, which tracks the agent status.
    /// An external terminal gets no status hooks or notification flags.
    pub agent_status: bool,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct InteractiveTranscriptQuery<'a> {
    pub cwd: &'a Path,
    pub started_at: SystemTime,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct InteractiveTranscript {
    pub cli_session_id: String,
    pub path: PathBuf,
    pub started_at: SystemTime,
    pub modified_at: SystemTime,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum InteractiveTranscriptDiscovery {
    Found(Vec<InteractiveTranscript>),
    Unsupported(&'static str),
}

impl<'a> InteractiveLaunchContext<'a> {
    pub(super) fn routing(&self) -> TaskRoutingContext<'a> {
        TaskRoutingContext {
            enabled: self.task_routing,
            coordinator_model: self.model,
            conversation_id: self.conversation_id,
            worker_lane: self.worker_lane,
            tier_agents: self.tier_agents.clone(),
            workflow_routing: self.workflow_routing,
        }
    }
}

pub(super) enum TurnKind<'a> {
    Start {
        cli_session_id: Option<&'a str>,
    },
    Steer {
        cli_session_id: &'a str,
        turn_id: Option<&'a str>,
    },
    Queued {
        cli_session_id: &'a str,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct PendingPermission {
    pub request_id: String,
    pub rpc_id: Value,
    pub method: String,
    pub params: Value,
}

#[derive(Debug, Default, PartialEq)]
pub(super) struct ParsedTurn {
    pub events: Vec<AgentEvent>,
    pub responses: Vec<String>,
    pub pending_permissions: Vec<PendingPermission>,
}

pub(super) struct AdapterProcess {
    pub child: Option<Child>,
    pub stdin: Option<ChildStdin>,
    pub stdout: Option<ChildStdout>,
    pub stderr: Option<ChildStderr>,
    pub pid: u32,
}

impl AdapterProcess {
    pub(super) fn persistent(mut child: Child, display_name: &str) -> AgentSessionResult<Self> {
        let stdin = child.stdin.take().ok_or_else(|| {
            stop_child(&mut child);
            format!("the {display_name} process did not expose stdin")
        })?;
        let stdout = child.stdout.take().ok_or_else(|| {
            stop_child(&mut child);
            format!("the {display_name} process did not expose stdout")
        })?;
        let stderr = child.stderr.take();
        let pid = child.id();
        Ok(Self {
            child: Some(child),
            stdin: Some(stdin),
            stdout: Some(stdout),
            stderr,
            pid,
        })
    }

    pub(super) const fn deferred() -> Self {
        Self {
            child: None,
            stdin: None,
            stdout: None,
            stderr: None,
            pid: 0,
        }
    }

    pub(super) fn per_turn(mut child: Child, display_name: &str) -> AgentSessionResult<Self> {
        let stdout = child.stdout.take().ok_or_else(|| {
            stop_child(&mut child);
            format!("the {display_name} process did not expose stdout")
        })?;
        let stderr = child.stderr.take();
        let pid = child.id();
        Ok(Self {
            child: Some(child),
            stdin: None,
            stdout: Some(stdout),
            stderr,
            pid,
        })
    }

    pub(super) fn stop(&mut self) {
        if let Some(child) = &mut self.child {
            stop_child(child);
        }
        self.child.take();
    }

    #[cfg(test)]
    pub(super) const fn in_memory() -> Self {
        Self::deferred()
    }
}

fn stop_child(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

pub(super) enum AdapterState {
    Claude(claude::ClaudeAdapterState),
    Codex(codex::CodexAdapterState),
    Google(google::GoogleAdapterState),
    Kimi(kimi::KimiAdapterState),
}

impl AdapterState {
    pub(super) fn has_init_timeout(&self) -> bool {
        matches!(self, Self::Codex(state) if state.init.is_some())
    }

    pub(super) fn begin_init_flush(&mut self) -> bool {
        match self {
            Self::Codex(state) => state.begin_init_flush(),
            Self::Claude(_) | Self::Google(_) | Self::Kimi(_) => true,
        }
    }

    pub(super) fn remember_protocol_error(&mut self, message: &str) {
        if let Self::Codex(state) = self {
            state.remember_protocol_error(message);
        }
    }

    pub(super) fn next_queued_message(&mut self) -> Option<String> {
        match self {
            Self::Codex(state) => state.next_queued_message(),
            Self::Claude(_) | Self::Google(_) | Self::Kimi(_) => None,
        }
    }

    pub(super) fn mark_init_failed(&mut self) {
        if let Self::Codex(state) = self {
            state.mark_init_failed();
        }
    }

    pub(super) fn init_started_at(&self) -> Option<Instant> {
        match self {
            Self::Codex(state) => state.init_started_at(),
            Self::Claude(_) | Self::Google(_) | Self::Kimi(_) => None,
        }
    }

    pub(super) fn init_timeout(&mut self, now: Instant) -> Option<super::CodexInitTimeout> {
        match self {
            Self::Codex(state) => state.init_timeout(now),
            Self::Claude(_) | Self::Google(_) | Self::Kimi(_) => None,
        }
    }

    pub(super) fn register_pending_permission(&mut self, request: PendingPermission) {
        if let Self::Codex(state) = self {
            state
                .pending_permissions
                .insert(request.request_id.clone(), request);
        }
    }

    pub(super) fn permission_response(
        &self,
        request_id: &str,
        approve: bool,
    ) -> AgentSessionResult<String> {
        let Self::Codex(state) = self else {
            return Err("this agent adapter does not support permission responses".to_string());
        };
        let request = state.pending_permissions.get(request_id).ok_or_else(|| {
            format!("permission request `{request_id}` is unknown or already resolved")
        })?;
        codex::permission_response(request, approve)
    }

    pub(super) fn remove_pending_permission(&mut self, request_id: &str) {
        if let Self::Codex(state) = self {
            state.pending_permissions.remove(request_id);
        }
    }

    pub(super) fn pending_permissions_empty(&self) -> bool {
        match self {
            Self::Codex(state) => state.pending_permissions.is_empty(),
            Self::Claude(_) | Self::Google(_) | Self::Kimi(_) => true,
        }
    }

    pub(super) fn drain_pending_permission_events(&mut self) -> Vec<AgentEvent> {
        let Self::Codex(state) = self else {
            return Vec::new();
        };
        let mut request_ids = state
            .pending_permissions
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

    pub(super) fn set_active_turn(&mut self, turn_id: Option<String>) {
        if let Self::Codex(state) = self {
            state.active_turn_id = turn_id;
        }
    }

    pub(super) fn active_turn_id(&self) -> Option<&str> {
        match self {
            Self::Codex(state) => state.active_turn_id.as_deref(),
            Self::Claude(_) | Self::Google(_) | Self::Kimi(_) => None,
        }
    }
}

pub(super) trait AgentAdapter: Send + Sync {
    fn kind(&self) -> AgentKind;
    fn probe(&self) -> AgentProbe;
    /// The CLI binary that `probe` runs. The probe cache keys its results on
    /// this file. `None` turns the cache off for the adapter.
    fn binary(&self) -> Option<PathBuf> {
        None
    }
    fn permission_flags(&self, mode: CanonicalMode) -> AgentSessionResult<PermissionFlags>;
    fn login_launch_spec(&self) -> AgentSessionResult<LaunchSpec>;
    fn interactive_launch_spec(
        &self,
        ctx: &InteractiveLaunchContext<'_>,
    ) -> AgentSessionResult<LaunchSpec>;
    fn start(
        &self,
        ctx: &LaunchContext<'_>,
        state: &mut AdapterState,
    ) -> AgentSessionResult<AdapterProcess>;
    fn new_state(
        &self,
        resumed: bool,
        permission_mode: CanonicalMode,
        started_at: Instant,
        initializing: bool,
    ) -> AdapterState;
    fn parse_line(&self, state: &mut AdapterState, line: &str) -> ParsedTurn;
    fn encode_user_message(
        &self,
        state: &mut AdapterState,
        text: &str,
        turn: TurnKind<'_>,
    ) -> AgentSessionResult<Option<String>>;

    fn start_turn(
        &self,
        _state: &mut AdapterState,
        _text: &str,
        _resume_cli_session_id: Option<&str>,
    ) -> AgentSessionResult<AdapterProcess> {
        Err("this agent adapter does not use one process per turn".to_string())
    }

    fn is_per_turn(&self) -> bool {
        false
    }

    fn emits_turn_started(&self) -> bool {
        false
    }

    fn requires_session_discovery(&self) -> bool {
        false
    }

    fn assigned_interactive_session_id(&self, resume_cli_session_id: Option<&str>) -> bool {
        resume_cli_session_id.is_none()
    }

    fn uses_rtk_instruction(&self) -> bool {
        false
    }

    fn supports_nested_codex_home(&self, _proxy_shutdown_on_exit: bool) -> bool {
        true
    }

    fn isolated_proxy_upstream(&self, _lane: &SessionLane) -> Option<&'static str> {
        None
    }

    fn accepts_initial_prompt(&self) -> bool {
        true
    }

    fn initial_prompt_args(&self, prompt: &str) -> Vec<OsString> {
        vec![OsString::from(prompt)]
    }

    fn discover_interactive_transcripts(
        &self,
        _query: InteractiveTranscriptQuery<'_>,
    ) -> AgentSessionResult<InteractiveTranscriptDiscovery> {
        Ok(InteractiveTranscriptDiscovery::Found(Vec::new()))
    }

    fn prepare_usage(&self, _state: &mut AdapterState, _cli_session_id: Option<&str>) {}

    fn measured_usage(
        &self,
        _state: &mut AdapterState,
        _cli_session_id: Option<&str>,
    ) -> Option<TurnUsage> {
        None
    }
}

pub(super) fn launch_spec_with_prompt(
    adapter: &dyn AgentAdapter,
    ctx: &InteractiveLaunchContext<'_>,
    initial_prompt: Option<&str>,
) -> AgentSessionResult<LaunchSpec> {
    let mut spec = adapter.interactive_launch_spec(ctx)?;
    if adapter.accepts_initial_prompt() && ctx.resume_cli_session_id.is_none() {
        if let Some(prompt) = initial_prompt
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            spec.args.extend(adapter.initial_prompt_args(prompt));
        }
    }
    Ok(spec)
}

#[cfg(test)]
mod routing_context_tests {
    use super::TaskRoutingContext;
    use crate::task_routing::Lane;

    #[test]
    fn the_workflow_section_follows_the_flow_selection() {
        let context = TaskRoutingContext {
            enabled: true,
            workflow_routing: true,
            ..TaskRoutingContext::default()
        };
        let text = context.instructions().expect("routing is on");
        let flow = text.find("## Select the flow").unwrap();
        let workflow = text.find("## Workflow routing").unwrap();
        let hierarchy = text.find("## Codex hierarchy").unwrap();
        assert!(flow < workflow && workflow < hierarchy);
        assert!(text.contains("`args.jev[key].retry`"));

        let legacy = TaskRoutingContext {
            workflow_routing: false,
            ..context.clone()
        };
        assert!(!legacy.instructions().unwrap().contains("Workflow routing"));
        assert_eq!(TaskRoutingContext::default().instructions(), None);
    }

    #[test]
    fn a_pane_keeps_only_the_routing_of_its_vendor() {
        let context = TaskRoutingContext {
            enabled: true,
            coordinator_model: Some("gpt-6-astra"),
            workflow_routing: true,
            ..TaskRoutingContext::default()
        };
        let claude = context.clone().for_lane(Lane::Claude);
        assert_eq!(claude.coordinator_model, None);
        assert!(claude.workflow_routing);
        let codex = context.for_lane(Lane::Codex);
        assert_eq!(codex.coordinator_model, Some("gpt-6-astra"));
        assert!(!codex.workflow_routing);
    }
}
