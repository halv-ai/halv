//! Kimi CLI creates its OAuth provider with type `kimi` and the platform base URL.
//! See MoonshotAI/kimi-cli `src/kimi_cli/auth/oauth.py:570-575`.
//! The CLI applies `KIMI_BASE_URL` to each `kimi` provider.
//! See MoonshotAI/kimi-cli `src/kimi_cli/llm.py:276-300`.
//! `_kimi_default_headers` adds `oauth.common_headers()` to each OAuth request.
//! The proxy base URL therefore keeps OAuth authentication intact.

use super::{
    adapter::{
        AdapterProcess, AdapterState, AgentAdapter, AgentProbe, CanonicalMode,
        InteractiveLaunchContext, LaunchContext, ParsedTurn, PermissionFlags, TurnKind,
    },
    canonical_permission_mode,
    cli_binary::{self, HostOs},
    savings_stack_environment, session_files, AgentEvent, AgentKind, LaunchSpec, SessionLane,
    TurnUsage, RTK_DEVELOPER_INSTRUCTIONS,
};
use crate::host_env;
use serde_json::{json, Value};
use std::{
    collections::{HashMap, HashSet},
    env,
    ffi::OsString,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
};

const KIMI_BINARY_NAMES: [&str; 2] = ["kimi", "kimi-code"];
const KIMI_MODEL_NAME: &str = "kimi-for-coding";
pub(super) const KIMI_API_KEY_UPSTREAM: &str = "https://api.moonshot.ai/v1";
pub(super) const KIMI_SUBSCRIPTION_UPSTREAM: &str = "https://api.kimi.com/coding/v1";

struct KimiLaunchConfig {
    binary: PathBuf,
    cwd: PathBuf,
    environment: Vec<(OsString, OsString)>,
    mcp_args: Vec<OsString>,
    permission_mode: String,
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
    let mut command = Command::new(&binary);
    command.arg("--version");
    host_env::apply_to_command(&mut command);
    crate::process::hide_console(&mut command);
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
                    format!("kimi --version exited with {}", output.status)
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

pub(super) struct KimiAdapterState {
    parser: KimiEventParser,
    launch_config: Option<KimiLaunchConfig>,
}

pub(super) struct KimiAdapter;
pub(super) static KIMI_ADAPTER: KimiAdapter = KimiAdapter;

fn permission_flags(
    agent_kind: AgentKind,
    permission_mode: &str,
) -> Result<PermissionFlags, String> {
    if agent_kind != AgentKind::Kimi {
        return Err("Kimi permission mapping received an incompatible agent kind".to_string());
    }
    KIMI_ADAPTER.permission_flags(canonical_permission_mode(permission_mode)?)
}

impl AgentAdapter for KimiAdapter {
    fn kind(&self) -> AgentKind {
        AgentKind::Kimi
    }

    fn probe(&self) -> AgentProbe {
        probe()
    }

    fn binary(&self) -> Option<PathBuf> {
        find_binary()
    }

    fn permission_flags(&self, mode: CanonicalMode) -> Result<PermissionFlags, String> {
        let mut flags = PermissionFlags::empty();
        if mode == CanonicalMode::SkipAll {
            flags.args.push(OsString::from("--yolo"));
        }
        Ok(flags)
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
            ctx.nested_codex_home,
            ctx.extension_manager,
        )
    }

    fn start(
        &self,
        ctx: &LaunchContext<'_>,
        state: &mut AdapterState,
    ) -> Result<AdapterProcess, String> {
        let AdapterState::Kimi(state) = state else {
            return Err("Kimi adapter received incompatible session state".to_string());
        };
        state.launch_config = Some(KimiLaunchConfig::new(
            ctx.cwd,
            ctx.lane,
            ctx.permission_mode.as_str(),
            ctx.proxy_port,
            ctx.project,
            ctx.nested_codex_home,
            ctx.extension_manager,
        )?);
        Ok(AdapterProcess::deferred())
    }

    fn new_state(
        &self,
        resumed: bool,
        _permission_mode: CanonicalMode,
        _started_at: std::time::Instant,
        _initializing: bool,
    ) -> AdapterState {
        AdapterState::Kimi(KimiAdapterState {
            parser: KimiEventParser::with_session_initialized(resumed),
            launch_config: None,
        })
    }

    fn parse_line(&self, state: &mut AdapterState, line: &str) -> ParsedTurn {
        let AdapterState::Kimi(state) = state else {
            return ParsedTurn {
                events: vec![AgentEvent::ProtocolError {
                    message: "Kimi adapter received incompatible parser state".to_string(),
                }],
                ..ParsedTurn::default()
            };
        };
        ParsedTurn {
            events: state.parser.parse_line(line),
            ..ParsedTurn::default()
        }
    }

    fn encode_user_message(
        &self,
        _state: &mut AdapterState,
        _text: &str,
        turn: TurnKind<'_>,
    ) -> Result<Option<String>, String> {
        match turn {
            TurnKind::Start { .. } => Ok(None),
            TurnKind::Steer { .. } | TurnKind::Queued { .. } => {
                Err("Kimi does not support steering an active turn".to_string())
            }
        }
    }

