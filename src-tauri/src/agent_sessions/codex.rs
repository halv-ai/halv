use super::{
    adapter::{
        AdapterProcess, AdapterState, AgentAdapter, AgentProbe, CanonicalMode,
        CodexApprovalSettings, InteractiveLaunchContext, LaunchContext, ParsedTurn,
        PendingPermission, PermissionFlags, TaskRoutingContext, TurnKind,
    },
    apply_savings_stack_environment, canonical_permission_mode,
    cli_binary::{self, HostOs},
    command_environment,
    install::{self, ManagedAgent},
    session_files, AgentEvent, AgentKind, CodexInitState, CodexInitTimeout,
    CodexMessageDisposition, LaunchSpec, SessionLane, TurnUsage, RTK_DEVELOPER_INSTRUCTIONS,
};
use crate::{
    host_env,
    ponytail::{self, PonytailMode},
};
use serde::Serialize;
use serde_json::{json, Value};
use std::{
    collections::{HashMap, HashSet},
    env,
    ffi::{OsStr, OsString},
    fs,
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::{Child, ChildStdin, Command, Stdio},
    time::{Duration, SystemTime},
};

const CODEX_BINARY_NAME: &str = "codex";
const EXECUTION_DEVELOPER_INSTRUCTIONS: &str = include_str!("execution_instructions.md");
const HALV_PROVIDER_NAME: &str = "halv_headroom";
const INITIALIZE_REQUEST_ID: u64 = 1;
const THREAD_REQUEST_ID: u64 = 2;
pub(super) const FIRST_TURN_REQUEST_ID: usize = 3;

/// `port` is `None` when the Halv Engine is off. The home then gets no Halv
/// provider block, and nested workers use the provider of the user's config.
pub(super) fn prepare_nested_home(port: Option<u16>, project: &str) -> Result<PathBuf, String> {
    // The nested home mirrors the active wallet account, so a nested Codex
    // worker uses the same login as a direct Codex launch.
    prepare_nested_home_from(codex_home(), port, project)
}

fn prepare_nested_home_from(
    source_home: Option<PathBuf>,
    port: Option<u16>,
    project: &str,
) -> Result<PathBuf, String> {
    let session_home =
        session_files::directory(project)?.join(session_files::CODEX_HOME_DIRECTORY_NAME);
    fs::create_dir(&session_home).map_err(|error| {
        format!(
            "failed to create the session Codex home `{}`: {error}",
            session_home.display()
        )
    })?;
    restrict_directory_permissions(&session_home)?;

    let result = (|| {
        let mut config = source_home
            .as_ref()
            .map(|home| home.join("config.toml"))
            .filter(|path| path.is_file())
            .and_then(|path| fs::read_to_string(path).ok())
            .and_then(|content| content.parse::<toml::Table>().ok())
            .unwrap_or_default();
        if let Some(port) = port {
            configure_nested_provider(&mut config, port);
        }
        let config_text = toml::to_string_pretty(&config)
            .map_err(|error| format!("failed to encode the session Codex config: {error}"))?;
        let config_path = session_home.join("config.toml");
        crate::exec::apply_patch::atomic_write_file(&config_path, config_text.as_bytes()).map_err(
            |error| {
                format!(
                    "failed to write the session Codex config `{}`: {error}",
                    config_path.display()
                )
            },
        )?;
        restrict_file_permissions(&config_path)?;

        if let Some(source_home) = source_home.as_deref().filter(|path| path.is_dir()) {
            mirror_codex_home_entries(source_home, &session_home)?;
        }
        Ok(())
    })();

    if let Err(error) = result {
        let _ = fs::remove_dir_all(&session_home);
        return Err(error);
    }
    Ok(session_home)
}

pub(super) fn remove_nested_home(path: &Path) {
    if let Err(error) = fs::remove_dir_all(path) {
        eprintln!(
            "failed to remove the session Codex home `{}`: {error}",
            path.display()
        );
    }
}

fn configure_nested_provider(config: &mut toml::Table, port: u16) {
    let base_url = format!("http://127.0.0.1:{port}/v1");
    config.insert(
        "model_provider".to_string(),
        toml::Value::String(HALV_PROVIDER_NAME.to_string()),
    );
    config.insert(
        "openai_base_url".to_string(),
        toml::Value::String(base_url.clone()),
    );

    let providers = config
        .entry("model_providers".to_string())
        .or_insert_with(|| toml::Value::Table(toml::Table::new()));
    if !providers.is_table() {
        *providers = toml::Value::Table(toml::Table::new());
    }
    let providers = providers
        .as_table_mut()
        .expect("model providers value was replaced with a table");
    let mut headers = toml::Table::new();
    headers.insert(
        "X-Headroom-Project".to_string(),
        toml::Value::String("HEADROOM_PROJECT".to_string()),
    );
    let mut provider = toml::Table::new();
    provider.insert(
        "name".to_string(),
        toml::Value::String("OpenAI via Halv Headroom".to_string()),
    );
    provider.insert("base_url".to_string(), toml::Value::String(base_url));
    provider.insert(
        "supports_websockets".to_string(),
        toml::Value::Boolean(true),
    );
    provider.insert(
        "requires_openai_auth".to_string(),
        toml::Value::Boolean(true),
    );
    provider.insert("env_http_headers".to_string(), toml::Value::Table(headers));
    providers.insert(HALV_PROVIDER_NAME.to_string(), toml::Value::Table(provider));
}

fn mirror_codex_home_entries(source_home: &Path, session_home: &Path) -> Result<(), String> {
    let entries = fs::read_dir(source_home).map_err(|error| {
        format!(
            "failed to inspect the Codex home `{}`: {error}",
            source_home.display()
        )
    })?;
    for entry in entries {
        let entry = entry.map_err(|error| {
            format!(
                "failed to inspect an entry in the Codex home `{}`: {error}",
                source_home.display()
            )
        })?;
        if !should_mirror_codex_home_entry(&entry.file_name()) {
            continue;
        }
        let source = entry.path();
        let target = session_home.join(entry.file_name());
        let file_type = entry.file_type().map_err(|error| {
            format!(
                "failed to inspect the Codex home entry `{}`: {error}",
                source.display()
            )
        })?;
        if link_codex_home_entry(&source, &target, file_type.is_dir()).is_ok() {
            continue;
        }
        if file_type.is_file() {
            fs::copy(&source, &target).map_err(|error| {
                format!(
                    "failed to copy the Codex home entry `{}`: {error}",
                    source.display()
                )
            })?;
            restrict_file_permissions(&target)?;
        }
    }
    Ok(())
}

fn should_mirror_codex_home_entry(name: &OsStr) -> bool {
    matches!(
        name.to_str(),
        Some(
            "auth.json"
                | "AGENTS.md"
                | "models_cache.json"
                | "version.json"
                | "installation_id"
                | ".personality_migration"
                | ".sandbox_migration"
                | "skills"
                | "plugins"
                | "rules"
                | "memories"
                | "vendor_imports"
        )
    )
}

#[cfg(unix)]
fn link_codex_home_entry(source: &Path, target: &Path, _is_directory: bool) -> std::io::Result<()> {
    std::os::unix::fs::symlink(source, target)
}

#[cfg(windows)]
fn link_codex_home_entry(source: &Path, target: &Path, is_directory: bool) -> std::io::Result<()> {
    if is_directory {
        std::os::windows::fs::symlink_dir(source, target)
    } else {
        std::os::windows::fs::symlink_file(source, target)
    }
}

#[cfg(unix)]
fn restrict_directory_permissions(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;

    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(|error| {
        format!(
            "failed to protect the session Codex home `{}`: {error}",
            path.display()
        )
    })
}

#[cfg(not(unix))]
fn restrict_directory_permissions(_path: &Path) -> Result<(), String> {
    Ok(())
}

#[cfg(unix)]
fn restrict_file_permissions(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;

    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).map_err(|error| {
        format!(
            "failed to protect the session Codex file `{}`: {error}",
            path.display()
        )
    })
}

#[cfg(not(unix))]
fn restrict_file_permissions(_path: &Path) -> Result<(), String> {
    Ok(())
}

pub(super) fn nested_usage(codex_home: &Path) -> TurnUsage {
    let mut paths = Vec::new();
    collect_rollout_paths(&codex_home.join("sessions"), &mut paths);
    let mut total = TurnUsage::default();
    for path in paths {
        let Some(usage) = rollout_usage(&path) else {
            continue;
        };
        total.input_tokens = total.input_tokens.saturating_add(usage.input_tokens);
        total.output_tokens = total.output_tokens.saturating_add(usage.output_tokens);
        total.cache_read_tokens = total
            .cache_read_tokens
            .saturating_add(usage.cache_read_tokens);
        total.cache_creation_tokens = total
            .cache_creation_tokens
            .saturating_add(usage.cache_creation_tokens);
    }
    total
}

fn collect_rollout_paths(directory: &Path, paths: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_dir() {
            collect_rollout_paths(&path, paths);
        } else if file_type.is_file()
            && path.extension().and_then(|extension| extension.to_str()) == Some("jsonl")
        {
            paths.push(path);
        }
    }
}

fn rollout_usage(path: &Path) -> Option<TurnUsage> {
    let file = fs::File::open(path).ok()?;
    let mut latest = None;
    for line in BufReader::new(file).lines().map_while(Result::ok) {
        let Ok(value) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let Some(payload) = value.get("payload") else {
            continue;
        };
        if value.get("type").and_then(Value::as_str) != Some("event_msg")
            || payload.get("type").and_then(Value::as_str) != Some("token_count")
        {
            continue;
        }
        let Some(usage) = payload
            .get("info")
            .and_then(|info| info.get("total_token_usage"))
        else {
            continue;
        };
        latest = Some(TurnUsage {
            input_tokens: token_field(usage, "input_tokens", "inputTokens"),
            output_tokens: token_field(usage, "output_tokens", "outputTokens"),
            cache_read_tokens: token_field(usage, "cached_input_tokens", "cachedInputTokens"),
            cache_creation_tokens: token_field(
                usage,
                "cache_write_input_tokens",
                "cacheWriteInputTokens",
            ),
            total_cost_usd: None,
        });
    }
    latest
}

type CodexParseResult = ParsedTurn;
type CodexPermissionRequest = PendingPermission;

#[derive(Default)]
struct CodexEventParser {
    session_init_emitted: bool,
    tool_names: HashMap<String, String>,
    turn_usage: HashMap<String, TurnUsage>,
    hold_permission_requests: bool,
}

impl CodexEventParser {
    pub(super) fn with_permission_mode(permission_mode: &str) -> Self {
        Self {
            hold_permission_requests: approval_settings(permission_mode).approval_policy != "never",
            ..Self::default()
        }
    }

    fn parse_line(&mut self, line: &str) -> CodexParseResult {
        let value = match serde_json::from_str::<Value>(line) {
            Ok(value) => value,
            Err(error) => {
                return CodexParseResult {
                    events: vec![AgentEvent::ProtocolError {
                        message: format!("invalid Codex JSON-RPC JSON: {error}"),
                    }],
                    responses: Vec::new(),
                    pending_permissions: Vec::new(),
                };
            }
        };

        if value.get("method").is_some() {
            if value.get("id").is_some() {
                return self.parse_server_request(&value);
            }
            return CodexParseResult {
                events: self.parse_notification(&value),
                responses: Vec::new(),
                pending_permissions: Vec::new(),
            };
        }

        CodexParseResult {
            events: self.parse_response(&value),
            responses: Vec::new(),
            pending_permissions: Vec::new(),
        }
    }

    fn parse_response(&mut self, value: &Value) -> Vec<AgentEvent> {
        if let Some(error) = value.get("error") {
            let message = error
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("Codex app-server returned a JSON-RPC error");
            return vec![AgentEvent::ProtocolError {
                message: message.to_string(),
            }];
        }

        if request_id(value.get("id")) != Some(THREAD_REQUEST_ID) {
            return Vec::new();
        }
        let Some(result) = value.get("result") else {
            return Vec::new();
        };
        self.session_init(result).into_iter().collect()
    }

    fn parse_notification(&mut self, value: &Value) -> Vec<AgentEvent> {
        let method = value.get("method").and_then(Value::as_str);
        let params = value.get("params").unwrap_or(&Value::Null);
        match method {
            Some("turn/started") => vec![AgentEvent::TurnStarted {
                turn_id: params
                    .get("turn")
                    .and_then(|turn| turn.get("id"))
                    .and_then(Value::as_str)
                    .map(str::to_string),
            }],
            Some("item/agentMessage/delta") => params
                .get("delta")
                .and_then(Value::as_str)
                .map(|text| {
                    vec![AgentEvent::TextDelta {
                        text: text.to_string(),
                    }]
                })
                .unwrap_or_default(),
            Some("item/started") => self.parse_item_started(params).into_iter().collect(),
            Some("item/completed") => self.parse_item_completed(params).into_iter().collect(),
            Some("thread/tokenUsage/updated") => {
                self.remember_turn_usage(params);
                Vec::new()
            }
            Some("turn/completed") => vec![self.parse_turn_completed(params)],
            Some("error") => params
                .get("error")
                .and_then(|error| error.get("message"))
                .and_then(Value::as_str)
                .map(|message| {
                    vec![AgentEvent::ProtocolError {
                        message: message.to_string(),
                    }]
                })
                .unwrap_or_default(),
            _ => Vec::new(),
        }
    }

    fn session_init(&mut self, value: &Value) -> Option<AgentEvent> {
        if self.session_init_emitted {
            return None;
        }
        let thread = value.get("thread")?;
        let cli_session_id = thread.get("id")?.as_str()?.to_string();
        let model = value
            .get("model")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        self.session_init_emitted = true;
        Some(AgentEvent::SessionInit {
            cli_session_id,
            model,
        })
    }

    fn parse_item_started(&mut self, params: &Value) -> Option<AgentEvent> {
        let item = params.get("item")?;
        let (id, name) = tool_item(item)?;
        self.tool_names.insert(id, name.clone());
        Some(AgentEvent::ToolCallStarted {
            name,
            summary_json: item.to_string(),
        })
    }

    fn parse_item_completed(&mut self, params: &Value) -> Option<AgentEvent> {
        let item = params.get("item")?;
        let id = item.get("id").and_then(Value::as_str).unwrap_or_default();
        let name = self
            .tool_names
            .remove(id)
            .or_else(|| tool_item(item).map(|(_, name)| name))?;
        Some(AgentEvent::ToolCallCompleted { name })
    }

    fn remember_turn_usage(&mut self, params: &Value) {
        let Some(turn_id) = params.get("turnId").and_then(Value::as_str) else {
            return;
        };
        let Some(usage) = params.get("tokenUsage").and_then(|usage| usage.get("last")) else {
            return;
        };
        self.turn_usage
            .insert(turn_id.to_string(), parse_usage(usage));
    }

    fn parse_turn_completed(&mut self, params: &Value) -> AgentEvent {
        let turn = params.get("turn").unwrap_or(params);
        let turn_id = turn.get("id").and_then(Value::as_str).unwrap_or_default();
        let usage = turn
            .get("usage")
            .or_else(|| params.get("usage"))
            .map(parse_usage)
            .or_else(|| self.turn_usage.remove(turn_id))
            .unwrap_or_default();
        AgentEvent::TurnCompleted { usage }
    }

    fn parse_server_request(&self, value: &Value) -> CodexParseResult {
        let Some(method) = value.get("method").and_then(Value::as_str) else {
            return CodexParseResult::default();
        };
        let Some(id) = value.get("id") else {
            return CodexParseResult::default();
        };
        let params = value.get("params").cloned().unwrap_or(Value::Null);
        match method {
            "item/commandExecution/requestApproval"
            | "item/fileChange/requestApproval"
            | "execCommandApproval"
            | "applyPatchApproval"
            | "item/permissions/requestApproval" => {}
            _ => return CodexParseResult::default(),
        }
        let request_id = request_id_text(id);
        let description = approval_description(method, &params);
        let pending = CodexPermissionRequest {
            request_id: request_id.clone(),
            rpc_id: id.clone(),
            method: method.to_string(),
            params,
        };
        let responses = if self.hold_permission_requests {
            Vec::new()
        } else {
            vec![permission_response(&pending, true)
                .expect("supported Codex permission request should build a response")]
        };
        let pending_permissions = self
            .hold_permission_requests
            .then_some(pending.clone())
            .into_iter()
            .collect();
        CodexParseResult {
            events: vec![AgentEvent::PermissionRequest {
                request_id,
                description,
                payload_json: pending.params.to_string(),
            }],
            responses,
            pending_permissions,
        }
    }
}

pub(super) fn permission_response(
    request: &CodexPermissionRequest,
    approve: bool,
) -> Result<String, String> {
    let result = match request.method.as_str() {
        "item/commandExecution/requestApproval" | "item/fileChange/requestApproval" => json!({
            "decision": if approve { "accept" } else { "decline" }
        }),
        "execCommandApproval" | "applyPatchApproval" => {
            // codex-cli 0.149.1 app-server JSON schema requires this denial shape:
            // {"denied":{"rejection":"..."}}, with a required rejection string.
            if approve {
                json!({ "decision": "approved" })
            } else {
                json!({
                    "decision": {
                        "denied": { "rejection": "The user denied the request." }
                    }
                })
            }
        }
        "item/permissions/requestApproval" => {
            // The app-server protocol accepts any subset of the requested permissions.
            // An empty subset denies every requested permission.
            let permissions = if approve {
                request
                    .params
                    .get("permissions")
                    .cloned()
                    .unwrap_or_else(|| json!({}))
            } else {
                json!({})
            };
            json!({ "permissions": permissions, "scope": "turn" })
        }
        _ => {
            return Err(format!(
                "unsupported Codex permission request method `{}`",
                request.method
            ));
        }
    };
    Ok(json!({ "id": request.rpc_id, "result": result }).to_string())
}

fn probe() -> AgentProbe {
    let Some(binary) = find_binary() else {
        return AgentProbe {
            found: false,
            binary: None,
            version: None,
            error: None,
        };
    };
    let binary_text = binary.to_string_lossy().into_owned();
    let mut command = cli_binary::command_for(&binary);
    command.arg("--version");
    host_env::apply_to_command(&mut command);
    match command.output() {
        Ok(output) => {
            let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            let version = if stdout.is_empty() {
                (!stderr.is_empty()).then_some(stderr.clone())
            } else {
                Some(stdout)
            };
            let error = (!output.status.success()).then(|| {
                if stderr.is_empty() {
                    format!("codex --version exited with {}", output.status)
                } else {
                    stderr
                }
            });
            AgentProbe {
                found: true,
                binary: Some(binary_text),
                version,
                error,
            }
        }
        Err(error) => {
            let error = format!("failed to run {binary_text} --version: {error}");
            AgentProbe {
                found: true,
                binary: Some(binary_text),
                version: None,
                error: Some(error),
            }
        }
    }
}

pub(super) struct CodexAdapterState {
    parser: CodexEventParser,
    pub(super) pending_permissions: HashMap<String, PendingPermission>,
    pub(super) init: Option<CodexInitState>,
    next_request_id: usize,
    pub(super) active_turn_id: Option<String>,
}

impl CodexAdapterState {
    pub(super) fn begin_init_flush(&mut self) -> bool {
        self.init.as_mut().map_or(true, CodexInitState::begin_flush)
    }

    pub(super) fn remember_protocol_error(&mut self, message: &str) {
        if let Some(init) = &mut self.init {
            init.remember_protocol_error(message);
        }
    }

    pub(super) fn next_queued_message(&mut self) -> Option<String> {
        self.init
            .as_mut()
            .and_then(CodexInitState::next_queued_message)
    }

    pub(super) fn mark_init_failed(&mut self) {
        if let Some(init) = &mut self.init {
            init.mark_failed();
        }
    }

    pub(super) fn init_started_at(&self) -> Option<std::time::Instant> {
        self.init.as_ref().map(|init| init.started_at)
    }

    pub(super) fn init_timeout(&mut self, now: std::time::Instant) -> Option<CodexInitTimeout> {
        self.init.as_mut().and_then(|init| init.timeout(now))
    }

    fn next_request_id(&mut self) -> usize {
        let request_id = self.next_request_id;
        self.next_request_id = self.next_request_id.saturating_add(1);
        request_id
    }
}

pub(super) struct CodexAdapter;
pub(super) static CODEX_ADAPTER: CodexAdapter = CodexAdapter;

fn permission_flags(
    agent_kind: AgentKind,
    permission_mode: &str,
) -> Result<PermissionFlags, String> {
    if agent_kind != AgentKind::Codex {
        return Err("Codex permission mapping received an incompatible agent kind".to_string());
    }
    CODEX_ADAPTER.permission_flags(canonical_permission_mode(permission_mode)?)
}

impl AgentAdapter for CodexAdapter {
    fn kind(&self) -> AgentKind {
        AgentKind::Codex
    }

    fn probe(&self) -> AgentProbe {
        probe()
    }

    fn binary(&self) -> Option<PathBuf> {
        find_binary()
    }

    fn permission_flags(&self, mode: CanonicalMode) -> Result<PermissionFlags, String> {
        let (args, config_overrides, approval_policy, sandbox) = match mode {
            CanonicalMode::ReadOnly => (
                vec!["--sandbox", "read-only", "--ask-for-approval", "never"],
                Vec::new(),
                "never",
                "read-only",
            ),
            CanonicalMode::Ask => (
                vec![
                    "--sandbox",
                    "workspace-write",
                    "--ask-for-approval",
                    "on-request",
                ],
                Vec::new(),
                "on-request",
                "workspace-write",
            ),
            CanonicalMode::OnRequest => (
                vec![
                    "--sandbox",
                    "workspace-write",
                    "--ask-for-approval",
                    "on-request",
                ],
                vec!["approvals_reviewer=\"auto_review\""],
                "on-request",
                "workspace-write",
            ),
            CanonicalMode::OnFailure => (
                Vec::new(),
                vec![
                    "approval_policy=\"on-failure\"",
                    "sandbox_mode=\"workspace-write\"",
                ],
                "on-failure",
                "workspace-write",
            ),
            CanonicalMode::Never => (
                vec![
                    "--sandbox",
                    "workspace-write",
                    "--ask-for-approval",
                    "never",
                ],
                Vec::new(),
                "never",
                "workspace-write",
            ),
            CanonicalMode::SkipAll => (vec!["--yolo"], Vec::new(), "never", "danger-full-access"),
        };
        Ok(PermissionFlags {
            args: args.into_iter().map(OsString::from).collect(),
            config_overrides: config_overrides.into_iter().map(OsString::from).collect(),
            codex_settings: Some(CodexApprovalSettings {
                approval_policy,
                sandbox,
            }),
        })
    }

    fn login_launch_spec(&self) -> Result<LaunchSpec, String> {
        login_launch_spec()
    }

    fn interactive_launch_spec(
        &self,
        ctx: &InteractiveLaunchContext<'_>,
    ) -> Result<LaunchSpec, String> {
        interactive_launch_spec(
            ctx.cwd,
            ctx.lane,
            ctx.permission_mode.as_str(),
            ctx.resume_cli_session_id,
            ctx.proxy_port,
            ctx.project,
            ctx.savings_database_path,
            ctx.ponytail_mode,
            ctx.routing().for_lane(crate::task_routing::Lane::Codex),
            ctx.extension_manager,
            ctx.agent_status,
        )
    }

    fn start(
        &self,
        ctx: &LaunchContext<'_>,
        _state: &mut AdapterState,
    ) -> Result<AdapterProcess, String> {
        let child = spawn(
            ctx.cwd,
            ctx.lane,
            ctx.permission_mode.as_str(),
            ctx.proxy_port,
            ctx.project,
            ctx.savings_database_path,
            ctx.ponytail_mode,
            TaskRoutingContext {
                coordinator_model: ctx.model,
                ..TaskRoutingContext::default()
            },
            ctx.extension_manager,
        )?;
        let mut process = AdapterProcess::persistent(child, "Codex")?;
        if let Err(error) = write_startup(
            process
                .stdin
                .as_mut()
                .expect("persistent Codex process should have stdin"),
            ctx.cwd,
            ctx.permission_mode.as_str(),
            ctx.resume_cli_session_id,
            ctx.model,
        ) {
            process.stop();
            return Err(error);
        }
        Ok(process)
    }

    fn new_state(
        &self,
        _resumed: bool,
        permission_mode: CanonicalMode,
        started_at: std::time::Instant,
        initializing: bool,
    ) -> AdapterState {
        AdapterState::Codex(CodexAdapterState {
            parser: CodexEventParser::with_permission_mode(permission_mode.as_str()),
            pending_permissions: HashMap::new(),
            init: initializing.then(|| CodexInitState::new(started_at)),
            next_request_id: FIRST_TURN_REQUEST_ID,
            active_turn_id: None,
        })
    }

    fn parse_line(&self, state: &mut AdapterState, line: &str) -> ParsedTurn {
        let AdapterState::Codex(state) = state else {
            return ParsedTurn {
                events: vec![AgentEvent::ProtocolError {
                    message: "Codex adapter received incompatible parser state".to_string(),
                }],
                ..ParsedTurn::default()
            };
        };
        state.parser.parse_line(line)
    }

    fn encode_user_message(
        &self,
        state: &mut AdapterState,
        text: &str,
        turn: TurnKind<'_>,
    ) -> Result<Option<String>, String> {
        let AdapterState::Codex(state) = state else {
            return Err("Codex adapter received incompatible session state".to_string());
        };
        match turn {
            TurnKind::Start { cli_session_id } => {
                if let Some(init) = &mut state.init {
                    if init.queue_message(text)? == CodexMessageDisposition::Queued {
                        return Ok(None);
                    }
                }
                let cli_session_id = cli_session_id.ok_or_else(|| {
                    "Codex app-server has not initialized its thread yet".to_string()
                })?;
                user_message_line(text, cli_session_id, state.next_request_id()).map(Some)
            }
            TurnKind::Steer {
                cli_session_id,
                turn_id,
            } => {
                let turn_id = turn_id
                    .ok_or_else(|| "Codex has not started a steerable turn yet".to_string())?;
                steer_message_line(text, cli_session_id, turn_id, state.next_request_id()).map(Some)
            }
            TurnKind::Queued { cli_session_id } => {
                user_message_line(text, cli_session_id, state.next_request_id()).map(Some)
            }
        }
    }

    fn requires_session_discovery(&self) -> bool {
        true
    }

    fn assigned_interactive_session_id(&self, _resume_cli_session_id: Option<&str>) -> bool {
        false
    }

    fn uses_rtk_instruction(&self) -> bool {
        true
    }

    fn supports_nested_codex_home(&self, _proxy_shutdown_on_exit: bool) -> bool {
        false
    }
}

fn login_launch_spec() -> Result<LaunchSpec, String> {
    let program = find_binary().ok_or_else(|| {
        "Codex CLI was not found in PATH or a supported install location".to_string()
    })?;
    Ok(login_launch_spec_with_program(program))
}

fn login_launch_spec_with_program(program: PathBuf) -> LaunchSpec {
    let launch = cli_binary::launch_for(&program);
    LaunchSpec {
        program: launch.program.clone(),
        args: launch.with_args([OsString::from("login")]),
        env: Vec::new(),
        env_remove: Vec::new(),
    }
}

#[allow(clippy::too_many_arguments)]
fn spawn(
    cwd: &Path,
    lane: &SessionLane,
    permission_mode: &str,
    headroom_port: Option<u16>,
    headroom_project: &str,
    savings_database_path: &Path,
    ponytail_mode: PonytailMode,
    routing: TaskRoutingContext<'_>,
    extension_manager: &crate::extensions::ExtensionManager,
) -> Result<Child, String> {
    let binary = find_binary().ok_or_else(|| {
        "Codex CLI was not found in PATH or a supported install location".to_string()
    })?;
    let mut command = gui_command_with_program_and_crux(
        binary,
        cwd,
        lane,
        permission_mode,
        headroom_port,
        headroom_project,
        savings_database_path,
        ponytail_mode,
        routing,
        crate::extensions::crux_enabled_for_project(Some(extension_manager), Some(cwd)),
    )?;
    command
        .spawn()
        .map_err(|error| format!("failed to start Codex app-server: {error}"))
}