    fn start_turn(
        &self,
        state: &mut AdapterState,
        text: &str,
        resume_cli_session_id: Option<&str>,
    ) -> Result<AdapterProcess, String> {
        let AdapterState::Kimi(state) = state else {
            return Err("Kimi adapter received incompatible session state".to_string());
        };
        let config = state
            .launch_config
            .as_ref()
            .ok_or_else(|| "the Kimi launch configuration is unavailable".to_string())?;
        AdapterProcess::per_turn(config.spawn_turn(text, resume_cli_session_id)?, "Kimi")
    }

    fn is_per_turn(&self) -> bool {
        true
    }

    fn assigned_interactive_session_id(&self, _resume_cli_session_id: Option<&str>) -> bool {
        false
    }

    fn uses_rtk_instruction(&self) -> bool {
        true
    }

    fn supports_nested_codex_home(&self, proxy_shutdown_on_exit: bool) -> bool {
        !proxy_shutdown_on_exit
    }

    fn accepts_initial_prompt(&self) -> bool {
        false
    }

    fn isolated_proxy_upstream(&self, lane: &SessionLane) -> Option<&'static str> {
        match lane {
            SessionLane::KimiApiKey { .. } => Some(KIMI_API_KEY_UPSTREAM),
            SessionLane::KimiSubscription => Some(KIMI_SUBSCRIPTION_UPSTREAM),
            _ => None,
        }
    }
}