#[allow(clippy::too_many_arguments)]
fn gui_command_with_program_and_crux(
    binary: PathBuf,
    cwd: &Path,
    lane: &SessionLane,
    permission_mode: &str,
    headroom_port: Option<u16>,
    headroom_project: &str,
    savings_database_path: &Path,
    ponytail_mode: PonytailMode,
    routing: TaskRoutingContext<'_>,
    crux_enabled: bool,
) -> Result<Command, String> {
    let args = invocation_args(
        lane,
        permission_mode,
        headroom_port,
        headroom_project,
        savings_database_path,
        ponytail_mode,
        routing,
        crux_enabled,
    )?;
    let mut command = cli_binary::command_for(&binary);
    command
        .args(args)
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    apply_custom_environment(&mut command, lane)?;
    command.env("HEADROOM_PROJECT", headroom_project);
    apply_savings_stack_environment(&mut command, headroom_project);
    host_env::apply_to_command(&mut command);
    Ok(command)
}

#[cfg(test)]
#[allow(clippy::too_many_arguments)]
fn gui_command_with_program(
    binary: PathBuf,
    cwd: &Path,
    lane: &SessionLane,
    permission_mode: &str,
    headroom_port: Option<u16>,
    headroom_project: &str,
    savings_database_path: &Path,
    ponytail_mode: PonytailMode,
) -> Result<Command, String> {
    gui_command_with_program_and_crux(
        binary,
        cwd,
        lane,
        permission_mode,
        headroom_port,
        headroom_project,
        savings_database_path,
        ponytail_mode,
        TaskRoutingContext::default(),
        false,
    )
}

#[allow(clippy::too_many_arguments)]
fn interactive_launch_spec(
    cwd: &Path,
    lane: &SessionLane,
    permission_mode: &str,
    resume_cli_session_id: Option<&str>,
    headroom_port: Option<u16>,
    headroom_project: &str,
    savings_database_path: &Path,
    ponytail_mode: PonytailMode,
    routing: TaskRoutingContext<'_>,
    extension_manager: &crate::extensions::ExtensionManager,
    terminal_notifications: bool,
) -> Result<LaunchSpec, String> {
    let program = find_binary().ok_or_else(|| {
        "Codex CLI was not found in PATH or supported install location".to_string()
    })?;
    interactive_launch_spec_with_program_and_crux(
        program,
        cwd,
        lane,
        permission_mode,
        resume_cli_session_id,
        headroom_port,
        headroom_project,
        savings_database_path,
        ponytail_mode,
        routing,
        crate::extensions::crux_enabled_for_project(Some(extension_manager), Some(cwd)),
        terminal_notifications,
    )
}

/// The interactive TUI writes an OSC 9 notification when it asks for an
/// approval, asks a question, shows a plan prompt, or ends a turn. Halv
/// reads these from the PTY output of its own panes. The app-server launch
/// and an external terminal must not get them.
const TERMINAL_NOTIFICATION_FLAGS: [&str; 6] = [
    "-c",
    r#"tui.notifications=["agent-turn-complete","approval-requested","plan-mode-prompt","async-question"]"#,
    "-c",
    r#"tui.notification_method="osc9""#,
    "-c",
    r#"tui.notification_condition="always""#,
];

#[allow(clippy::too_many_arguments)]
fn interactive_launch_spec_with_program_and_crux(
    program: PathBuf,
    cwd: &Path,
    lane: &SessionLane,
    permission_mode: &str,
    resume_cli_session_id: Option<&str>,
    headroom_port: Option<u16>,
    headroom_project: &str,
    savings_database_path: &Path,
    ponytail_mode: PonytailMode,
    routing: TaskRoutingContext<'_>,
    crux_enabled: bool,
    terminal_notifications: bool,
) -> Result<LaunchSpec, String> {
    let mut args = invocation_args(
        lane,
        permission_mode,
        headroom_port,
        headroom_project,
        savings_database_path,
        ponytail_mode,
        routing,
        crux_enabled,
    )?;
    if args.len() < 2 {
        return Err("Codex invocation did not contain the app-server command".to_string());
    }
    args.truncate(args.len() - 2);
    if terminal_notifications {
        args.extend(TERMINAL_NOTIFICATION_FLAGS.map(OsString::from));
    }
    if let Some(cli_session_id) = resume_cli_session_id {
        args.push(OsString::from("resume"));
        args.push(OsString::from(cli_session_id));
    }

    let launch = cli_binary::launch_for(&program);
    let mut environment_command = Command::new(&launch.program);
    environment_command.current_dir(cwd);
    apply_custom_environment(&mut environment_command, lane)?;
    environment_command.env("HEADROOM_PROJECT", headroom_project);
    host_env::apply_to_command(&mut environment_command);
    let (env, env_remove) = command_environment(&environment_command);
    Ok(LaunchSpec {
        program: launch.program.clone(),
        args: launch.with_args(args),
        env,
        env_remove,
    })
}

#[cfg(test)]
#[allow(clippy::too_many_arguments)]
pub(super) fn interactive_launch_spec_with_program(
    program: PathBuf,
    cwd: &Path,
    lane: &SessionLane,
    permission_mode: &str,
    resume_cli_session_id: Option<&str>,
    headroom_port: Option<u16>,
    headroom_project: &str,
    savings_database_path: &Path,
    ponytail_mode: PonytailMode,
) -> Result<LaunchSpec, String> {
    interactive_launch_spec_with_program_and_crux(
        program,
        cwd,
        lane,
        permission_mode,
        resume_cli_session_id,
        headroom_port,
        headroom_project,
        savings_database_path,
        ponytail_mode,
        TaskRoutingContext::default(),
        false,
        true,
    )
}

fn write_startup(
    writer: &mut ChildStdin,
    cwd: &Path,
    permission_mode: &str,
    resume_cli_session_id: Option<&str>,
    model: Option<&str>,
) -> Result<(), String> {
    for line in startup_lines(cwd, permission_mode, resume_cli_session_id, model) {
        writeln!(writer, "{line}")
            .map_err(|error| format!("failed to initialize Codex app-server: {error}"))?;
    }
    writer
        .flush()
        .map_err(|error| format!("failed to flush Codex app-server initialization: {error}"))
}

fn user_message_line(
    text: &str,
    cli_session_id: &str,
    request_id: usize,
) -> Result<String, String> {
    serde_json::to_string(&json!({
        "id": request_id,
        "method": "turn/start",
        "params": {
            "threadId": cli_session_id,
            "input": [{ "type": "text", "text": text }]
        }
    }))
    .map_err(|error| format!("failed to encode Codex turn request: {error}"))
}

fn steer_message_line(
    text: &str,
    cli_session_id: &str,
    turn_id: &str,
    request_id: usize,
) -> Result<String, String> {
    serde_json::to_string(&json!({
        "id": request_id,
        "method": "turn/steer",
        "params": {
            "threadId": cli_session_id,
            "input": [{ "type": "text", "text": text }],
            "expectedTurnId": turn_id
        }
    }))
    .map_err(|error| format!("failed to encode Codex steer request: {error}"))
}

pub(super) fn headroom_openai_base_url(port: u16, project: &str) -> String {
    format!("http://127.0.0.1:{port}/p/{project}/v1")
}

fn combined_developer_instructions(ponytail_mode: PonytailMode) -> String {
    let mut instructions = format!(
        "{}\n\n{}",
        RTK_DEVELOPER_INSTRUCTIONS,
        EXECUTION_DEVELOPER_INSTRUCTIONS.trim()
    );
    if let Some(text) = ponytail::instructions(ponytail_mode) {
        instructions.push_str("\n\n");
        instructions.push_str(&text);
    }
    instructions
}

/// A TOML inline table of strings for a Codex `-c` value. JSON strings are
/// also valid TOML strings.
fn toml_string_table(
    values: &std::collections::BTreeMap<String, String>,
) -> Result<String, String> {
    let entries = values
        .iter()
        .map(|(key, value)| {
            Ok(format!(
                "{} = {}",
                serde_json::to_string(key)?,
                serde_json::to_string(value)?
            ))
        })
        .collect::<Result<Vec<_>, serde_json::Error>>()
        .map_err(|error| format!("failed to encode an MCP environment: {error}"))?;
    Ok(format!("{{{}}}", entries.join(", ")))
}

#[allow(clippy::too_many_arguments)]
fn invocation_args(
    lane: &SessionLane,
    permission_mode: &str,
    headroom_port: Option<u16>,
    headroom_project: &str,
    savings_database_path: &Path,
    ponytail_mode: PonytailMode,
    routing: TaskRoutingContext<'_>,
    crux_enabled: bool,
) -> Result<Vec<OsString>, String> {
    let permission_flags = permission_flags(AgentKind::Codex, permission_mode)?;
    let savings = crate::savings_mcp::launch_config(
        savings_database_path,
        headroom_project,
        routing.coordinator_model,
        routing.conversation_id,
        routing.worker_lane,
        routing.tier_agents.clone(),
        // Without routing, the server lists no routing tool (ADR 0023).
        routing.enabled,
        false,
    )?;
    let savings_command = serde_json::to_string(&savings.command)
        .map_err(|error| format!("failed to encode savings MCP command: {error}"))?;
    let savings_args = serde_json::to_string(&savings.args)
        .map_err(|error| format!("failed to encode savings MCP arguments: {error}"))?;
    let mut developer_instructions = combined_developer_instructions(ponytail_mode);
    if let Some(text) = routing.instructions() {
        developer_instructions.push_str("\n\n");
        developer_instructions.push_str(&text);
    }
    let mut args = Vec::new();
    if crux_enabled {
        // Codex removes session variables from the MCP server environment.
        // The MCP configuration must include the Crux ledger path.
        // Codex parses each `-c` value as TOML.
        let crux_env = format!(
            "{{PATH = {}, CRUX_SAVINGS_LEDGER = {}}}",
            serde_json::to_string(&crate::extensions::crux_path_environment())
                .map_err(|error| format!("failed to encode crux PATH: {error}"))?,
            serde_json::to_string(
                &crate::exec::crux_session_ledger_path(headroom_project)
                    .to_string_lossy()
                    .into_owned(),
            )
            .map_err(|error| format!("failed to encode crux ledger path: {error}"))?,
        );
        args.extend([
            OsString::from("-c"),
            OsString::from(r#"mcp_servers.crux.command="crux""#),
            OsString::from("-c"),
            OsString::from(r#"mcp_servers.crux.args=["--profile","slim"]"#),
            OsString::from("-c"),
            OsString::from(format!("mcp_servers.crux.env={crux_env}")),
        ]);
    }
    if routing.enabled {
        args.extend([
            OsString::from("-c"),
            OsString::from("features.multi_agent=true"),
            OsString::from("-c"),
            OsString::from("features.multi_agent_v2=true"),
            OsString::from("-c"),
            OsString::from("agents.max_depth=2"),
            OsString::from("-c"),
            OsString::from("agents.max_threads=8"),
        ]);
    }
    args.extend([
        OsString::from("-c"),
        OsString::from(format!(
            "mcp_servers.halv-savings.command={savings_command}"
        )),
        OsString::from("-c"),
        OsString::from(format!("mcp_servers.halv-savings.args={savings_args}")),
        OsString::from("-c"),
        OsString::from(format!(
            "developer_instructions={}",
            serde_json::to_string(&developer_instructions)
                .map_err(|error| format!("failed to encode RTK instructions: {error}"))?
        )),
    ]);
    // Codex removes session variables from the MCP server environment. On
    // Linux the savings server needs the D-Bus session bus to read the Halv
    // session token from the keychain.
    if !savings.env.is_empty() {
        args.extend([
            OsString::from("-c"),
            OsString::from(format!(
                "mcp_servers.halv-savings.env={}",
                toml_string_table(&savings.env)?
            )),
        ]);
    }
    match lane {
        SessionLane::CodexChatGpt { proxied } => {
            // No port means the Halv Engine is off. The lane then goes direct.
            if let Some(port) = headroom_port.filter(|_| *proxied) {
                // The user's config.toml can select a custom model provider
                // (for example an RTK "headroom" provider) whose base_url has
                // no session prefix. openai_base_url only overrides the
                // built-in provider, so force that provider; otherwise the
                // session traffic stays unattributed and the meter reads 0.
                args.push(OsString::from("-c"));
                args.push(OsString::from(r#"model_provider="openai""#));
                // Only model requests use Headroom. Codex owns account discovery.
                // Proxying chatgpt_base_url breaks workspace discovery in Codex 0.159.
                args.push(OsString::from("-c"));
                args.push(OsString::from(format!(
                    "openai_base_url=\"{}\"",
                    headroom_openai_base_url(port, headroom_project)
                )));
                args.push(OsString::from("-c"));
                args.push(OsString::from("features.apps=false"));
            }
        }
        SessionLane::Custom { env } => {
            let has_openai_api_key = env
                .iter()
                .rev()
                .find(|(name, _)| name == "OPENAI_API_KEY")
                .is_some_and(|(_, value)| !value.trim().is_empty());
            // No port means the Halv Engine is off. The lane then goes direct.
            if let Some(port) = headroom_port.filter(|_| has_openai_api_key) {
                // User config can select another provider.
                // Force OpenAI so this API-key lane reaches the session proxy.
                args.push(OsString::from("-c"));
                args.push(OsString::from(r#"model_provider="openai""#));
                args.push(OsString::from("-c"));
                args.push(OsString::from(format!(
                    "openai_base_url=\"{}\"",
                    headroom_openai_base_url(port, headroom_project)
                )));
            }
        }
        SessionLane::ClaudeSubscription
        | SessionLane::ClaudeApiKey { .. }
        | SessionLane::GrokApiKey { .. }
        | SessionLane::ZaiGlm { .. }
        | SessionLane::KimiApiKey { .. }
        | SessionLane::KimiSubscription => {
            return Err("this lane cannot be used with the Codex adapter".to_string());
        }
    }
    args.extend(permission_flags.cli_args());
    args.extend([OsString::from("app-server"), OsString::from("--stdio")]);
    Ok(args)
}

fn apply_custom_environment(command: &mut Command, lane: &SessionLane) -> Result<(), String> {
    if let SessionLane::CodexChatGpt { .. } = lane {
        // A new ChatGPT session uses the active wallet accounts.
        crate::wallet::session_environment(crate::wallet::WalletProvider::Codex, true)?
            .apply(command);
        return Ok(());
    }
    let SessionLane::Custom { env } = lane else {
        return Ok(());
    };
    for (name, value) in env {
        if name.is_empty() || name.contains('=') || name.contains('\0') {
            return Err(format!("invalid custom environment variable name `{name}`"));
        }
        if value.contains('\0') {
            return Err(format!("custom environment variable `{name}` contains NUL"));
        }
        command.env(name, value);
    }
    Ok(())
}

fn startup_lines(
    cwd: &Path,
    permission_mode: &str,
    resume_cli_session_id: Option<&str>,
    model: Option<&str>,
) -> Vec<String> {
    let settings = approval_settings(permission_mode);
    let initialize = json!({
        "id": INITIALIZE_REQUEST_ID,
        "method": "initialize",
        "params": {
            "clientInfo": {
                "name": "halv",
                "title": "Halv",
                "version": env!("CARGO_PKG_VERSION")
            },
            "capabilities": { "experimentalApi": true }
        }
    });
    let initialized = json!({ "method": "initialized" });
    let (method, mut params) = match resume_cli_session_id {
        Some(thread_id) => (
            "thread/resume",
            json!({
                "threadId": thread_id,
                "cwd": cwd,
                "approvalPolicy": settings.approval_policy,
                "sandbox": settings.sandbox
            }),
        ),
        None => (
            "thread/start",
            json!({
                "cwd": cwd,
                "approvalPolicy": settings.approval_policy,
                "sandbox": settings.sandbox
            }),
        ),
    };
    if let Some(model) = model.map(str::trim).filter(|model| !model.is_empty()) {
        params["model"] = Value::String(model.to_string());
    }
    let thread = json!({
        "id": THREAD_REQUEST_ID,
        "method": method,
        "params": params
    });
    vec![
        initialize.to_string(),
        initialized.to_string(),
        thread.to_string(),
    ]
}

fn approval_settings(permission_mode: &str) -> CodexApprovalSettings {
    permission_flags(AgentKind::Codex, permission_mode)
        .expect("validated Codex permission mode")
        .codex_settings
        .expect("Codex permission mapping must include app-server settings")
}

fn parse_usage(value: &Value) -> TurnUsage {
    TurnUsage {
        input_tokens: token_field(value, "inputTokens", "input_tokens"),
        output_tokens: token_field(value, "outputTokens", "output_tokens"),
        cache_read_tokens: token_field(value, "cachedInputTokens", "cached_input_tokens"),
        cache_creation_tokens: token_field(
            value,
            "cacheWriteInputTokens",
            "cache_write_input_tokens",
        ),
        total_cost_usd: value
            .get("totalCostUsd")
            .or_else(|| value.get("total_cost_usd"))
            .and_then(Value::as_f64),
    }
}

fn token_field(value: &Value, camel_case: &str, snake_case: &str) -> u64 {
    value
        .get(camel_case)
        .or_else(|| value.get(snake_case))
        .and_then(Value::as_u64)
        .unwrap_or_default()
}

fn tool_item(item: &Value) -> Option<(String, String)> {
    let id = item.get("id")?.as_str()?.to_string();
    let item_type = item.get("type")?.as_str()?;
    let name = match item_type {
        "commandExecution" => "commandExecution".to_string(),
        "fileChange" => "applyPatch".to_string(),
        "mcpToolCall" => {
            let server = item.get("server").and_then(Value::as_str).unwrap_or("mcp");
            let tool = item.get("tool").and_then(Value::as_str).unwrap_or("tool");
            format!("{server}/{tool}")
        }
        "dynamicToolCall" | "collabAgentToolCall" => item
            .get("tool")
            .and_then(Value::as_str)
            .unwrap_or(item_type)
            .to_string(),
        "webSearch" => "webSearch".to_string(),
        _ => return None,
    };
    Some((id, name))
}

fn approval_description(method: &str, params: &Value) -> String {
    params
        .get("reason")
        .and_then(Value::as_str)
        .or_else(|| params.get("command").and_then(Value::as_str))
        .unwrap_or(method)
        .to_string()
}

fn request_id(value: Option<&Value>) -> Option<u64> {
    value.and_then(|value| {
        value
            .as_u64()
            .or_else(|| value.as_str().and_then(|value| value.parse().ok()))
    })
}

fn request_id_text(value: &Value) -> String {
    value
        .as_str()
        .map(str::to_string)
        .unwrap_or_else(|| value.to_string())
}

const WHAM_USAGE_URL: &str = "https://chatgpt.com/backend-api/wham/usage";
const USAGE_CACHE_TTL: Duration = Duration::from_secs(60);
/// The cache key names the account, so a wallet switch never shows the
/// usage of the previous account.
type UsageCacheEntry = (String, std::time::Instant, CodexUsageSummary);
static USAGE_CACHE: std::sync::Mutex<Option<UsageCacheEntry>> = std::sync::Mutex::new(None);

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexUsageSummary {
    pub plan_type: Option<String>,
    pub windows: Vec<CodexUsageWindow>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexUsageWindow {
    pub label: String,
    pub used_percent: Option<f64>,
    pub resets_at: Option<String>,
    pub window_seconds: Option<i64>,
}

#[derive(serde::Deserialize)]
struct WhamUsageResponse {
    plan_type: Option<String>,
    rate_limit: Option<WhamRateLimit>,
}

#[derive(serde::Deserialize)]
struct WhamRateLimit {
    primary_window: Option<WhamWindow>,
    secondary_window: Option<WhamWindow>,
}

#[derive(serde::Deserialize)]
struct WhamWindow {
    used_percent: Option<f64>,
    limit_window_seconds: Option<i64>,
    reset_at: Option<i64>,
}

/// The Codex home of the active account: a wallet account home, else
/// `CODEX_HOME`, else `~/.codex`.
pub(super) fn codex_home() -> Option<PathBuf> {
    crate::wallet::codex_home()
}

fn backend_auth() -> Option<(String, Option<String>)> {
    let auth = fs::read_to_string(codex_home()?.join("auth.json")).ok()?;
    let auth: Value = serde_json::from_str(&auth).ok()?;
    let tokens = auth.get("tokens")?;
    let access_token = tokens.get("access_token")?.as_str()?.to_string();
    let account_id = tokens
        .get("account_id")
        .and_then(Value::as_str)
        .map(ToString::to_string);
    Some((access_token, account_id))
}

pub(super) fn usage_window_label(limit_window_seconds: Option<i64>) -> String {
    match limit_window_seconds {
        Some(seconds) if seconds >= 518_400 => "7d".to_string(),
        Some(seconds) if seconds <= 21_600 => "5h".to_string(),
        Some(seconds) => format!("{}h", (seconds as f64 / 3600.0).round() as i64),
        None => String::new(),
    }
}

fn usage_window_from_wham(window: &WhamWindow) -> CodexUsageWindow {
    CodexUsageWindow {
        label: usage_window_label(window.limit_window_seconds),
        used_percent: window
            .used_percent
            .filter(|percent| percent.is_finite())
            .map(|percent| percent.clamp(0.0, 100.0)),
        resets_at: window.reset_at.and_then(|epoch| {
            chrono::DateTime::from_timestamp(epoch, 0).map(|reset| reset.to_rfc3339())
        }),
        window_seconds: window.limit_window_seconds,
    }
}

fn usage_summary_from_wham(payload: WhamUsageResponse) -> CodexUsageSummary {
    let mut windows = payload
        .rate_limit
        .into_iter()
        .flat_map(|rate_limit| [rate_limit.primary_window, rate_limit.secondary_window])
        .flatten()
        .map(|window| usage_window_from_wham(&window))
        .collect::<Vec<_>>();
    windows.sort_by_key(|window| {
        (
            window.window_seconds.is_none(),
            window.window_seconds.unwrap_or_default(),
        )
    });
    CodexUsageSummary {
        plan_type: payload.plan_type,
        windows,
    }
}

// The Codex plan meters come from the same ChatGPT backend endpoint the
// Codex CLI reads. A short cache keeps the sidebar refresh from spamming it.
pub(super) async fn fetch_usage_windows() -> CodexUsageSummary {
    let account_key = crate::wallet::usage_cache_key(crate::wallet::WalletProvider::Codex);
    if let Some((cached_key, cached_at, summary)) = USAGE_CACHE
        .lock()
        .expect("codex usage cache lock poisoned")
        .clone()
    {
        if cached_key == account_key && cached_at.elapsed() < USAGE_CACHE_TTL {
            return summary;
        }
    }
    let summary = fetch_usage_windows_uncached().await.unwrap_or_default();
    if !summary.windows.is_empty() {
        *USAGE_CACHE.lock().expect("codex usage cache lock poisoned") =
            Some((account_key, std::time::Instant::now(), summary.clone()));
    }
    summary
}

async fn fetch_usage_windows_uncached() -> Option<CodexUsageSummary> {
    let (access_token, account_id) = backend_auth()?;
    let mut request = reqwest::Client::new()
        .get(WHAM_USAGE_URL)
        .bearer_auth(access_token)
        .header("User-Agent", "codex-cli")
        .header("OpenAI-Beta", "codex-1")
        .header("originator", "Codex Desktop")
        .timeout(Duration::from_secs(10));
    if let Some(account_id) = account_id {
        request = request.header("ChatGPT-Account-Id", account_id);
    }
    let response = request.send().await.ok()?;
    if !response.status().is_success() {
        return None;
    }
    let payload = response.json::<WhamUsageResponse>().await.ok()?;
    Some(usage_summary_from_wham(payload))
}

const ROLLOUT_PREFIX: &str = "rollout-";
const ROLLOUT_SUFFIX: &str = ".jsonl";
const ROLLOUT_UUID_LENGTH: usize = 36;
const ROLLOUT_CREATION_SLACK: Duration = Duration::from_secs(5);
const ROLLOUT_MAX_DIRECTORY_DEPTH: usize = 4;

// The Codex TUI never reports the session id it creates, so Halv reads the
// rollout file the CLI writes under `$CODEX_HOME/sessions` instead. The file
// name carries the CREATION timestamp and the session UUID; the first line
// records the session cwd. The creation timestamp is the filter — a sibling
// session that is still writing its rollout has a fresh mtime, and an mtime
// filter made a duplicate split steal the sibling's session id. Oldest-first
// order lets concurrent spawns in one cwd claim distinct files.
pub(super) fn discover_session_ids(cwd: &Path, spawned_at: SystemTime) -> Vec<String> {
    let Some(root) = sessions_root() else {
        return Vec::new();
    };
    let spawned_at_local = chrono::DateTime::<chrono::Local>::from(spawned_at).naive_local()
        - chrono::Duration::seconds(ROLLOUT_CREATION_SLACK.as_secs() as i64);
    let mut candidates = Vec::new();
    collect_rollout_candidates(&root, cwd, spawned_at_local, 0, &mut candidates);
    candidates.sort_by_key(|candidate| candidate.0);
    candidates
        .into_iter()
        .map(|(_, session_id)| session_id)
        .collect()
}

pub(super) fn sessions_root() -> Option<PathBuf> {
    if let Some(codex_home) = env::var_os("CODEX_HOME") {
        return Some(PathBuf::from(codex_home).join("sessions"));
    }
    home_directory().map(|home| home.join(".codex/sessions"))
}

fn collect_rollout_candidates(
    directory: &Path,
    cwd: &Path,
    spawned_at_local: chrono::NaiveDateTime,
    depth: usize,
    candidates: &mut Vec<(chrono::NaiveDateTime, String)>,
) {
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        if metadata.is_dir() {
            if depth < ROLLOUT_MAX_DIRECTORY_DEPTH {
                collect_rollout_candidates(&path, cwd, spawned_at_local, depth + 1, candidates);
            }
            continue;
        }
        let Some((created_at, session_id)) = rollout_created_at_and_session_id(&path) else {
            continue;
        };
        if created_at < spawned_at_local {
            continue;
        }
        if !rollout_matches_cwd(&path, cwd) {
            continue;
        }
        candidates.push((created_at, session_id));
    }
}

pub(super) fn rollout_created_at_and_session_id(
    path: &Path,
) -> Option<(chrono::NaiveDateTime, String)> {
    let name = path.file_name()?.to_str()?;
    let stem = name
        .strip_prefix(ROLLOUT_PREFIX)?
        .strip_suffix(ROLLOUT_SUFFIX)?;
    if stem.len() < ROLLOUT_UUID_LENGTH + 2 {
        return None;
    }
    // A multibyte name can put the split inside one character.
    // `get` rejects that name instead of a panic.
    let split_index = stem.len() - ROLLOUT_UUID_LENGTH - 1;
    let timestamp = stem.get(..split_index)?;
    let dashed_session_id = stem.get(split_index..)?;
    let session_id = dashed_session_id.strip_prefix('-')?;
    if !is_uuid(session_id) {
        return None;
    }
    // The CLI names rollouts with its local creation time, e.g.
    // rollout-2026-08-17T16-26-01-<uuid>.jsonl.
    let created_at = chrono::NaiveDateTime::parse_from_str(timestamp, "%Y-%m-%dT%H-%M-%S").ok()?;
    Some((created_at, session_id.to_string()))
}

fn is_uuid(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() == ROLLOUT_UUID_LENGTH
        && bytes.iter().enumerate().all(|(index, byte)| match index {
            8 | 13 | 18 | 23 => *byte == b'-',
            _ => byte.is_ascii_hexdigit(),
        })
}

// An unreadable or unrecognized header keeps the file as a candidate so
// discovery survives rollout format changes.
fn rollout_matches_cwd(path: &Path, cwd: &Path) -> bool {
    let Ok(file) = fs::File::open(path) else {
        return true;
    };
    let mut first_line = String::new();
    if BufReader::new(file).read_line(&mut first_line).is_err() {
        return true;
    }
    let Ok(value) = serde_json::from_str::<Value>(&first_line) else {
        return true;
    };
    let Some(recorded) = value
        .get("payload")
        .and_then(|payload| payload.get("cwd"))
        .and_then(Value::as_str)
    else {
        return true;
    };
    Path::new(recorded) == cwd
}

pub(super) fn find_binary() -> Option<PathBuf> {
    if let Some(binary) = install::managed_binary(ManagedAgent::Codex) {
        return Some(binary);
    }
    let mut seen = HashSet::new();
    binary_candidates(
        env::var_os("PATH"),
        home_directory().as_deref(),
        HostOs::current(),
    )
    .into_iter()
    .find(|candidate| seen.insert(candidate.clone()) && cli_binary::is_executable(candidate))
}

fn binary_candidates(path: Option<OsString>, home: Option<&Path>, os: HostOs) -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(path) = path {
        for directory in env::split_paths(&path) {
            cli_binary::push_candidates(&mut candidates, &directory, CODEX_BINARY_NAME, os);
        }
    }
    // npm puts its global shims in %APPDATA%\npm. A PATH without that
    // directory still finds the install.
    if os == HostOs::Windows {
        if let Some(app_data) = env::var_os("APPDATA") {
            cli_binary::push_candidates(
                &mut candidates,
                &Path::new(&app_data).join("npm"),
                CODEX_BINARY_NAME,
                os,
            );
        }
    }
    if let Some(home) = home {
        candidates.push(home.join(".codex/bin/codex"));
        candidates.push(home.join(".npm-global/bin/codex"));
        candidates.push(home.join(".local/bin/codex"));
    }
    candidates.extend([
        PathBuf::from("/opt/homebrew/bin/codex"),
        PathBuf::from("/usr/local/bin/codex"),
        PathBuf::from("/usr/bin/codex"),
    ]);
    if let Some(home) = home {
        super::claude::add_versioned_binary_candidates(
            &mut candidates,
            &home.join(".nvm/versions/node"),
            Path::new("bin/codex"),
        );
        candidates.push(home.join(".volta/bin/codex"));
        super::claude::add_versioned_binary_candidates(
            &mut candidates,
            &home.join(".local/share/fnm/node-versions"),
            Path::new("installation/bin/codex"),
        );
        for relative in [
            ".local/share/mise/shims/codex",
            ".asdf/shims/codex",
            ".bun/bin/codex",
            "Library/pnpm/codex",
        ] {
            candidates.push(home.join(relative));
        }
    }
    candidates
}

fn home_directory() -> Option<PathBuf> {
    #[cfg(windows)]
    let value = env::var_os("USERPROFILE");
    #[cfg(not(windows))]
    let value = env::var_os("HOME");
    value.map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toml_string_table_gives_a_codex_mcp_environment() {
        let environment = std::collections::BTreeMap::from([
            (
                "DBUS_SESSION_BUS_ADDRESS".to_string(),
                "unix:path=/run/user/1000/bus,guid=\"abc\"".to_string(),
            ),
            ("XDG_RUNTIME_DIR".to_string(), "/run/user/1000".to_string()),
        ]);
        let value = toml_string_table(&environment).expect("environment should encode");
        let parsed = format!("env = {value}")
            .parse::<toml::Table>()
            .expect("Codex should parse the value as TOML");
        let parsed = parsed["env"]
            .as_table()
            .expect("the value should be an inline table");
        assert_eq!(parsed.len(), 2);
        assert_eq!(
            parsed["DBUS_SESSION_BUS_ADDRESS"].as_str(),
            Some("unix:path=/run/user/1000/bus,guid=\"abc\"")
        );
        assert_eq!(parsed["XDG_RUNTIME_DIR"].as_str(), Some("/run/user/1000"));
    }

    #[test]
    fn codex_discovery_recovers_from_an_obsolete_node_path() {
        let nonce = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let home = env::temp_dir().join(format!("halv-codex-path-{}-{nonce}", std::process::id()));
        let old = home.join(".nvm/versions/node/v20.0.0/bin");
        let recent = home.join(".nvm/versions/node/v22.10.0/bin/codex");
        let older = home.join(".nvm/versions/node/v22.9.0/bin/codex");
        for binary in [&recent, &older] {
            fs::create_dir_all(binary.parent().unwrap()).unwrap();
            fs::write(binary, "#!/bin/sh\n").unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(binary, fs::Permissions::from_mode(0o755)).unwrap();
            }
        }
        let candidates = binary_candidates(Some(old.into_os_string()), Some(&home), HostOs::Unix);
        let installations = candidates
            .into_iter()
            .filter(|path| {
                path.starts_with(&home) && cli_binary::is_executable_on(path, HostOs::Unix)
            })
            .collect::<Vec<_>>();
        assert_eq!(installations, vec![recent.clone(), older]);
        let explicit = binary_candidates(
            Some(recent.parent().unwrap().as_os_str().to_owned()),
            Some(&home),
            HostOs::Unix,
        );
        assert_eq!(explicit.first(), Some(&recent));
        fs::remove_dir_all(home).unwrap();
    }

    /// Mirrors `%APPDATA%\npm` after `npm install -g @openai/codex`: a
    /// POSIX shell script `codex` next to `codex.cmd`.
    fn npm_prefix() -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let prefix = env::temp_dir().join(format!("halv-codex-npm-{}-{nonce}", std::process::id()));
        fs::create_dir_all(&prefix).unwrap();
        for (name, contents) in [
            ("codex", "#!/bin/sh\nexec node codex.js \"$@\"\n"),
            ("codex.cmd", "@ECHO off\r\n"),
        ] {
            let path = prefix.join(name);
            fs::write(&path, contents).unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
            }
        }
        prefix
    }

    fn first_executable(path: &Path, os: HostOs) -> Option<PathBuf> {
        binary_candidates(Some(path.as_os_str().to_owned()), None, os)
            .into_iter()
            .find(|candidate| cli_binary::is_executable_on(candidate, os))
    }

    #[test]
    fn windows_discovery_picks_the_cmd_shim_over_the_npm_shell_script() {
        let prefix = npm_prefix();
        assert_eq!(
            first_executable(&prefix, HostOs::Windows),
            Some(prefix.join("codex.cmd"))
        );
        fs::remove_dir_all(prefix).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn unix_discovery_picks_the_bare_name() {
        let prefix = npm_prefix();
        assert_eq!(
            first_executable(&prefix, HostOs::Unix),
            Some(prefix.join("codex"))
        );
        fs::remove_dir_all(prefix).unwrap();
    }

    fn crux_census(languages: &[(&str, u64, u64)]) -> crate::extensions::CruxProjectCensus {
        crate::extensions::CruxProjectCensus {
            languages: languages
                .iter()
                .map(|(name, files, lines)| {
                    (
                        (*name).to_string(),
                        crate::extensions::CruxLanguageCensus {
                            files: *files,
                            lines: *lines,
                        },
                    )
                })
                .collect(),
        }
    }

    #[test]
    fn an_external_terminal_launch_gets_no_tui_notifications() {
        let spec = interactive_launch_spec_with_program_and_crux(
            PathBuf::from("/tmp/codex"),
            Path::new("/tmp"),
            &SessionLane::CodexChatGpt { proxied: false },
            "default",
            None,
            None,
            "session-123",
            Path::new("/tmp/halv.sqlite3"),
            PonytailMode::Off,
            TaskRoutingContext::default(),
            false,
            false,
        )
        .expect("terminal specification should build");
        assert!(!spec
            .args
            .iter()
            .any(|value| value.to_string_lossy().starts_with("tui.")));
    }

    #[test]
    fn proxied_terminal_forces_the_builtin_provider_through_the_session_prefix() {
        let spec = interactive_launch_spec_with_program(
            PathBuf::from("/tmp/codex"),
            Path::new("/tmp"),
            &SessionLane::CodexChatGpt { proxied: true },
            "skip_all",
            None,
            Some(9123),
            "session-123",
            Path::new("/tmp/halv.sqlite3"),
            PonytailMode::Off,
        )
        .expect("proxied terminal specification should build");
        let args = spec
            .args
            .iter()
            .map(|value| value.to_string_lossy().into_owned())
            .collect::<Vec<_>>();

        assert!(args
            .windows(2)
            .any(|pair| pair == ["-c", "model_provider=\"openai\""]));
        assert!(args.windows(2).any(|pair| {
            pair == [
                "-c",
                "openai_base_url=\"http://127.0.0.1:9123/p/session-123/v1\"",
            ]
        }));
        assert!(!args.contains(&"app-server".to_string()));
        assert!(args
            .windows(2)
            .any(|pair| pair == ["-c", r#"tui.notification_method="osc9""#]));
        assert!(args
            .windows(2)
            .any(|pair| pair == ["-c", r#"tui.notification_condition="always""#]));
        assert!(args
            .iter()
            .any(|value| value.starts_with("tui.notifications=[")
                && value.contains("\"approval-requested\"")
                && value.contains("\"async-question\"")));
        let instructions = args
            .iter()
            .find(|value| value.starts_with("developer_instructions="))
            .expect("terminal sessions should carry execution guidance");
        assert!(instructions.contains("After an empty poll"));
        assert!(instructions.contains("smallest relevant test target"));
    }

    #[test]
    fn execution_guidance_preserves_ponytail_without_duplicating_crux() {
        let instructions = combined_developer_instructions(PonytailMode::Full);
        assert_eq!(
            instructions
                .matches("# Efficient command execution")
                .count(),
            1
        );
        assert!(instructions.contains("PONYTAIL MODE ACTIVE — level: full"));
        assert!(instructions.contains("Do not let longer polls delay cancellation"));
        assert!(!EXECUTION_DEVELOPER_INSTRUCTIONS.contains("Crux"));
        assert!(!EXECUTION_DEVELOPER_INSTRUCTIONS.contains("scip_"));
    }

    #[test]
    fn every_codex_session_carries_the_rtk_developer_instructions() {
        let command = gui_command_with_program(
            PathBuf::from("/tmp/codex"),
            Path::new("/tmp"),
            &SessionLane::CodexChatGpt { proxied: false },
            "default",
            None,
            "session-123",
            Path::new("/tmp/halv.sqlite3"),
            PonytailMode::Off,
        )
        .expect("Codex GUI command should build");
        let args = command
            .get_args()
            .map(|value| value.to_string_lossy().into_owned())
            .collect::<Vec<_>>();

        assert!(
            !args.iter().any(|value| value.starts_with("tui.")),
            "the app-server launch must not turn on TUI notifications"
        );
        let instructions = args
            .iter()
            .find(|value| value.starts_with("developer_instructions="))
            .expect("developer instructions should ride along");
        assert!(instructions.contains("rtk git status"));
        assert!(instructions.contains("After an empty poll"));
        assert!(instructions.contains("smallest relevant test target"));
        assert!(instructions.contains("run the command directly"));
        assert!(!instructions.contains("PONYTAIL"));

        let command = gui_command_with_program(
            PathBuf::from("/tmp/codex"),
            Path::new("/tmp"),
            &SessionLane::CodexChatGpt { proxied: false },
            "default",
            None,
            "session-123",
            Path::new("/tmp/halv.sqlite3"),
            PonytailMode::Full,
        )
        .expect("Codex GUI command should build with Ponytail");
        let instructions = command
            .get_args()
            .map(|value| value.to_string_lossy().into_owned())
            .find(|value| value.starts_with("developer_instructions="))
            .expect("developer instructions should ride along");
        assert!(instructions.contains("rtk git status"));
        assert!(instructions.contains("PONYTAIL MODE ACTIVE — level: full"));
    }

    #[test]
    fn unproxied_lane_keeps_the_user_provider_selection() {
        let command = gui_command_with_program(
            PathBuf::from("/tmp/codex"),
            Path::new("/tmp"),
            &SessionLane::CodexChatGpt { proxied: false },
            "default",
            None,
            "session-123",
            Path::new("/tmp/halv.sqlite3"),
            PonytailMode::Off,
        )
        .expect("unproxied GUI command should build");
        let args = command
            .get_args()
            .map(|value| value.to_string_lossy().into_owned())
            .collect::<Vec<_>>();

        assert!(!args.iter().any(|value| value.contains("model_provider")));
        assert!(!args.iter().any(|value| value.contains("openai_base_url")));
    }

    #[test]
    fn custom_openai_api_key_lane_forces_the_session_proxy() {
        let spec = interactive_launch_spec_with_program(
            PathBuf::from("/tmp/codex"),
            Path::new("/tmp"),
            &SessionLane::Custom {
                env: vec![("OPENAI_API_KEY".to_string(), "openai-test-key".to_string())],
            },
            "default",
            None,
            Some(9123),
            "session-123",
            Path::new("/tmp/halv.sqlite3"),
            PonytailMode::Off,
        )
        .expect("Custom OpenAI launch specification should build");
        let args = spec
            .args
            .iter()
            .map(|value| value.to_string_lossy().into_owned())
            .collect::<Vec<_>>();

        assert!(args
            .windows(2)
            .any(|pair| pair == ["-c", "model_provider=\"openai\""]));
        assert!(args.windows(2).any(|pair| {
            pair == [
                "-c",
                "openai_base_url=\"http://127.0.0.1:9123/p/session-123/v1\"",
            ]
        }));
        assert!(spec
            .env
            .iter()
            .any(|(name, value)| { name == "OPENAI_API_KEY" && value == "openai-test-key" }));
    }

    #[test]
    fn grok_lane_is_rejected() {
        let error = invocation_args(
            &SessionLane::GrokApiKey {
                credential: super::super::SessionCredential::ApiKey {
                    api_key: "xai-test-key".to_string(),
                },
                model_overrides: None,
            },
            "default",
            Some(9123),
            "session-123",
            Path::new("/tmp/halv.sqlite3"),
            PonytailMode::Off,
            TaskRoutingContext::default(),
            false,
        )
        .expect_err("Grok lane must not run through Codex");

        assert_eq!(error, "this lane cannot be used with the Codex adapter");
    }

    #[test]
    fn gui_and_terminal_use_the_same_permission_arguments() {
        for mode in [
            "read_only",
            "ask",
            "on_request",
            "on_failure",
            "never",
            "skip_all",
        ] {
            let expected = permission_flags(AgentKind::Codex, mode)
                .expect("Codex permission mode should map")
                .cli_args();
            let command = gui_command_with_program(
                PathBuf::from("/tmp/codex"),
                Path::new("/tmp"),
                &SessionLane::CodexChatGpt { proxied: false },
                mode,
                None,
                "session-123",
                Path::new("/tmp/halv.sqlite3"),
                PonytailMode::Off,
            )
            .expect("Codex GUI command should build");
            let mut gui_args = command.get_args().map(OsString::from).collect::<Vec<_>>();
            assert_eq!(
                gui_args.split_off(gui_args.len() - 2),
                ["app-server", "--stdio"]
            );
            let terminal = interactive_launch_spec_with_program(
                PathBuf::from("/tmp/codex"),
                Path::new("/tmp"),
                &SessionLane::CodexChatGpt { proxied: false },
                mode,
                None,
                None,
                "session-123",
                Path::new("/tmp/halv.sqlite3"),
                PonytailMode::Off,
            )
            .expect("Codex terminal specification should build");

            assert!(gui_args.ends_with(&expected));
            // Only the terminal launch turns on TUI notifications.
            let mut terminal_expected = gui_args.clone();
            terminal_expected.extend(TERMINAL_NOTIFICATION_FLAGS.map(OsString::from));
            assert_eq!(terminal.args, terminal_expected);
            match mode {
                "on_request" => assert!(gui_args
                    .windows(2)
                    .any(|pair| { pair == ["-c", "approvals_reviewer=\"auto_review\""] })),
                "ask" => assert!(!gui_args
                    .iter()
                    .any(|argument| argument == "approvals_reviewer=\"auto_review\"")),
                _ => {}
            }
        }
    }

    #[test]
    fn app_server_startup_uses_the_shared_permission_mapping() {
        for mode in [
            "read_only",
            "ask",
            "on_request",
            "on_failure",
            "never",
            "skip_all",
        ] {
            let expected = permission_flags(AgentKind::Codex, mode)
                .expect("Codex permission mode should map")
                .codex_settings
                .expect("Codex mode should include app-server settings");
            let lines = startup_lines(Path::new("/tmp/project"), mode, None, None);
            let thread: Value = serde_json::from_str(&lines[2]).expect("thread start should parse");

            assert_eq!(thread["params"]["approvalPolicy"], expected.approval_policy);
            assert_eq!(thread["params"]["sandbox"], expected.sandbox);
        }
    }

    #[test]
    fn nested_provider_uses_the_managed_proxy_and_project_header() {
        let mut config = toml::Table::new();
        config.insert(
            "model_provider".to_string(),
            toml::Value::String("personal".to_string()),
        );

        configure_nested_provider(&mut config, 9123);

        assert_eq!(
            config.get("model_provider").and_then(toml::Value::as_str),
            Some(HALV_PROVIDER_NAME)
        );
        assert_eq!(
            config.get("openai_base_url").and_then(toml::Value::as_str),
            Some("http://127.0.0.1:9123/v1")
        );
        let provider = config
            .get("model_providers")
            .and_then(toml::Value::as_table)
            .and_then(|providers| providers.get(HALV_PROVIDER_NAME))
            .and_then(toml::Value::as_table)
            .expect("Halv provider should exist");
        assert_eq!(
            provider
                .get("env_http_headers")
                .and_then(toml::Value::as_table)
                .and_then(|headers| headers.get("X-Headroom-Project"))
                .and_then(toml::Value::as_str),
            Some("HEADROOM_PROJECT")
        );
        assert_eq!(
            provider
                .get("requires_openai_auth")
                .and_then(toml::Value::as_bool),
            Some(true)
        );
    }

    #[test]
    fn nested_home_is_private_and_in_the_session_directory() {
        let session_id = format!("nested-home-{}", crate::storage::new_uuid_v4());
        let source = env::temp_dir().join(format!(
            "halv-codex-source-{}",
            crate::storage::new_uuid_v4()
        ));
        fs::create_dir_all(&source).expect("source Codex home should exist");
        fs::write(source.join("config.toml"), "model = \"gpt-test\"\n")
            .expect("source config should write");
        fs::write(source.join("auth.json"), "{}").expect("source auth should write");

        let home = prepare_nested_home_from(Some(source.clone()), Some(9123), &session_id)
            .expect("nested Codex home should be prepared");

        assert_eq!(
            home,
            session_files::directory(&session_id)
                .expect("session directory should exist")
                .join(session_files::CODEX_HOME_DIRECTORY_NAME)
        );
        assert!(!home.starts_with(crate::exec::rtk_state_directory()));
        let config = fs::read_to_string(home.join("config.toml"))
            .expect("nested config should read")
            .parse::<toml::Table>()
            .expect("nested config should parse");
        assert_eq!(
            config.get("model").and_then(toml::Value::as_str),
            Some("gpt-test")
        );
        assert_eq!(
            config.get("model_provider").and_then(toml::Value::as_str),
            Some(HALV_PROVIDER_NAME)
        );
        assert!(home.join("auth.json").exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            let mode = fs::metadata(&home)
                .expect("nested home should have metadata")
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o700);
        }

        session_files::remove(&session_id);
        assert!(!home.exists());
        fs::remove_dir_all(source).expect("source Codex home should be removable");
    }

    #[test]
    fn reads_latest_usage_from_each_nested_codex_rollout() {
        let home = env::temp_dir().join(format!(
            "halv-codex-usage-{}",
            crate::storage::new_uuid_v4()
        ));
        let sessions = home.join("sessions/2026/08/20");
        fs::create_dir_all(&sessions).expect("nested Codex sessions should be created");
        fs::write(
            sessions.join("rollout-a.jsonl"),
            concat!(
                r#"{"type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":10,"cached_input_tokens":5,"cache_write_input_tokens":2,"output_tokens":3}}}}"#,
                "\n",
                r#"{"type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":30,"cached_input_tokens":20,"cache_write_input_tokens":4,"output_tokens":8}}}}"#,
                "\n"
            ),
        )
        .expect("first nested Codex rollout should be written");
        fs::write(
            sessions.join("rollout-b.jsonl"),
            concat!(
                r#"{"type":"session_meta","payload":{}}"#,
                "\n",
                r#"{"type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":7,"cached_input_tokens":2,"cache_write_input_tokens":1,"output_tokens":5}}}}"#,
                "\n"
            ),
        )
        .expect("second nested Codex rollout should be written");

        let usage = nested_usage(&home);
        assert_eq!(usage.input_tokens, 37);
        assert_eq!(usage.output_tokens, 13);
        assert_eq!(usage.cache_read_tokens, 22);
        assert_eq!(usage.cache_creation_tokens, 5);
        fs::remove_dir_all(home).expect("nested Codex usage fixture should be removed");
    }

    #[test]
    fn startup_requests_and_notifications_omit_jsonrpc_field() {
        let lines = startup_lines(Path::new("/tmp/project"), "default", None, Some("gpt-5.5"));
        let initialize: Value = serde_json::from_str(&lines[0]).expect("initialize should parse");
        let initialized: Value = serde_json::from_str(&lines[1]).expect("initialized should parse");
        let thread: Value = serde_json::from_str(&lines[2]).expect("thread start should parse");

        assert_eq!(initialize["method"], "initialize");
        assert_eq!(initialize["id"], INITIALIZE_REQUEST_ID);
        assert!(initialize.get("jsonrpc").is_none());
        assert_eq!(initialized["method"], "initialized");
        assert!(initialized.get("id").is_none());
        assert_eq!(thread["method"], "thread/start");
        assert_eq!(thread["params"]["sandbox"], "workspace-write");
        assert_eq!(thread["params"]["model"], "gpt-5.5");
    }

    #[test]
    fn response_initializes_the_codex_session() {
        let mut parser = CodexEventParser::default();
        let parsed = parser.parse_line(
            r#"{"id":2,"result":{"thread":{"id":"thread-123"},"model":"gpt-5.6-sol"}}"#,
        );

        assert_eq!(
            parsed.events,
            vec![AgentEvent::SessionInit {
                cli_session_id: "thread-123".to_string(),
                model: "gpt-5.6-sol".to_string(),
            }]
        );
        assert!(parsed.responses.is_empty());
    }

    #[test]
    fn maps_turn_started_and_agent_message_delta_notifications() {
        let mut parser = CodexEventParser::default();
        let started = parser.parse_line(
            r#"{"method":"turn/started","params":{"threadId":"thread-1","turn":{"id":"turn-1"}}}"#,
        );
        let delta = parser.parse_line(
            r#"{"method":"item/agentMessage/delta","params":{"threadId":"thread-1","turnId":"turn-1","itemId":"item-1","delta":"Hello"}}"#,
        );

        assert_eq!(
            started.events,
            vec![AgentEvent::TurnStarted {
                turn_id: Some("turn-1".to_string()),
            }]
        );
        assert_eq!(
            delta.events,
            vec![AgentEvent::TextDelta {
                text: "Hello".to_string(),
            }]
        );
    }

    #[test]
    fn maps_completed_turn_usage_from_token_notification() {
        let mut parser = CodexEventParser::default();
        let usage = parser.parse_line(
            r#"{"method":"thread/tokenUsage/updated","params":{"threadId":"thread-1","turnId":"turn-1","tokenUsage":{"last":{"inputTokens":11,"cachedInputTokens":33,"cacheWriteInputTokens":44,"outputTokens":22,"reasoningOutputTokens":5,"totalTokens":66},"total":{"inputTokens":11,"cachedInputTokens":33,"cacheWriteInputTokens":44,"outputTokens":22,"reasoningOutputTokens":5,"totalTokens":66}}}}"#,
        );
        let completed = parser.parse_line(
            r#"{"method":"turn/completed","params":{"threadId":"thread-1","turn":{"id":"turn-1","items":[],"status":"completed"}}}"#,
        );

        assert!(usage.events.is_empty());
        assert_eq!(
            completed.events,
            vec![AgentEvent::TurnCompleted {
                usage: TurnUsage {
                    input_tokens: 11,
                    output_tokens: 22,
                    cache_read_tokens: 33,
                    cache_creation_tokens: 44,
                    total_cost_usd: None,
                },
            }]
        );
    }

    #[test]
    fn maps_command_items_and_approval_requests() {
        let mut parser = CodexEventParser::with_permission_mode("never");
        let started = parser.parse_line(
            r#"{"method":"item/started","params":{"item":{"type":"commandExecution","id":"item-1","command":"rg TODO","cwd":"/tmp","status":"inProgress"},"threadId":"thread-1","turnId":"turn-1","startedAtMs":1}}"#,
        );
        let completed = parser.parse_line(
            r#"{"method":"item/completed","params":{"item":{"type":"commandExecution","id":"item-1","command":"rg TODO","cwd":"/tmp","status":"completed"},"threadId":"thread-1","turnId":"turn-1","completedAtMs":2}}"#,
        );
        let approval = parser.parse_line(
            r#"{"id":"approval-1","method":"item/commandExecution/requestApproval","params":{"threadId":"thread-1","turnId":"turn-1","itemId":"item-1","command":"git status","startedAtMs":1}}"#,
        );

        assert!(matches!(
            started.events.as_slice(),
            [AgentEvent::ToolCallStarted { name, .. }] if name == "commandExecution"
        ));
        assert_eq!(
            completed.events,
            vec![AgentEvent::ToolCallCompleted {
                name: "commandExecution".to_string(),
            }]
        );
        assert!(matches!(
            approval.events.as_slice(),
            [AgentEvent::PermissionRequest { request_id, .. }] if request_id == "approval-1"
        ));
        let response: Value =
            serde_json::from_str(&approval.responses[0]).expect("approval response should parse");
        assert_eq!(response["id"], "approval-1");
        assert_eq!(response["result"]["decision"], "accept");
        assert!(response.get("jsonrpc").is_none());
    }

    #[test]
    fn holds_approval_requests_for_interactive_permission_modes() {
        for mode in [
            "",
            "default",
            "ask",
            "on_request",
            "on_failure",
            "on-failure",
        ] {
            let mut parser = CodexEventParser::with_permission_mode(mode);
            let approval = parser.parse_line(
                r#"{"id":"approval-1","method":"item/commandExecution/requestApproval","params":{"command":"git status"}}"#,
            );

            assert!(approval.responses.is_empty(), "mode {mode} must wait");
            assert_eq!(approval.pending_permissions.len(), 1);
            assert_eq!(approval.pending_permissions[0].request_id, "approval-1");
        }
    }

    #[test]
    fn auto_accepts_approval_requests_for_noninteractive_permission_modes() {
        for mode in ["read_only", "never", "skip_all"] {
            let mut parser = CodexEventParser::with_permission_mode(mode);
            let approval = parser.parse_line(
                r#"{"id":"approval-1","method":"item/commandExecution/requestApproval","params":{"command":"git status"}}"#,
            );

            assert_eq!(approval.responses.len(), 1, "mode {mode} must respond");
            assert!(approval.pending_permissions.is_empty());
        }
    }

    #[test]
    fn builds_permission_responses_for_each_codex_method() {
        let cases = [
            (
                "item/commandExecution/requestApproval",
                json!({ "decision": "accept" }),
                json!({ "decision": "decline" }),
            ),
            (
                "item/fileChange/requestApproval",
                json!({ "decision": "accept" }),
                json!({ "decision": "decline" }),
            ),
            (
                "execCommandApproval",
                json!({ "decision": "approved" }),
                json!({
                    "decision": {
                        "denied": { "rejection": "The user denied the request." }
                    }
                }),
            ),
            (
                "applyPatchApproval",
                json!({ "decision": "approved" }),
                json!({
                    "decision": {
                        "denied": { "rejection": "The user denied the request." }
                    }
                }),
            ),
            (
                "item/permissions/requestApproval",
                json!({
                    "permissions": { "network": { "enabled": true } },
                    "scope": "turn"
                }),
                json!({ "permissions": {}, "scope": "turn" }),
            ),
        ];

        for (method, approved_result, denied_result) in cases {
            let request = CodexPermissionRequest {
                request_id: "approval-7".to_string(),
                rpc_id: json!(7),
                method: method.to_string(),
                params: json!({ "permissions": { "network": { "enabled": true } } }),
            };
            let approved: Value = serde_json::from_str(
                &permission_response(&request, true).expect("approval response should build"),
            )
            .expect("approval response should parse");
            let denied: Value = serde_json::from_str(
                &permission_response(&request, false).expect("denial response should build"),
            )
            .expect("denial response should parse");

            assert_eq!(approved["id"], 7);
            assert_eq!(approved["result"], approved_result);
            assert_eq!(denied["id"], 7);
            assert_eq!(denied["result"], denied_result);
        }
    }

    #[test]
    fn legacy_denial_responses_include_non_empty_rejection_message() {
        for method in ["execCommandApproval", "applyPatchApproval"] {
            let request = CodexPermissionRequest {
                request_id: "approval-7".to_string(),
                rpc_id: json!(7),
                method: method.to_string(),
                params: json!({}),
            };
            let denied: Value = serde_json::from_str(
                &permission_response(&request, false).expect("denial response should build"),
            )
            .expect("denial response should parse");
            let rejection = denied
                .pointer("/result/decision/denied/rejection")
                .and_then(Value::as_str)
                .expect("legacy denial must include a rejection string");

            assert!(
                !rejection.trim().is_empty(),
                "legacy denial rejection must not be empty for {method}"
            );
        }
    }

    #[test]
    fn proxied_lane_builds_private_headroom_overrides() {
        let command = gui_command_with_program(
            PathBuf::from("/tmp/codex"),
            Path::new("/tmp"),
            &SessionLane::CodexChatGpt { proxied: true },
            "default",
            Some(9123),
            "session-123",
            Path::new("/tmp/halv.sqlite3"),
            PonytailMode::Off,
        )
        .expect("proxied GUI command should build");
        let args = command
            .get_args()
            .map(|value| value.to_string_lossy().into_owned())
            .collect::<Vec<_>>();

        // The built-in provider must be forced before the base overrides:
        // a user config.toml that selects a custom provider (RTK headroom)
        // would otherwise route past the session prefix unattributed.
        assert!(args
            .windows(2)
            .any(|pair| pair == ["-c", "model_provider=\"openai\""]));
        assert!(!args.iter().any(|arg| arg.starts_with("chatgpt_base_url=")));
        assert!(args.windows(2).any(|pair| {
            pair == [
                "-c",
                "openai_base_url=\"http://127.0.0.1:9123/p/session-123/v1\"",
            ]
        }));
        assert!(args
            .windows(2)
            .any(|pair| pair == ["-c", "features.apps=false"]));
        assert!(args.ends_with(&[
            "--sandbox".to_string(),
            "workspace-write".to_string(),
            "--ask-for-approval".to_string(),
            "on-request".to_string(),
            "app-server".to_string(),
            "--stdio".to_string(),
        ]));
        assert_eq!(
            headroom_openai_base_url(9123, "session-123"),
            "http://127.0.0.1:9123/p/session-123/v1"
        );
        assert!(command
            .get_envs()
            .any(|(name, value)| name == "PATH" && value.is_some()));
    }

    #[test]
    fn fresh_and_resumed_terminals_keep_account_discovery_outside_headroom() {
        for resume in [None, Some("saved-codex-session")] {
            let spec = interactive_launch_spec_with_program(
                PathBuf::from("/tmp/codex"),
                Path::new("/tmp"),
                &SessionLane::CodexChatGpt { proxied: true },
                "default",
                resume,
                Some(9123),
                "session-123",
                Path::new("/tmp/halv.sqlite3"),
                PonytailMode::Off,
            )
            .expect("Codex terminal launch should build");
            let args = spec
                .args
                .iter()
                .map(|arg| arg.to_string_lossy())
                .collect::<Vec<_>>();
            assert!(!args.iter().any(|arg| arg.starts_with("chatgpt_base_url=")));
            assert!(args.iter().any(|arg| {
                arg == "openai_base_url=\"http://127.0.0.1:9123/p/session-123/v1\""
            }));
            assert!(!args.iter().any(|arg| arg == "app-server"));
            if let Some(session_id) = resume {
                assert_eq!(&args[args.len() - 2..], &["resume", session_id]);
            }
        }
    }

    #[test]
    fn lanes_go_direct_when_the_halv_engine_is_off() {
        let custom = SessionLane::Custom {
            env: vec![("OPENAI_API_KEY".to_string(), "openai-test-key".to_string())],
        };
        for lane in [SessionLane::CodexChatGpt { proxied: true }, custom.clone()] {
            let direct = invocation_args(
                &lane,
                "ask",
                None,
                "session-123",
                Path::new("/tmp/halv.sqlite3"),
                PonytailMode::Off,
                TaskRoutingContext::default(),
                true,
            )
            .expect("a lane without an engine port should build");
            assert!(!direct.iter().any(|arg| {
                let arg = arg.to_string_lossy();
                arg.starts_with("openai_base_url=") || arg == r#"model_provider="openai""#
            }));
            assert!(!direct
                .iter()
                .any(|arg| arg.to_string_lossy() == "features.apps=false"));
        }

        let proxied = invocation_args(
            &custom,
            "ask",
            Some(9123),
            "session-123",
            Path::new("/tmp/halv.sqlite3"),
            PonytailMode::Off,
            TaskRoutingContext::default(),
            true,
        )
        .expect("a proxied Custom API-key lane should build");
        assert!(proxied.iter().any(|arg| {
            arg.to_string_lossy() == r#"openai_base_url="http://127.0.0.1:9123/p/session-123/v1""#
        }));
    }

    #[test]
    fn nested_home_without_a_port_has_no_halv_provider() {
        let session_id = format!("nested-home-direct-{}", crate::storage::new_uuid_v4());
        let source = env::temp_dir().join(format!(
            "halv-codex-source-{}",
            crate::storage::new_uuid_v4()
        ));
        fs::create_dir_all(&source).expect("source Codex home should exist");
        fs::write(source.join("config.toml"), "model = \"gpt-test\"\n")
            .expect("source config should write");
        fs::write(source.join("auth.json"), "{}").expect("source auth should write");

        let home = prepare_nested_home_from(Some(source.clone()), None, &session_id)
            .expect("nested Codex home should be prepared");
        let config = fs::read_to_string(home.join("config.toml"))
            .expect("nested config should read")
            .parse::<toml::Table>()
            .expect("nested config should parse");

        assert_eq!(
            config.get("model").and_then(toml::Value::as_str),
            Some("gpt-test")
        );
        assert!(config.get("model_provider").is_none());
        assert!(config.get("openai_base_url").is_none());
        assert!(config.get("model_providers").is_none());
        assert!(home.join("auth.json").exists());

        session_files::remove(&session_id);
        fs::remove_dir_all(source).expect("source Codex home should be removable");
    }

    #[test]
    fn mcp_servers_are_registered_and_apps_are_disabled_only_for_proxied_codex() {
        let proxied = invocation_args(
            &SessionLane::CodexChatGpt { proxied: true },
            "ask",
            Some(9123),
            "session-123",
            Path::new("/tmp/halv.sqlite3"),
            PonytailMode::Off,
            TaskRoutingContext::default(),
            true,
        )
        .expect("proxied Codex arguments should build");
        let direct = invocation_args(
            &SessionLane::CodexChatGpt { proxied: false },
            "ask",
            None,
            "session-123",
            Path::new("/tmp/halv.sqlite3"),
            PonytailMode::Off,
            TaskRoutingContext::default(),
            true,
        )
        .expect("direct Codex arguments should build");
        let proxied = proxied
            .iter()
            .map(|argument| argument.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        let direct = direct
            .iter()
            .map(|argument| argument.to_string_lossy().into_owned())
            .collect::<Vec<_>>();

        for args in [&proxied, &direct] {
            assert!(args
                .windows(2)
                .any(|pair| pair == ["-c", "mcp_servers.crux.command=\"crux\""]));
            assert!(args
                .windows(2)
                .any(|pair| { pair == ["-c", "mcp_servers.crux.args=[\"--profile\",\"slim\"]"] }));
            let savings_command = args
                .windows(2)
                .find(|pair| {
                    pair[0] == "-c" && pair[1].starts_with("mcp_servers.halv-savings.command=")
                })
                .expect("Codex should receive the savings MCP command");
            let command: String = serde_json::from_str(
                savings_command[1]
                    .strip_prefix("mcp_servers.halv-savings.command=")
                    .expect("savings command prefix should match"),
            )
            .expect("savings command should be a TOML-compatible JSON string");
            assert!(!command.is_empty());
            let savings_args = args
                .windows(2)
                .find(|pair| {
                    pair[0] == "-c" && pair[1].starts_with("mcp_servers.halv-savings.args=")
                })
                .expect("Codex should receive the savings MCP arguments");
            let arguments: Vec<String> = serde_json::from_str(
                savings_args[1]
                    .strip_prefix("mcp_servers.halv-savings.args=")
                    .expect("savings arguments prefix should match"),
            )
            .expect("savings arguments should be a TOML-compatible JSON array");
            assert_eq!(arguments.first().map(String::as_str), Some("--mcp-savings"));
            assert!(arguments
                .windows(2)
                .any(|pair| pair == ["--session", "session-123"]));
        }
        assert!(proxied
            .windows(2)
            .any(|pair| pair == ["-c", "features.apps=false"]));
        assert!(!direct
            .iter()
            .any(|argument| argument == "features.apps=false"));
    }

    #[test]
    fn crux_gate_matrix_controls_codex_registration() {
        use crate::extensions::CruxActivation;

        let above_files = crux_census(&[("rust", 200, 1)]);
        let above_lines = crux_census(&[("rust", 1, 75_000)]);
        let below_gate = crux_census(&[("rust", 199, 74_999)]);
        let cases = [
            (
                "auto files",
                CruxActivation::Auto,
                Some(&above_files),
                true,
                true,
                true,
            ),
            (
                "auto lines",
                CruxActivation::Auto,
                Some(&above_lines),
                true,
                true,
                true,
            ),
            (
                "auto below",
                CruxActivation::Auto,
                Some(&below_gate),
                true,
                true,
                false,
            ),
            (
                "always on",
                CruxActivation::AlwaysOn,
                Some(&below_gate),
                true,
                true,
                true,
            ),
            (
                "always off",
                CruxActivation::AlwaysOff,
                Some(&above_files),
                true,
                true,
                false,
            ),
            (
                "built-in off",
                CruxActivation::Auto,
                Some(&above_files),
                false,
                true,
                false,
            ),
            (
                "no root",
                CruxActivation::AlwaysOn,
                Some(&below_gate),
                true,
                false,
                false,
            ),
        ];

        for (name, activation, census, builtin_enabled, has_root, expected) in cases {
            let manager = crate::extensions::ExtensionManager::default();
            manager.set_crux_test_state(
                Path::new("/tmp"),
                activation,
                builtin_enabled,
                census.cloned().unwrap_or_default(),
            );
            let crux_enabled = crate::extensions::crux_enabled_for_project(
                Some(&manager),
                has_root.then_some(Path::new("/tmp")),
            );
            assert_eq!(crux_enabled, expected, "unexpected gate result for {name}");
            let args = invocation_args(
                &SessionLane::CodexChatGpt { proxied: false },
                "ask",
                None,
                "session-123",
                Path::new("/tmp/halv.sqlite3"),
                PonytailMode::Off,
                TaskRoutingContext::default(),
                crux_enabled,
            )
            .expect("Codex arguments should build");
            let args = args
                .iter()
                .map(|argument| argument.to_string_lossy().into_owned())
                .collect::<Vec<_>>();
            let has_crux = args
                .iter()
                .any(|argument| argument.contains("mcp_servers.crux"));
            assert_eq!(has_crux, expected, "unexpected Crux arguments for {name}");
            if expected {
                assert!(args.windows(2).any(|pair| {
                    pair == ["-c", "mcp_servers.crux.args=[\"--profile\",\"slim\"]"]
                }));
            }
        }
    }

    #[test]
    fn turn_request_uses_jsonrpc_framing_without_version_field() {
        let line = user_message_line("say hi", "thread-1", FIRST_TURN_REQUEST_ID)
            .expect("turn request should serialize");
        let value: Value = serde_json::from_str(&line).expect("turn request should parse");

        assert_eq!(value["id"], FIRST_TURN_REQUEST_ID);
        assert_eq!(value["method"], "turn/start");
        assert_eq!(value["params"]["threadId"], "thread-1");
        assert_eq!(value["params"]["input"][0]["text"], "say hi");
        assert!(value.get("jsonrpc").is_none());
    }

    #[test]
    fn steer_request_targets_the_active_turn() {
        let line = steer_message_line("check tests first", "thread-1", "turn-1", 42)
            .expect("steer request should serialize");
        let value: Value = serde_json::from_str(&line).expect("steer request should parse");

        assert_eq!(value["id"], 42);
        assert_eq!(value["method"], "turn/steer");
        assert_eq!(value["params"]["threadId"], "thread-1");
        assert_eq!(value["params"]["expectedTurnId"], "turn-1");
        assert_eq!(value["params"]["input"][0]["text"], "check tests first");
    }

    #[test]
    fn request_id_parser_accepts_numbers_and_strings() {
        assert_eq!(request_id(Some(&json!(2))), Some(2));
        assert_eq!(request_id(Some(&json!("2"))), Some(2));
        assert_eq!(request_id(Some(&json!("not-a-number"))), None);
    }

    #[test]
    fn resume_startup_uses_thread_resume() {
        let lines = startup_lines(
            Path::new("/tmp/project"),
            "default",
            Some("thread-123"),
            Some("gpt-5.5"),
        );
        let thread: Value = serde_json::from_str(&lines[2]).expect("thread resume should parse");

        assert_eq!(thread["method"], "thread/resume");
        assert_eq!(thread["params"]["threadId"], "thread-123");
    }

    #[test]
    fn executable_check_rejects_missing_path() {
        assert!(!cli_binary::is_executable(Path::new(
            "/definitely/missing/halv-codex-test"
        )));
    }

    #[test]
    fn usage_window_labels_map_durations() {
        assert_eq!(usage_window_label(Some(18_000)), "5h");
        assert_eq!(usage_window_label(Some(604_800)), "7d");
        assert_eq!(usage_window_label(Some(43_200)), "12h");
        assert_eq!(usage_window_label(None), "");
    }

    #[test]
    fn wham_payload_preserves_plan_and_omits_null_secondary_window() {
        let payload: WhamUsageResponse = serde_json::from_value(json!({
            "plan_type": "pro",
            "rate_limit": {
                "primary_window": {
                    "used_percent": 21,
                    "limit_window_seconds": 604_800,
                    "reset_at": 1_788_276_307
                },
                "secondary_window": null
            },
            "additional_rate_limits": [{
                "rate_limit": {
                    "primary_window": {
                        "used_percent": 10,
                        "limit_window_seconds": 18_000
                    }
                }
            }]
        }))
        .expect("Wham usage payload should parse");

        let summary = usage_summary_from_wham(payload);

        assert_eq!(summary.plan_type.as_deref(), Some("pro"));
        assert_eq!(summary.windows.len(), 1);
        assert_eq!(summary.windows[0].label, "7d");
        assert_eq!(summary.windows[0].window_seconds, Some(604_800));
    }

    #[test]
    fn wham_windows_sort_by_duration() {
        let payload: WhamUsageResponse = serde_json::from_value(json!({
            "plan_type": "plus",
            "rate_limit": {
                "primary_window": {
                    "limit_window_seconds": 604_800
                },
                "secondary_window": {
                    "limit_window_seconds": 18_000
                }
            }
        }))
        .expect("Wham usage payload should parse");

        let summary = usage_summary_from_wham(payload);

        assert_eq!(
            summary
                .windows
                .iter()
                .map(|window| window.window_seconds)
                .collect::<Vec<_>>(),
            vec![Some(18_000), Some(604_800)]
        );
    }

    #[test]
    fn rollout_file_names_yield_creation_time_and_session_id() {
        let parsed = rollout_created_at_and_session_id(Path::new(
            "/s/2026/08/17/rollout-2026-08-17T16-26-01-01a01130-1fd4-77a3-9418-41aa1a04462a.jsonl",
        ))
        .expect("rollout name should parse");
        assert_eq!(parsed.1, "01a01130-1fd4-77a3-9418-41aa1a04462a");
        assert_eq!(
            parsed.0,
            chrono::NaiveDateTime::parse_from_str("2026-08-17T16-26-01", "%Y-%m-%dT%H-%M-%S")
                .expect("fixture timestamp should parse"),
        );
        assert!(rollout_created_at_and_session_id(Path::new("/s/rollout-short.jsonl")).is_none());
        assert!(rollout_created_at_and_session_id(Path::new("/s/notes.txt")).is_none());
    }

    #[test]
    fn rollout_file_names_with_multibyte_characters_do_not_panic() {
        // The split index lands inside the two-byte `é`.
        let name = format!("/s/rollout-{}é{}.jsonl", "a".repeat(10), "b".repeat(36));
        assert!(rollout_created_at_and_session_id(Path::new(&name)).is_none());
    }

    #[test]
    fn discovery_uses_creation_time_so_busy_siblings_are_not_stolen() {
        let root = std::env::temp_dir().join(format!(
            "halv-codex-discovery-{:016x}",
            std::process::id() as u64 ^ 0x5eed
        ));
        let now = chrono::Local::now().naive_local();
        let day = root.join(format!("sessions/{}", now.format("%Y/%m/%d")));
        std::fs::create_dir_all(&day).expect("test directory should exist");
        let write = |offset_seconds: i64, session_id: &str, cwd: &str| {
            let created_at = now + chrono::Duration::seconds(offset_seconds);
            std::fs::write(
                day.join(format!(
                    "rollout-{}-{session_id}.jsonl",
                    created_at.format("%Y-%m-%dT%H-%M-%S")
                )),
                format!(
                    "{}\n",
                    json!({
                        "type": "session_meta",
                        "payload": { "id": "x", "cwd": cwd }
                    })
                ),
            )
            .expect("rollout fixture should write");
        };
        // A busy sibling created a minute ago still gets written (fresh
        // mtime), but its creation time keeps it out of the candidates.
        write(-60, "99999999-9999-4999-8999-999999999999", "/work/project");
        write(10, "11111111-1111-4111-8111-111111111111", "/work/project");
        write(11, "22222222-2222-4222-8222-222222222222", "/work/project");
        write(12, "33333333-3333-4333-8333-333333333333", "/work/other");

        let previous = env::var_os("CODEX_HOME");
        env::set_var("CODEX_HOME", &root);
        let ids = discover_session_ids(Path::new("/work/project"), SystemTime::now());
        match previous {
            Some(value) => env::set_var("CODEX_HOME", value),
            None => env::remove_var("CODEX_HOME"),
        }
        let _ = std::fs::remove_dir_all(&root);

        assert_eq!(
            ids,
            vec![
                "11111111-1111-4111-8111-111111111111".to_string(),
                "22222222-2222-4222-8222-222222222222".to_string(),
            ]
        );
    }

    #[test]
    fn golden_codex_launch_protocol_and_parser_contract() {
        let lane = SessionLane::Custom {
            env: vec![("CODEX_GOLDEN".to_string(), "fixed".to_string())],
        };
        let permission_modes = [
            (
                "read_only",
                vec!["--sandbox", "read-only", "--ask-for-approval", "never"],
                "never",
                "read-only",
            ),
            (
                "ask",
                vec![
                    "--sandbox",
                    "workspace-write",
                    "--ask-for-approval",
                    "on-request",
                ],
                "on-request",
                "workspace-write",
            ),
            (
                "on_request",
                vec![
                    "--sandbox",
                    "workspace-write",
                    "--ask-for-approval",
                    "on-request",
                    "-c",
                    "approvals_reviewer=\"auto_review\"",
                ],
                "on-request",
                "workspace-write",
            ),
            (
                "on_failure",
                vec![
                    "-c",
                    "approval_policy=\"on-failure\"",
                    "-c",
                    "sandbox_mode=\"workspace-write\"",
                ],
                "on-failure",
                "workspace-write",
            ),
            (
                "never",
                vec![
                    "--sandbox",
                    "workspace-write",
                    "--ask-for-approval",
                    "never",
                ],
                "never",
                "workspace-write",
            ),
            ("skip_all", vec!["--yolo"], "never", "danger-full-access"),
        ];

        for (mode, expected_flags, approval_policy, sandbox) in permission_modes {
            let expected_flags = expected_flags
                .into_iter()
                .map(OsString::from)
                .collect::<Vec<_>>();
            let flags = permission_flags(AgentKind::Codex, mode)
                .expect("Codex permission flags should build");
            assert_eq!(flags.cli_args(), expected_flags);
            assert_eq!(
                flags.codex_settings,
                Some(CodexApprovalSettings {
                    approval_policy,
                    sandbox,
                })
            );

            let expected_base = invocation_args(
                &lane,
                mode,
                None,
                "golden-session",
                Path::new("/tmp/golden.sqlite3"),
                PonytailMode::Off,
                TaskRoutingContext::default(),
                false,
            )
            .expect("Codex invocation arguments should build");
            for resume in [None, Some("codex-resume")] {
                let command = gui_command_with_program(
                    PathBuf::from("/golden/codex"),
                    Path::new("/tmp"),
                    &lane,
                    mode,
                    None,
                    "golden-session",
                    Path::new("/tmp/golden.sqlite3"),
                    PonytailMode::Off,
                )
                .expect("Codex GUI launch should build");
                assert_eq!(command.get_program(), Path::new("/golden/codex"));
                assert_eq!(
                    command.get_args().map(OsString::from).collect::<Vec<_>>(),
                    expected_base
                );

                let terminal = interactive_launch_spec_with_program(
                    PathBuf::from("/golden/codex"),
                    Path::new("/tmp"),
                    &lane,
                    mode,
                    resume,
                    None,
                    "golden-session",
                    Path::new("/tmp/golden.sqlite3"),
                    PonytailMode::Off,
                )
                .expect("Codex interactive launch should build");
                let mut expected_terminal = expected_base[..expected_base.len() - 2].to_vec();
                expected_terminal.extend(TERMINAL_NOTIFICATION_FLAGS.map(OsString::from));
                if let Some(session_id) = resume {
                    expected_terminal
                        .extend([OsString::from("resume"), OsString::from(session_id)]);
                }
                assert_eq!(terminal.program, PathBuf::from("/golden/codex"));
                assert_eq!(terminal.args, expected_terminal);
                assert_eq!(
                    terminal.env,
                    vec![
                        (OsString::from("CODEX_GOLDEN"), OsString::from("fixed")),
                        (
                            OsString::from("HEADROOM_PROJECT"),
                            OsString::from("golden-session")
                        ),
                    ]
                );
                assert!(terminal.env_remove.is_empty());
            }
        }

        assert_eq!(
            user_message_line("say hi", "thread-1", FIRST_TURN_REQUEST_ID)
                .expect("Codex user message should encode"),
            r#"{"id":3,"method":"turn/start","params":{"input":[{"text":"say hi","type":"text"}],"threadId":"thread-1"}}"#
        );
        assert_eq!(
            steer_message_line("check", "thread-1", "turn-1", 4)
                .expect("Codex steer message should encode"),
            r#"{"id":4,"method":"turn/steer","params":{"expectedTurnId":"turn-1","input":[{"text":"check","type":"text"}],"threadId":"thread-1"}}"#
        );
        let mut parser = CodexEventParser::default();
        assert_eq!(
            parser
                .parse_line(r#"{"id":2,"result":{"thread":{"id":"thread-1"},"model":"gpt-5.6"}}"#,)
                .events,
            vec![AgentEvent::SessionInit {
                cli_session_id: "thread-1".to_string(),
                model: "gpt-5.6".to_string(),
            }]
        );
    }

    #[test]
    fn adapter_login_launch_spec_is_exact() {
        assert_eq!(
            login_launch_spec_with_program(PathBuf::from("/golden/codex")),
            LaunchSpec {
                program: PathBuf::from("/golden/codex"),
                args: vec![OsString::from("login")],
                env: Vec::new(),
                env_remove: Vec::new(),
            }
        );
    }

    #[test]
    fn task_routing_appends_the_coordinator_instructions() {
        let launch_args = |enabled: bool| {
            invocation_args(
                &SessionLane::CodexChatGpt { proxied: false },
                "ask",
                None,
                "session-123",
                Path::new("/tmp/halv.sqlite3"),
                PonytailMode::Off,
                TaskRoutingContext {
                    enabled,
                    coordinator_model: Some("gpt-6-astra"),
                    conversation_id: Some("conversation-1"),
                    ..TaskRoutingContext::default()
                },
                false,
            )
            .expect("Codex arguments should build")
            .into_iter()
            .map(|argument| argument.to_string_lossy().into_owned())
            .collect::<Vec<_>>()
        };
        let routed_args = launch_args(true);
        let routed = routed_args
            .iter()
            .find(|argument| argument.starts_with("developer_instructions="))
            .expect("the developer instructions should exist");
        assert!(routed.contains("Halv task routing"));
        assert!(routed.contains("investigate-l1-l2-v1"));
        assert!(
            routed.contains("Use the Codex hierarchy when the requested `workerLane` is `codex`")
        );
        assert!(routed.contains("A saved tier assignment can override it for the selected tier"));
        assert!(routed.contains("The read-only role is an instruction and audit contract"));
        assert!(routed.contains("No L3 role exists"));
        assert!(routed.contains("Runtime tool and worker events provide progress"));
        assert!(routed.contains("timeout_ms` set to 3600000"));
        assert!(routed.contains("takes precedence over"));
        assert!(routed.contains("Keep the complete task as one brief by default"));
        assert!(routed.contains("No part may need another part's unfinished code"));
        assert!(routed.contains("Route every task brief, even when you expect tier `max`"));
        assert!(routed.contains("Start the worker even when its model and effort match yours"));
        assert!(routed.contains("Use native `spawn_agent` for a Codex worker"));
        assert!(routed.contains("Do not call `route_task` in parallel with repository discovery"));
        for setting in [
            "features.multi_agent=true",
            "features.multi_agent_v2=true",
            "agents.max_depth=2",
            "agents.max_threads=8",
        ] {
            assert!(routed_args.windows(2).any(|pair| pair == ["-c", setting]));
        }

        assert!(!routed_args
            .iter()
            .any(|argument| argument.contains("--task-routing")));

        let plain_args = launch_args(false);
        assert!(!plain_args
            .iter()
            .any(|argument| argument.contains("Halv task routing")));
        for setting in ["multi_agent", "max_depth", "max_threads"] {
            assert!(!plain_args.iter().any(|argument| argument.contains(setting)));
        }
        // Without routing, the savings server lists no routing tool
        // (ADR 0023). The savings tools stay.
        let savings_args = plain_args
            .iter()
            .find(|argument| argument.starts_with("mcp_servers.halv-savings.args="))
            .expect("the savings server arguments should exist");
        assert!(
            savings_args.contains(r#""--task-routing","off""#),
            "{savings_args}"
        );
    }
}