fn login_launch_spec() -> Result<LaunchSpec, String> {
    let program = find_binary().ok_or_else(|| {
        "Kimi Code was not found in PATH or a supported install location".to_string()
    })?;
    let mut command = Command::new(&program);
    command.arg("--help");
    host_env::apply_to_command(&mut command);
    crate::process::hide_console(&mut command);
    let help = command
        .output()
        .ok()
        .map(|output| {
            format!(
                "{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            )
        })
        .unwrap_or_default();
    Ok(login_launch_spec_with_program_and_help(program, &help))
}

fn login_launch_spec_with_program_and_help(program: PathBuf, help: &str) -> LaunchSpec {
    LaunchSpec {
        program,
        args: login_args_from_help(help),
        env: Vec::new(),
        env_remove: Vec::new(),
    }
}

fn login_args_from_help(help: &str) -> Vec<OsString> {
    let has_login = help
        .lines()
        .map(str::trim)
        .any(|line| line == "login" || line.starts_with("login "));
    if has_login {
        vec![OsString::from("login")]
    } else {
        Vec::new()
    }
}

impl KimiLaunchConfig {
    pub(super) fn new(
        cwd: &Path,
        lane: &SessionLane,
        permission_mode: &str,
        proxy_port: Option<u16>,
        project: &str,
        nested_codex_home: Option<&Path>,
        extension_manager: &crate::extensions::ExtensionManager,
    ) -> Result<Self, String> {
        let binary = find_binary().ok_or_else(|| {
            "Kimi Code was not found in PATH or a supported install location".to_string()
        })?;
        Self::new_with_program(
            binary,
            cwd,
            lane,
            permission_mode,
            proxy_port,
            project,
            nested_codex_home,
            crate::extensions::crux_enabled_for_project(Some(extension_manager), Some(cwd)),
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn new_with_program(
        binary: PathBuf,
        cwd: &Path,
        lane: &SessionLane,
        permission_mode: &str,
        proxy_port: Option<u16>,
        project: &str,
        nested_codex_home: Option<&Path>,
        crux_enabled: bool,
    ) -> Result<Self, String> {
        let mut environment = lane_environment(lane, proxy_port, project, nested_codex_home)?;
        environment.extend(savings_stack_environment(project));
        let mcp_args = mcp_config_args(&binary, project, crux_enabled)?;
        Ok(Self {
            binary,
            cwd: cwd.to_path_buf(),
            environment,
            mcp_args,
            permission_mode: canonical_permission_mode(permission_mode)?.to_string(),
        })
    }

    fn spawn_turn(
        &self,
        prompt: &str,
        resume_cli_session_id: Option<&str>,
    ) -> Result<Child, String> {
        let mut command = turn_command(
            &self.binary,
            &self.cwd,
            &self.environment,
            &self.mcp_args,
            &self.permission_mode,
            prompt,
            resume_cli_session_id,
        )?;
        command
            .spawn()
            .map_err(|error| format!("failed to start Kimi Code: {error}"))
    }
}

#[allow(clippy::too_many_arguments)]
fn interactive_launch_spec(
    cwd: &Path,
    lane: &SessionLane,
    permission_mode: &str,
    resume_cli_session_id: Option<&str>,
    proxy_port: Option<u16>,
    project: &str,
    nested_codex_home: Option<&Path>,
    extension_manager: &crate::extensions::ExtensionManager,
) -> Result<LaunchSpec, String> {
    let program = find_binary().ok_or_else(|| {
        "Kimi Code was not found in PATH or supported install location".to_string()
    })?;
    interactive_launch_spec_with_program_and_crux(
        program,
        lane,
        permission_mode,
        resume_cli_session_id,
        proxy_port,
        project,
        nested_codex_home,
        crate::extensions::crux_enabled_for_project(Some(extension_manager), Some(cwd)),
    )
}

#[cfg(test)]
pub(super) fn interactive_launch_spec_with_program(
    program: PathBuf,
    lane: &SessionLane,
    permission_mode: &str,
    resume_cli_session_id: Option<&str>,
    proxy_port: Option<u16>,
    project: &str,
    nested_codex_home: Option<&Path>,
) -> Result<LaunchSpec, String> {
    interactive_launch_spec_with_program_and_crux(
        program,
        lane,
        permission_mode,
        resume_cli_session_id,
        proxy_port,
        project,
        nested_codex_home,
        false,
    )
}

#[allow(clippy::too_many_arguments)]
fn interactive_launch_spec_with_program_and_crux(
    program: PathBuf,
    lane: &SessionLane,
    permission_mode: &str,
    resume_cli_session_id: Option<&str>,
    proxy_port: Option<u16>,
    project: &str,
    nested_codex_home: Option<&Path>,
    crux_enabled: bool,
) -> Result<LaunchSpec, String> {
    let mcp_args = mcp_config_args(&program, project, crux_enabled)?;
    interactive_launch_spec_with_mcp_args(
        program,
        lane,
        permission_mode,
        resume_cli_session_id,
        proxy_port,
        project,
        nested_codex_home,
        mcp_args,
    )
}

#[allow(clippy::too_many_arguments)]
fn interactive_launch_spec_with_mcp_args(
    program: PathBuf,
    lane: &SessionLane,
    permission_mode: &str,
    resume_cli_session_id: Option<&str>,
    proxy_port: Option<u16>,
    project: &str,
    nested_codex_home: Option<&Path>,
    mut args: Vec<OsString>,
) -> Result<LaunchSpec, String> {
    // Kimi does not support Ponytail instructions.
    args.extend(permission_flags(AgentKind::Kimi, permission_mode)?.cli_args());
    if let Some(cli_session_id) = resume_cli_session_id {
        args.push(OsString::from("--resume"));
        args.push(OsString::from(cli_session_id));
    }
    Ok(LaunchSpec {
        program,
        args,
        env: lane_environment(lane, proxy_port, project, nested_codex_home)?,
        env_remove: Vec::new(),
    })
}

fn turn_command(
    binary: &Path,
    cwd: &Path,
    environment: &[(OsString, OsString)],
    mcp_args: &[OsString],
    permission_mode: &str,
    prompt: &str,
    resume_cli_session_id: Option<&str>,
) -> Result<Command, String> {
    let mut command = Command::new(binary);
    command
        .args(mcp_args)
        .args(permission_flags(AgentKind::Kimi, permission_mode)?.cli_args());
    if let Some(cli_session_id) = resume_cli_session_id {
        command.arg("--resume").arg(cli_session_id);
    }
    command
        .arg("--prompt")
        .arg(prompt)
        .arg("--output-format")
        .arg("stream-json")
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        // The session's stderr tail keeps the diagnostics for a failed turn.
        .stderr(Stdio::piped());
    for (name, value) in environment {
        command.env(name, value);
    }
    host_env::apply_to_command(&mut command);
    crate::process::hide_console(&mut command);
    Ok(command)
}

fn mcp_config_args(
    program: &Path,
    session_id: &str,
    crux_enabled: bool,
) -> Result<Vec<OsString>, String> {
    let mut args = agent_file_args(session_id)?;
    let mut command = Command::new(program);
    command.arg("--help");
    host_env::apply_to_command(&mut command);
    crate::process::hide_console(&mut command);
    let help = command
        .output()
        .ok()
        .map(|output| {
            format!(
                "{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            )
        })
        .unwrap_or_default();
    if !help.contains("--mcp-config-file") {
        return Ok(args);
    }
    args.extend(supported_mcp_config_args(session_id, crux_enabled)?);
    Ok(args)
}

fn agent_file_args(session_id: &str) -> Result<Vec<OsString>, String> {
    let agent_path =
        session_files::directory(session_id)?.join(session_files::KIMI_AGENT_FILE_NAME);
    let agent = json!({
        "version": 1,
        "agent": {
            "extend": "default",
            "system_prompt_args": {
                "ROLE_ADDITIONAL": RTK_DEVELOPER_INSTRUCTIONS,
            },
        },
    });
    crate::exec::apply_patch::atomic_write_file(
        &agent_path,
        &serde_json::to_vec(&agent)
            .map_err(|error| format!("failed to encode the Kimi agent file: {error}"))?,
    )
    .map_err(|error| format!("failed to write the Kimi agent file: {error}"))?;
    Ok(vec![
        OsString::from("--agent-file"),
        agent_path.into_os_string(),
    ])
}

fn supported_mcp_config_args(
    session_id: &str,
    crux_enabled: bool,
) -> Result<Vec<OsString>, String> {
    let config_path = session_files::directory(session_id)?.join(session_files::KIMI_MCP_FILE_NAME);
    let mut servers = serde_json::Map::new();
    if crux_enabled {
        servers.insert(
            "crux".to_string(),
            json!({
                "command": "crux",
                "args": ["--profile", "slim"],
                "env": {
                    "PATH": crate::extensions::crux_path_environment(),
                    "CRUX_SAVINGS_LEDGER": crate::exec::crux_session_ledger_path(session_id)
                        .to_string_lossy(),
                },
            }),
        );
    }
    let config = json!({ "mcpServers": servers });
    crate::exec::apply_patch::atomic_write_file(
        &config_path,
        &serde_json::to_vec(&config)
            .map_err(|error| format!("failed to encode the Kimi MCP configuration: {error}"))?,
    )
    .map_err(|error| format!("failed to write the Kimi MCP configuration: {error}"))?;
    Ok(vec![
        OsString::from("--mcp-config-file"),
        config_path.into_os_string(),
    ])
}

fn lane_environment(
    lane: &SessionLane,
    proxy_port: Option<u16>,
    project: &str,
    nested_codex_home: Option<&Path>,
) -> Result<Vec<(OsString, OsString)>, String> {
    let mut environment = match lane {
        SessionLane::KimiApiKey { credential } => {
            let api_key = credential.api_key()?;
            if api_key.is_empty() {
                return Err("the Kimi API key cannot be empty".to_string());
            }
            let port = proxy_port.ok_or_else(|| {
                "the Kimi API-key lane requires a managed Headroom proxy".to_string()
            })?;
            let base_url = format!("http://127.0.0.1:{port}/p/{project}/v1");
            vec![
                ("KIMI_MODEL_NAME".into(), KIMI_MODEL_NAME.into()),
                ("KIMI_MODEL_API_KEY".into(), api_key.into()),
                ("KIMI_MODEL_PROVIDER_TYPE".into(), "kimi".into()),
                ("KIMI_MODEL_BASE_URL".into(), base_url.into()),
            ]
        }
        SessionLane::KimiSubscription => {
            let port = proxy_port.ok_or_else(|| {
                "the Kimi subscription lane requires a managed Headroom proxy".to_string()
            })?;
            let base_url = format!("http://127.0.0.1:{port}/p/{project}/v1");
            vec![
                ("KIMI_MODEL_NAME".into(), KIMI_MODEL_NAME.into()),
                ("KIMI_BASE_URL".into(), base_url.into()),
            ]
        }
        SessionLane::ClaudeSubscription
        | SessionLane::ClaudeApiKey { .. }
        | SessionLane::CodexChatGpt { .. }
        | SessionLane::GrokApiKey { .. }
        | SessionLane::ZaiGlm { .. }
        | SessionLane::Custom { .. } => {
            return Err("this lane cannot be used with Kimi".to_string())
        }
    };
    environment.push(("HEADROOM_PROJECT".into(), project.into()));
    if let Some(codex_home) = nested_codex_home {
        environment.push(("CODEX_HOME".into(), codex_home.as_os_str().to_os_string()));
    }
    Ok(environment)
}

#[derive(Default)]
struct KimiEventParser {
    session_initialized: bool,
    tool_names: HashMap<String, String>,
}

impl KimiEventParser {
    pub(super) fn with_session_initialized(session_initialized: bool) -> Self {
        Self {
            session_initialized,
            tool_names: HashMap::new(),
        }
    }

    fn parse_line(&mut self, line: &str) -> Vec<AgentEvent> {
        let value = match serde_json::from_str::<Value>(line) {
            Ok(value) => value,
            Err(error) => {
                return vec![AgentEvent::ProtocolError {
                    message: format!("invalid Kimi stream JSON: {error}"),
                }];
            }
        };
        match value.get("role").and_then(Value::as_str) {
            Some("assistant") => self.parse_assistant(&value),
            Some("tool") => self.parse_tool(&value).into_iter().collect(),
            Some("meta") => self.parse_meta(&value),
            _ => Vec::new(),
        }
    }

    fn parse_assistant(&mut self, value: &Value) -> Vec<AgentEvent> {
        let mut events = Vec::new();
        if let Some(text) = assistant_text(value.get("content")) {
            if !text.is_empty() {
                events.push(AgentEvent::TextDelta { text });
            }
        }
        if let Some(tool_calls) = value.get("tool_calls").and_then(Value::as_array) {
            for tool_call in tool_calls {
                let Some(function) = tool_call.get("function") else {
                    continue;
                };
                let name = function
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("tool")
                    .to_string();
                if let Some(id) = tool_call.get("id").and_then(Value::as_str) {
                    self.tool_names.insert(id.to_string(), name.clone());
                }
                let summary_json = function
                    .get("arguments")
                    .map(|arguments| {
                        arguments
                            .as_str()
                            .map(str::to_owned)
                            .unwrap_or_else(|| arguments.to_string())
                    })
                    .unwrap_or_else(|| "{}".to_string());
                events.push(AgentEvent::ToolCallStarted { name, summary_json });
            }
        }
        events
    }

    fn parse_tool(&mut self, value: &Value) -> Option<AgentEvent> {
        let id = value.get("tool_call_id").and_then(Value::as_str)?;
        let name = self
            .tool_names
            .remove(id)
            .unwrap_or_else(|| "tool".to_string());
        Some(AgentEvent::ToolCallCompleted { name })
    }

    fn parse_meta(&mut self, value: &Value) -> Vec<AgentEvent> {
        if value.get("type").and_then(Value::as_str) != Some("session.resume_hint") {
            return Vec::new();
        }
        let mut events = Vec::new();
        if !self.session_initialized {
            if let Some(cli_session_id) = value.get("session_id").and_then(Value::as_str) {
                events.push(AgentEvent::SessionInit {
                    cli_session_id: cli_session_id.to_string(),
                    model: KIMI_MODEL_NAME.to_string(),
                });
                self.session_initialized = true;
            }
        }
        events.push(AgentEvent::TurnCompleted {
            usage: parse_usage(value.get("usage")),
        });
        events
    }
}

fn assistant_text(content: Option<&Value>) -> Option<String> {
    match content? {
        Value::String(text) => Some(text.clone()),
        Value::Array(items) => {
            let text = items
                .iter()
                .filter_map(|item| item.get("text").and_then(Value::as_str))
                .collect::<String>();
            (!text.is_empty()).then_some(text)
        }
        _ => None,
    }
}

fn parse_usage(usage: Option<&Value>) -> TurnUsage {
    let usage = usage.unwrap_or(&Value::Null);
    TurnUsage {
        input_tokens: token_value(usage, &["input_tokens", "prompt_tokens"]),
        output_tokens: token_value(usage, &["output_tokens", "completion_tokens"]),
        cache_read_tokens: token_value(usage, &["cache_read_input_tokens", "cached_tokens"]),
        cache_creation_tokens: token_value(usage, &["cache_creation_input_tokens"]),
        total_cost_usd: usage.get("total_cost_usd").and_then(Value::as_f64),
    }
}

fn token_value(value: &Value, names: &[&str]) -> u64 {
    names
        .iter()
        .find_map(|name| value.get(name).and_then(Value::as_u64))
        .unwrap_or_default()
}

fn find_binary() -> Option<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(path) = env::var_os("PATH") {
        for directory in env::split_paths(&path) {
            add_binary_candidates(&mut candidates, &directory);
        }
    }
    if let Some(home) = home_directory() {
        for directory in [
            home.join(".local/bin"),
            home.join(".npm-global/bin"),
            home.join(".kimi/bin"),
        ] {
            add_binary_candidates(&mut candidates, &directory);
        }
    }
    for directory in [
        PathBuf::from("/opt/homebrew/bin"),
        PathBuf::from("/usr/local/bin"),
        PathBuf::from("/usr/bin"),
    ] {
        add_binary_candidates(&mut candidates, &directory);
    }
    let mut seen = HashSet::new();
    candidates
        .into_iter()
        .find(|candidate| seen.insert(candidate.clone()) && cli_binary::is_executable(candidate))
}

fn add_binary_candidates(candidates: &mut Vec<PathBuf>, directory: &Path) {
    for name in KIMI_BINARY_NAMES {
        cli_binary::push_candidates(candidates, directory, name, HostOs::current());
    }
}

fn home_directory() -> Option<PathBuf> {
    #[cfg(windows)]
    let value: Option<OsString> = env::var_os("USERPROFILE");
    #[cfg(not(windows))]
    let value: Option<OsString> = env::var_os("HOME");
    value.map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

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
    fn gui_and_terminal_use_the_same_permission_arguments() {
        for mode in [
            "read_only",
            "ask",
            "on_request",
            "on_failure",
            "never",
            "skip_all",
        ] {
            let expected = permission_flags(AgentKind::Kimi, mode)
                .expect("Kimi permission mode should map")
                .cli_args();
            let command = turn_command(
                Path::new("kimi"),
                Path::new("/tmp"),
                &[],
                &[],
                mode,
                "hello",
                None,
            )
            .expect("Kimi GUI command should build");
            let gui_args = command.get_args().map(OsString::from).collect::<Vec<_>>();
            let terminal = interactive_launch_spec_with_program(
                PathBuf::from("/tmp/kimi"),
                &SessionLane::KimiSubscription,
                mode,
                None,
                Some(9123),
                "session-123",
                None,
            )
            .expect("Kimi terminal specification should build");

            assert!(gui_args.starts_with(&expected));
            let mut terminal_args = terminal.args;
            let agent_index = terminal_args
                .iter()
                .position(|argument| argument == "--agent-file")
                .expect("Kimi terminal arguments include agent file");
            terminal_args.drain(agent_index..=agent_index + 1);
            assert_eq!(terminal_args, expected);
        }
    }

    #[test]
    fn login_help_selects_the_login_subcommand_when_available() {
        assert_eq!(
            login_args_from_help("Commands:\n  login    Sign in\n  help"),
            vec![OsString::from("login")]
        );
        assert!(login_args_from_help("Usage: kimi [OPTIONS]").is_empty());
    }
    use std::ffi::OsStr;

    #[test]
    fn api_key_lane_sets_current_kimi_model_environment() {
        let lane = SessionLane::KimiApiKey {
            credential: super::super::SessionCredential::ApiKey {
                api_key: "moonshot-test-key".to_string(),
            },
        };
        let config = KimiLaunchConfig::new_with_program(
            PathBuf::from("/tmp/kimi"),
            Path::new("/tmp"),
            &lane,
            "ask",
            Some(9123),
            "session-123",
            None,
            false,
        )
        .expect("Kimi GUI configuration should build");
        let command = turn_command(
            Path::new("kimi"),
            Path::new("/tmp"),
            &config.environment,
            &config.mcp_args,
            &config.permission_mode,
            "hello",
            Some("kimi-session"),
        )
        .expect("Kimi GUI command should build");

        assert_eq!(
            command_env(&command, "KIMI_MODEL_NAME"),
            Some(KIMI_MODEL_NAME)
        );
        assert_eq!(
            command_env(&command, "KIMI_MODEL_API_KEY"),
            Some("moonshot-test-key")
        );
        assert_eq!(
            command_env(&command, "KIMI_MODEL_PROVIDER_TYPE"),
            Some("kimi")
        );
        assert_eq!(
            command_env(&command, "KIMI_MODEL_BASE_URL"),
            Some("http://127.0.0.1:9123/p/session-123/v1")
        );
        assert!(command_env(&command, "PATH").is_some());
        let mut args = command
            .get_args()
            .map(|argument| argument.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        let agent_index = args
            .iter()
            .position(|argument| argument == "--agent-file")
            .expect("Kimi GUI arguments include agent file");
        args.drain(agent_index..=agent_index + 1);
        assert_eq!(
            args,
            [
                "--resume",
                "kimi-session",
                "--prompt",
                "hello",
                "--output-format",
                "stream-json"
            ]
        );
    }

    #[test]
    fn subscription_lane_sets_current_kimi_proxy_environment() {
        let environment = lane_environment(
            &SessionLane::KimiSubscription,
            Some(9123),
            "session-123",
            None,
        )
        .expect("subscription environment should build");

        assert_eq!(
            environment_value(&environment, "KIMI_MODEL_NAME"),
            Some(KIMI_MODEL_NAME)
        );
        assert_eq!(
            environment_value(&environment, "KIMI_BASE_URL"),
            Some("http://127.0.0.1:9123/p/session-123/v1")
        );
        assert_eq!(
            environment_value(&environment, "HEADROOM_PROJECT"),
            Some("session-123")
        );
    }

    #[test]
    fn subscription_and_api_key_lanes_load_rtk_guidance_and_crux() {
        let lanes = [
            (SessionLane::KimiSubscription, Some(9123)),
            (
                SessionLane::KimiApiKey {
                    credential: super::super::SessionCredential::ApiKey {
                        api_key: "moonshot-test-key".to_string(),
                    },
                },
                Some(9123),
            ),
        ];

        for (lane, proxy_port) in lanes {
            let mut config_args =
                agent_file_args("session-123").expect("agent arguments should build");
            config_args.extend(
                supported_mcp_config_args("session-123", true).expect("MCP arguments should build"),
            );
            let spec = interactive_launch_spec_with_mcp_args(
                PathBuf::from("/tmp/kimi"),
                &lane,
                "ask",
                None,
                proxy_port,
                "session-123",
                None,
                config_args,
            )
            .expect("Kimi launch specification should build");
            let session_directory =
                session_files::directory("session-123").expect("session directory should exist");
            let agent_index = spec
                .args
                .iter()
                .position(|argument| argument == "--agent-file")
                .expect("Kimi launch arguments should include an agent file");
            assert_eq!(
                Path::new(&spec.args[agent_index + 1]).parent(),
                Some(session_directory.as_path())
            );
            let agent = fs::read_to_string(&spec.args[agent_index + 1])
                .expect("Kimi agent file should be readable");
            let agent: Value =
                serde_json::from_str(&agent).expect("Kimi agent file should parse as YAML JSON");
            assert_eq!(agent["version"], 1);
            assert_eq!(agent["agent"]["extend"], "default");
            assert_eq!(
                agent["agent"]["system_prompt_args"]["ROLE_ADDITIONAL"],
                RTK_DEVELOPER_INSTRUCTIONS
            );

            let mcp_index = spec
                .args
                .iter()
                .position(|argument| argument == "--mcp-config-file")
                .expect("Kimi launch arguments should include an MCP configuration");
            assert_eq!(
                Path::new(&spec.args[mcp_index + 1]).parent(),
                Some(session_directory.as_path())
            );
            let config = fs::read_to_string(&spec.args[mcp_index + 1])
                .expect("Kimi MCP configuration should be readable");
            let config: Value =
                serde_json::from_str(&config).expect("Kimi MCP configuration should parse");

            assert_eq!(config["mcpServers"]["crux"]["command"], "crux");
            assert_eq!(
                config["mcpServers"]["crux"]["args"],
                json!(["--profile", "slim"])
            );
            assert_eq!(
                config["mcpServers"]["crux"]["env"]["CRUX_SAVINGS_LEDGER"].as_str(),
                crate::exec::crux_session_ledger_path("session-123").to_str()
            );
        }
    }

    #[test]
    fn concurrent_agent_file_writes_remain_complete() {
        let session_id = format!("concurrent-agent-file-{}", crate::storage::new_uuid_v4());
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(3));
        let writers = (0..2)
            .map(|_| {
                let barrier = barrier.clone();
                let session_id = session_id.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    agent_file_args(&session_id).expect("concurrent agent file should write")
                })
            })
            .collect::<Vec<_>>();

        barrier.wait();
        let paths = writers
            .into_iter()
            .map(|writer| {
                let args = writer.join().expect("agent file writer should finish");
                PathBuf::from(args[1].clone())
            })
            .collect::<Vec<_>>();

        assert_eq!(paths[0], paths[1]);
        let contents =
            fs::read_to_string(&paths[0]).expect("concurrent Kimi agent file should be readable");
        let agent: Value = serde_json::from_str(&contents)
            .expect("concurrent Kimi agent file should be complete JSON");
        assert_eq!(agent["version"], 1);
        assert_eq!(agent["agent"]["extend"], "default");
        session_files::remove(&session_id);
    }

    #[test]
    fn crux_gate_matrix_controls_kimi_registration() {
        use crate::extensions::CruxActivation;

        let above_files = crux_census(&[("rust", 200, 1)]);
        let above_lines = crux_census(&[("rust", 1, 75_000)]);
        let below_gate = crux_census(&[("rust", 199, 74_999)]);
        let cases = [
            (CruxActivation::Auto, Some(&above_files), true, true, true),
            (CruxActivation::Auto, Some(&above_lines), true, true, true),
            (CruxActivation::Auto, Some(&below_gate), true, true, false),
            (
                CruxActivation::AlwaysOn,
                Some(&below_gate),
                true,
                true,
                true,
            ),
            (
                CruxActivation::AlwaysOff,
                Some(&above_files),
                true,
                true,
                false,
            ),
            (CruxActivation::Auto, Some(&above_files), false, true, false),
            (
                CruxActivation::AlwaysOn,
                Some(&below_gate),
                true,
                false,
                false,
            ),
        ];

        for (index, (activation, census, builtin_enabled, has_root, expected)) in
            cases.into_iter().enumerate()
        {
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
            assert_eq!(crux_enabled, expected);
            let session_id = format!("crux-gate-{index}-{}", rand::random::<u64>());
            let args = supported_mcp_config_args(&session_id, crux_enabled)
                .expect("Kimi MCP arguments should build");
            let config =
                fs::read_to_string(&args[1]).expect("Kimi MCP configuration should be readable");
            let config: Value =
                serde_json::from_str(&config).expect("Kimi MCP configuration should parse");
            let has_crux = config["mcpServers"]
                .as_object()
                .is_some_and(|servers| servers.contains_key("crux"));
            assert_eq!(has_crux, expected);
            if expected {
                assert_eq!(
                    config["mcpServers"]["crux"]["args"],
                    json!(["--profile", "slim"])
                );
            }
        }
    }

    #[test]
    fn grok_lane_is_rejected() {
        let error = lane_environment(
            &SessionLane::GrokApiKey {
                credential: super::super::SessionCredential::ApiKey {
                    api_key: "xai-test-key".to_string(),
                },
                model_overrides: None,
            },
            Some(9123),
            "session-123",
            None,
        )
        .expect_err("Grok lane must not run through Kimi");

        assert_eq!(error, "this lane cannot be used with Kimi");
    }

    #[test]
    fn parses_assistant_text_and_tool_records() {
        let mut parser = KimiEventParser::default();
        assert_eq!(
            parser.parse_line(r#"{"role":"assistant","content":"hello world"}"#),
            vec![AgentEvent::TextDelta {
                text: "hello world".to_string()
            }]
        );
        assert_eq!(
            parser.parse_line(
                r#"{"role":"assistant","content":"checking","tool_calls":[{"type":"function","id":"tc_1","function":{"name":"Shell","arguments":"{\"command\":\"ls\"}"}}]}"#,
            ),
            vec![
                AgentEvent::TextDelta {
                    text: "checking".to_string(),
                },
                AgentEvent::ToolCallStarted {
                    name: "Shell".to_string(),
                    summary_json: r#"{"command":"ls"}"#.to_string(),
                },
            ]
        );
        assert_eq!(
            parser.parse_line(
                r#"{"role":"tool","tool_call_id":"tc_1","content":"file1.py\nfile2.py"}"#,
            ),
            vec![AgentEvent::ToolCallCompleted {
                name: "Shell".to_string()
            }]
        );
    }

    #[test]
    fn parses_resume_hint_as_session_init_and_turn_completion() {
        let mut parser = KimiEventParser::default();
        assert_eq!(
            parser.parse_line(
                r#"{"role":"meta","type":"session.resume_hint","session_id":"ses_prompt","command":"kimi -r ses_prompt","content":"To resume this session: kimi -r ses_prompt"}"#,
            ),
            vec![
                AgentEvent::SessionInit {
                    cli_session_id: "ses_prompt".to_string(),
                    model: KIMI_MODEL_NAME.to_string(),
                },
                AgentEvent::TurnCompleted {
                    usage: TurnUsage::default(),
                },
            ]
        );
    }

    #[test]
    fn reports_invalid_json_without_stopping_parser() {
        let mut parser = KimiEventParser::default();
        assert!(matches!(
            parser.parse_line("{not-json").as_slice(),
            [AgentEvent::ProtocolError { message }] if message.contains("invalid Kimi stream JSON")
        ));
        assert!(parser
            .parse_line(r#"{"role":"activity","type":"unknown"}"#)
            .is_empty());
    }

    fn command_env<'a>(command: &'a Command, name: &str) -> Option<&'a str> {
        command.get_envs().find_map(|(candidate, value)| {
            (candidate == OsStr::new(name))
                .then_some(value)
                .flatten()
                .and_then(OsStr::to_str)
        })
    }

    fn environment_value<'a>(
        environment: &'a [(OsString, OsString)],
        name: &str,
    ) -> Option<&'a str> {
        environment
            .iter()
            .find_map(|(key, value)| (key == name).then(|| value.to_str()).flatten())
    }

    #[test]
    fn golden_kimi_launch_protocol_and_parser_contract() {
        let lane = SessionLane::KimiSubscription;
        let permission_modes = [
            ("read_only", Vec::<&str>::new()),
            ("ask", Vec::new()),
            ("on_request", Vec::new()),
            ("on_failure", Vec::new()),
            ("never", Vec::new()),
            ("skip_all", vec!["--yolo"]),
        ];

        for (mode, expected_flags) in permission_modes {
            let expected_flags = expected_flags
                .into_iter()
                .map(OsString::from)
                .collect::<Vec<_>>();
            assert_eq!(
                permission_flags(AgentKind::Kimi, mode)
                    .expect("Kimi permission flags should build")
                    .cli_args(),
                expected_flags
            );

            for resume in [None, Some("kimi-resume")] {
                let command = turn_command(
                    Path::new("/golden/kimi"),
                    Path::new("/tmp"),
                    &[
                        (OsString::from("KIMI_GOLDEN"), OsString::from("fixed")),
                        (
                            OsString::from("HEADROOM_PROJECT"),
                            OsString::from("golden-session"),
                        ),
                    ],
                    &[
                        OsString::from("--agent-file"),
                        OsString::from("/golden/agent.md"),
                    ],
                    mode,
                    "hello Kimi",
                    resume,
                )
                .expect("Kimi turn launch should build");
                let mut expected_turn = vec![
                    OsString::from("--agent-file"),
                    OsString::from("/golden/agent.md"),
                ];
                expected_turn.extend(expected_flags.clone());
                if let Some(session_id) = resume {
                    expected_turn.extend([OsString::from("--resume"), OsString::from(session_id)]);
                }
                expected_turn.extend([
                    OsString::from("--prompt"),
                    OsString::from("hello Kimi"),
                    OsString::from("--output-format"),
                    OsString::from("stream-json"),
                ]);
                assert_eq!(command.get_program(), Path::new("/golden/kimi"));
                assert_eq!(
                    command.get_args().map(OsString::from).collect::<Vec<_>>(),
                    expected_turn
                );
                assert_eq!(command_env(&command, "KIMI_GOLDEN"), Some("fixed"));
                assert_eq!(
                    command_env(&command, "HEADROOM_PROJECT"),
                    Some("golden-session")
                );

                let terminal = interactive_launch_spec_with_mcp_args(
                    PathBuf::from("/golden/kimi"),
                    &lane,
                    mode,
                    resume,
                    Some(9123),
                    "golden-session",
                    Some(Path::new("/golden/codex-home")),
                    vec![
                        OsString::from("--agent-file"),
                        OsString::from("/golden/agent.md"),
                    ],
                )
                .expect("Kimi interactive launch should build");
                let mut expected_terminal = vec![
                    OsString::from("--agent-file"),
                    OsString::from("/golden/agent.md"),
                ];
                expected_terminal.extend(expected_flags.clone());
                if let Some(session_id) = resume {
                    expected_terminal
                        .extend([OsString::from("--resume"), OsString::from(session_id)]);
                }
                assert_eq!(terminal.program, PathBuf::from("/golden/kimi"));
                assert_eq!(terminal.args, expected_terminal);
                assert_eq!(
                    terminal.env,
                    vec![
                        (
                            OsString::from("KIMI_MODEL_NAME"),
                            OsString::from(KIMI_MODEL_NAME),
                        ),
                        (
                            OsString::from("KIMI_BASE_URL"),
                            OsString::from("http://127.0.0.1:9123/p/golden-session/v1",),
                        ),
                        (
                            OsString::from("HEADROOM_PROJECT"),
                            OsString::from("golden-session"),
                        ),
                        (
                            OsString::from("CODEX_HOME"),
                            OsString::from("/golden/codex-home"),
                        ),
                    ]
                );
                assert!(terminal.env_remove.is_empty());
            }
        }

        assert_eq!(
            login_args_from_help("Commands:\n  login Sign in\n"),
            ["login"]
        );
        assert!(login_args_from_help("Usage: kimi [OPTIONS]").is_empty());
        let mut parser = KimiEventParser::default();
        assert_eq!(
            parser.parse_line(r#"{"role":"assistant","content":"hello world"}"#),
            vec![AgentEvent::TextDelta {
                text: "hello world".to_string(),
            }]
        );
    }

    #[test]
    fn adapter_login_launch_spec_is_exact() {
        assert_eq!(
            login_launch_spec_with_program_and_help(
                PathBuf::from("/golden/kimi"),
                "Commands:\n  login Sign in\n",
            ),
            LaunchSpec {
                program: PathBuf::from("/golden/kimi"),
                args: vec![OsString::from("login")],
                env: Vec::new(),
                env_remove: Vec::new(),
            }
        );
        assert_eq!(
            login_launch_spec_with_program_and_help(
                PathBuf::from("/golden/kimi"),
                "Usage: kimi [OPTIONS]",
            ),
            LaunchSpec {
                program: PathBuf::from("/golden/kimi"),
                args: Vec::new(),
                env: Vec::new(),
                env_remove: Vec::new(),
            }
        );
    }
}
