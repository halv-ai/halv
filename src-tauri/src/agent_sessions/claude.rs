use super::{
    adapter::{
        AdapterProcess, AdapterState, AgentAdapter, AgentProbe, CanonicalMode,
        InteractiveLaunchContext, LaunchContext, ParsedTurn, PermissionFlags, TaskRoutingContext,
        TurnKind,
    },
    apply_savings_stack_environment, canonical_permission_mode,
    cli_binary::{self, HostOs},
    command_environment,
    install::{self, ManagedAgent},
    session_files, AgentEvent, AgentKind, LaunchSpec, SessionLane, TurnUsage,
};
use crate::{
    host_env,
    ponytail::{self, PonytailMode},
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    cmp::Ordering,
    collections::{HashMap, HashSet, VecDeque},
    env,
    ffi::OsString,
    fs::{self, File},
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::{SystemTime, UNIX_EPOCH},
};

const CLAUDE_BINARY_NAME: &str = "claude";
const ANTHROPIC_DEFAULT_BASE_URL: &str = "https://api.anthropic.com";
const ZAI_ANTHROPIC_BASE_URL: &str = "https://api.z.ai/api/anthropic";
const ZAI_DEFAULT_SONNET_MODEL: &str = "glm-5.2";
const ZAI_DEFAULT_OPUS_MODEL: &str = "glm-5.2";
const ZAI_DEFAULT_HAIKU_MODEL: &str = "glm-4.7";
const GROK_ANTHROPIC_BASE_URL: &str = "https://api.x.ai";
const GROK_DEFAULT_SONNET_MODEL: &str = "grok-4.6";
const GROK_DEFAULT_OPUS_MODEL: &str = "grok-4.6";
const GROK_DEFAULT_HAIKU_MODEL: &str = "grok-build-0.1";

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaudeUsageWindow {
    pub label: String,
    pub used_percent: Option<f64>,
    pub resets_at: Option<String>,
}

#[derive(Deserialize)]
struct ClaudeLocalState {
    #[serde(rename = "cachedUsageUtilization")]
    cached_usage_utilization: Option<ClaudeCachedUsage>,
}

#[derive(Deserialize)]
struct ClaudeCachedUsage {
    utilization: ClaudeUsageSnapshot,
}

#[derive(Deserialize)]
struct ClaudeUsageSnapshot {
    five_hour: Option<ClaudeUsageValue>,
    seven_day: Option<ClaudeUsageValue>,
}

#[derive(Deserialize)]
struct ClaudeUsageValue {
    utilization: Option<f64>,
    resets_at: Option<String>,
}

const OAUTH_USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";
const USAGE_CACHE_TTL: std::time::Duration = std::time::Duration::from_secs(60);
/// The cache key names the account, so a wallet switch never shows the
/// usage of the previous account.
type UsageCacheEntry = (String, std::time::Instant, Vec<ClaudeUsageWindow>);
static USAGE_CACHE: std::sync::Mutex<Option<UsageCacheEntry>> = std::sync::Mutex::new(None);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CredentialSource {
    Keychain,
    File,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum CredentialUnavailableReason {
    KeychainMissing,
    #[cfg_attr(all(not(target_os = "macos"), not(test)), allow(dead_code))]
    KeychainRequestFailed,
    KeychainParseFailed,
    KeychainNoToken,
    FileMissing,
    FileReadFailed,
    FileParseFailed,
    FileNoToken,
    TokenExpired,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CredentialUnavailable {
    sources_tried: Vec<CredentialSource>,
    reason: CredentialUnavailableReason,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Credential {
    access_token: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaudeUsageWindowsResult {
    pub windows: Vec<ClaudeUsageWindow>,
    pub unavailable_reason: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UsageUnavailable {
    Credential(CredentialUnavailableReason),
    Request,
    Http(u16),
    ResponseParse,
}

impl UsageUnavailable {
    fn user_reason(self) -> String {
        match self {
            Self::Credential(_) => "no readable Claude credential".to_string(),
            Self::Request => "usage request failed".to_string(),
            Self::Http(status) => format!("usage request failed (HTTP {status})"),
            Self::ResponseParse => "usage response was invalid".to_string(),
        }
    }
}

/// Reads the keychain item of the active account. A managed wallet account
/// and a set `CLAUDE_CONFIG_DIR` use the folder-scoped item name.
fn read_keychain_credentials() -> Result<Option<String>, CredentialUnavailableReason> {
    for service in crate::wallet::claude_active_keychain_services() {
        if let Some(contents) = crate::wallet::read_keychain(&service)
            .map_err(|()| CredentialUnavailableReason::KeychainRequestFailed)?
        {
            return Ok(Some(contents));
        }
    }
    Ok(None)
}

/// The config folder of the active account: a wallet account folder, else
/// `CLAUDE_CONFIG_DIR`, else `~/.claude`.
fn claude_config_directory() -> Option<PathBuf> {
    crate::wallet::claude_config_dir()
}

fn read_file_credentials(path: &Path) -> Result<Option<String>, CredentialUnavailableReason> {
    match fs::read_to_string(path) {
        Ok(contents) => Ok(Some(contents)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(_) => Err(CredentialUnavailableReason::FileReadFailed),
    }
}

fn credential_from_contents(
    contents: &str,
    source: CredentialSource,
    now_millis: u128,
) -> Result<Credential, CredentialUnavailableReason> {
    let parse_reason = match source {
        CredentialSource::Keychain => CredentialUnavailableReason::KeychainParseFailed,
        CredentialSource::File => CredentialUnavailableReason::FileParseFailed,
    };
    let no_token_reason = match source {
        CredentialSource::Keychain => CredentialUnavailableReason::KeychainNoToken,
        CredentialSource::File => CredentialUnavailableReason::FileNoToken,
    };
    let credentials = serde_json::from_str::<Value>(contents).map_err(|_| parse_reason)?;
    let oauth = credentials.get("claudeAiOauth").ok_or(no_token_reason)?;
    let access_token = oauth
        .get("accessToken")
        .and_then(Value::as_str)
        .ok_or(no_token_reason)?
        .trim();
    let access_token = (!access_token.is_empty())
        .then(|| access_token.to_string())
        .ok_or(no_token_reason)?;
    if let Some(expires_at) = oauth.get("expiresAt") {
        let expires_at = expires_at
            .as_u64()
            .map(u128::from)
            .or_else(|| expires_at.as_str()?.parse::<u128>().ok())
            .ok_or(parse_reason)?;
        if expires_at <= now_millis {
            return Err(CredentialUnavailableReason::TokenExpired);
        }
    }
    Ok(Credential { access_token })
}

fn load_credential_with<K, F>(
    config_dir: Option<&Path>,
    now_millis: u128,
    read_keychain: K,
    read_file: F,
) -> Result<Credential, CredentialUnavailable>
where
    K: FnOnce() -> Result<Option<String>, CredentialUnavailableReason>,
    F: FnOnce(&Path) -> Result<Option<String>, CredentialUnavailableReason>,
{
    let mut sources_tried = vec![CredentialSource::Keychain];
    let keychain_reason = match read_keychain() {
        Ok(Some(contents)) => {
            match credential_from_contents(&contents, CredentialSource::Keychain, now_millis) {
                Ok(credential) => return Ok(credential),
                Err(error) => error,
            }
        }
        Ok(None) => CredentialUnavailableReason::KeychainMissing,
        Err(error) => error,
    };

    sources_tried.push(CredentialSource::File);
    let reason = if let Some(path) = config_dir.map(|directory| directory.join(".credentials.json"))
    {
        match read_file(&path) {
            Ok(Some(contents)) => {
                match credential_from_contents(&contents, CredentialSource::File, now_millis) {
                    Ok(credential) => return Ok(credential),
                    Err(error) => error,
                }
            }
            Ok(None) if keychain_reason != CredentialUnavailableReason::KeychainMissing => {
                keychain_reason
            }
            Ok(None) => CredentialUnavailableReason::FileMissing,
            Err(error) => error,
        }
    } else {
        CredentialUnavailableReason::FileMissing
    };
    Err(CredentialUnavailable {
        sources_tried,
        reason,
    })
}

fn current_unix_millis() -> Option<u128> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_millis())
}

fn load_credential() -> Result<Credential, CredentialUnavailable> {
    let now_millis = current_unix_millis().unwrap_or_default();
    let config_dir = claude_config_directory();
    load_credential_with(
        config_dir.as_deref(),
        now_millis,
        read_keychain_credentials,
        read_file_credentials,
    )
}

fn stored_credential_status() -> Option<bool> {
    match load_credential() {
        Ok(_) => Some(true),
        Err(error) if error.reason == CredentialUnavailableReason::TokenExpired => Some(false),
        Err(_) => None,
    }
}

fn claude_local_state_path() -> Option<PathBuf> {
    crate::wallet::claude_state_file()
}

fn top_level_json_keys(value: &Value) -> Vec<&str> {
    let mut keys = value
        .as_object()
        .into_iter()
        .flat_map(|object| object.keys().map(String::as_str))
        .collect::<Vec<_>>();
    keys.sort_unstable();
    keys
}

fn log_usage_failure(category: &str, status: Option<u16>, payload: Option<&Value>) {
    let status = status
        .map(|value| value.to_string())
        .unwrap_or_else(|| "unavailable".to_string());
    let keys = payload.map(top_level_json_keys).unwrap_or_default();
    eprintln!(
        "Claude usage fetch failed: category={category}; status={status}; top_level_json_keys={keys:?}"
    );
}

// Claude Code uses this endpoint for usage data. Local data is a fallback.
// Claude updates the local data only after a turn.
pub(super) async fn fetch_usage_windows_live() -> ClaudeUsageWindowsResult {
    let account_key = crate::wallet::usage_cache_key(crate::wallet::WalletProvider::Claude);
    if let Some((cached_key, cached_at, windows)) = USAGE_CACHE
        .lock()
        .expect("claude usage cache lock poisoned")
        .clone()
    {
        if cached_key == account_key && cached_at.elapsed() < USAGE_CACHE_TTL {
            return ClaudeUsageWindowsResult {
                windows,
                unavailable_reason: None,
            };
        }
    }
    match fetch_usage_windows_uncached().await {
        Ok(windows) => {
            *USAGE_CACHE
                .lock()
                .expect("claude usage cache lock poisoned") =
                Some((account_key, std::time::Instant::now(), windows.clone()));
            ClaudeUsageWindowsResult {
                windows,
                unavailable_reason: None,
            }
        }
        Err(error) => ClaudeUsageWindowsResult {
            windows: usage_windows(),
            unavailable_reason: Some(error.user_reason()),
        },
    }
}

async fn fetch_usage_windows_uncached() -> Result<Vec<ClaudeUsageWindow>, UsageUnavailable> {
    let access_token = load_credential().map_err(|error| {
        let _sources_tried = error.sources_tried;
        UsageUnavailable::Credential(error.reason)
    })?;
    let response = reqwest::Client::new()
        .get(OAUTH_USAGE_URL)
        .bearer_auth(access_token.access_token)
        .header("anthropic-beta", "oauth-2025-04-20")
        .header("User-Agent", "claude-code/2.1.0")
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await
        .map_err(|_| {
            log_usage_failure("request", None, None);
            UsageUnavailable::Request
        })?;
    let status = response.status();
    let payload = response.text().await.map_err(|_| {
        log_usage_failure("response_read", Some(status.as_u16()), None);
        UsageUnavailable::ResponseParse
    })?;
    let payload = serde_json::from_str::<Value>(&payload);
    if !status.is_success() {
        log_usage_failure("http", Some(status.as_u16()), payload.as_ref().ok());
        return Err(UsageUnavailable::Http(status.as_u16()));
    }
    let payload = payload.map_err(|_| {
        log_usage_failure("json", Some(status.as_u16()), None);
        UsageUnavailable::ResponseParse
    })?;
    let window = |key: &str, label: &str| {
        let value = payload.get(key)?.as_object()?;
        Some(ClaudeUsageWindow {
            label: label.to_string(),
            used_percent: value
                .get("utilization")
                .and_then(Value::as_f64)
                .filter(|percent| percent.is_finite())
                .map(|percent| percent.clamp(0.0, 100.0)),
            resets_at: value
                .get("resets_at")
                .and_then(Value::as_str)
                .map(ToString::to_string),
        })
    };
    let windows: Vec<ClaudeUsageWindow> = [("five_hour", "5h"), ("seven_day", "7d")]
        .into_iter()
        .filter_map(|(key, label)| window(key, label))
        .collect();
    if windows.is_empty() {
        log_usage_failure("json_shape", Some(status.as_u16()), Some(&payload));
        return Err(UsageUnavailable::ResponseParse);
    }
    Ok(windows)
}

pub(super) fn usage_windows() -> Vec<ClaudeUsageWindow> {
    let Some(path) = claude_local_state_path() else {
        return unknown_usage_windows();
    };
    let Ok(contents) = fs::read_to_string(path) else {
        return unknown_usage_windows();
    };
    parse_usage_windows(&contents)
}

fn parse_usage_windows(contents: &str) -> Vec<ClaudeUsageWindow> {
    let snapshot = serde_json::from_str::<ClaudeLocalState>(contents)
        .ok()
        .and_then(|state| state.cached_usage_utilization)
        .map(|cached| cached.utilization);
    vec![
        usage_window(
            "5h",
            snapshot.as_ref().and_then(|value| value.five_hour.as_ref()),
        ),
        usage_window(
            "7d",
            snapshot.as_ref().and_then(|value| value.seven_day.as_ref()),
        ),
    ]
}

fn unknown_usage_windows() -> Vec<ClaudeUsageWindow> {
    vec![usage_window("5h", None), usage_window("7d", None)]
}

fn usage_window(label: &str, value: Option<&ClaudeUsageValue>) -> ClaudeUsageWindow {
    ClaudeUsageWindow {
        label: label.to_string(),
        used_percent: value
            .and_then(|window| window.utilization)
            .filter(|percent| percent.is_finite())
            .map(|percent| percent.clamp(0.0, 100.0)),
        resets_at: value
            .and_then(|window| window.resets_at.as_deref())
            .map(str::trim)
            .filter(|reset| !reset.is_empty())
            .map(ToString::to_string),
    }
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
                    format!("claude --version exited with {}", output.status)
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

pub(super) struct ClaudeAdapterState {
    parser: ClaudeEventParser,
    usage_before: Option<TurnUsage>,
}

pub(super) struct ClaudeAdapter {
    kind: AgentKind,
}

pub(super) static CLAUDE_ADAPTER: ClaudeAdapter = ClaudeAdapter {
    kind: AgentKind::Claude,
};
pub(super) static GROK_ADAPTER: ClaudeAdapter = ClaudeAdapter {
    kind: AgentKind::Grok,
};
pub(super) static ZAI_ADAPTER: ClaudeAdapter = ClaudeAdapter {
    kind: AgentKind::Zai,
};

fn permission_flags(
    agent_kind: AgentKind,
    permission_mode: &str,
) -> Result<PermissionFlags, String> {
    ClaudeAdapter { kind: agent_kind }.permission_flags(canonical_permission_mode(permission_mode)?)
}

impl AgentAdapter for ClaudeAdapter {
    fn kind(&self) -> AgentKind {
        self.kind
    }

    fn probe(&self) -> AgentProbe {
        probe()
    }

    fn binary(&self) -> Option<PathBuf> {
        find_binary()
    }

    fn permission_flags(&self, mode: CanonicalMode) -> Result<PermissionFlags, String> {
        let args = match (self.kind, mode) {
            (_, CanonicalMode::ReadOnly) => vec!["--permission-mode", "plan"],
            (AgentKind::Claude, CanonicalMode::Ask) => vec!["--permission-mode", "default"],
            (AgentKind::Claude, CanonicalMode::OnRequest) => vec!["--permission-mode", "auto"],
            (AgentKind::Grok | AgentKind::Zai, CanonicalMode::Ask | CanonicalMode::OnRequest) => {
                vec!["--permission-mode", "default"]
            }
            (_, CanonicalMode::OnFailure) => vec!["--permission-mode", "acceptEdits"],
            (_, CanonicalMode::Never) => vec!["--permission-mode", "dontAsk"],
            (_, CanonicalMode::SkipAll) => vec!["--dangerously-skip-permissions"],
            (
                AgentKind::Antigravity | AgentKind::Codex | AgentKind::Gemini | AgentKind::Kimi,
                _,
            ) => return Err("Claude adapter received an incompatible agent kind".to_string()),
        };
        Ok(PermissionFlags {
            args: args.into_iter().map(OsString::from).collect(),
            ..PermissionFlags::empty()
        })
    }

    fn login_launch_spec(&self) -> Result<LaunchSpec, String> {
        match self.kind {
            AgentKind::Claude => login_launch_spec(),
            AgentKind::Zai => Err(
                "GLM does not use a CLI login. Add a GLM Coding Plan API key profile.".to_string(),
            ),
            AgentKind::Grok => {
                Err("Grok does not use a CLI login. Add a Grok (xAI) API key profile.".to_string())
            }
            AgentKind::Antigravity | AgentKind::Codex | AgentKind::Gemini | AgentKind::Kimi => {
                Err("Claude adapter received an incompatible agent kind".to_string())
            }
        }
    }

    fn interactive_launch_spec(
        &self,
        ctx: &InteractiveLaunchContext<'_>,
    ) -> Result<LaunchSpec, String> {
        interactive_launch_spec(
            self.kind,
            ctx.cwd,
            ctx.lane,
            ctx.permission_mode.as_str(),
            ctx.resume_cli_session_id,
            ctx.assigned_cli_session_id,
            ctx.proxy_port,
            ctx.project,
            ctx.savings_database_path,
            ctx.nested_codex_home,
            ctx.ponytail_mode,
            ctx.claude_subscription_proxy,
            ctx.routing(),
            ctx.claude_ultracode,
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
            self.kind,
            ctx.cwd,
            ctx.lane,
            ctx.permission_mode.as_str(),
            ctx.resume_cli_session_id,
            ctx.proxy_port,
            ctx.project,
            ctx.savings_database_path,
            ctx.model,
            (self.kind == AgentKind::Claude)
                .then_some(ctx.effort)
                .flatten(),
            ctx.nested_codex_home,
            ctx.ponytail_mode,
            TaskRoutingContext {
                coordinator_model: ctx.model,
                ..TaskRoutingContext::default()
            },
            ctx.extension_manager,
        )?;
        AdapterProcess::persistent(child, "Claude")
    }

    fn new_state(
        &self,
        _resumed: bool,
        _permission_mode: CanonicalMode,
        _started_at: std::time::Instant,
        _initializing: bool,
    ) -> AdapterState {
        AdapterState::Claude(ClaudeAdapterState {
            parser: ClaudeEventParser::default(),
            usage_before: None,
        })
    }

    fn parse_line(&self, state: &mut AdapterState, line: &str) -> ParsedTurn {
        let AdapterState::Claude(state) = state else {
            return ParsedTurn {
                events: vec![AgentEvent::ProtocolError {
                    message: "Claude adapter received incompatible parser state".to_string(),
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
        text: &str,
        _turn: TurnKind<'_>,
    ) -> Result<Option<String>, String> {
        user_message_line(text).map(Some)
    }

    fn emits_turn_started(&self) -> bool {
        true
    }

    fn uses_rtk_instruction(&self) -> bool {
        false
    }

    fn prepare_usage(&self, state: &mut AdapterState, cli_session_id: Option<&str>) {
        if self.kind != AgentKind::Claude {
            return;
        }
        let AdapterState::Claude(state) = state else {
            return;
        };
        state.usage_before = Some(
            cli_session_id
                .and_then(transcript_usage)
                .unwrap_or_default(),
        );
    }

    fn measured_usage(
        &self,
        state: &mut AdapterState,
        cli_session_id: Option<&str>,
    ) -> Option<TurnUsage> {
        if self.kind != AgentKind::Claude {
            return None;
        }
        let AdapterState::Claude(state) = state else {
            return None;
        };
        let before = state.usage_before.take()?;
        let after = transcript_usage(cli_session_id?)?;
        Some(super::turn_usage_delta(after, before))
    }
}

fn login_launch_spec() -> Result<LaunchSpec, String> {
    let program = find_binary().ok_or_else(|| {
        "Claude Code was not found in PATH or a supported install location".to_string()
    })?;
    Ok(login_launch_spec_with_program(program))
}

fn login_launch_spec_with_program(program: PathBuf) -> LaunchSpec {
    LaunchSpec {
        program,
        args: vec![OsString::from("auth"), OsString::from("login")],
        env: Vec::new(),
        env_remove: Vec::new(),
    }
}

pub(super) fn login_status() -> Option<bool> {
    // The status follows the active wallet account. A missing account
    // folder means no usable login.
    let Ok(account_environment) =
        crate::wallet::probe_environment(crate::wallet::WalletProvider::Claude)
    else {
        return Some(false);
    };
    let Some(program) = find_binary() else {
        return stored_credential_status();
    };
    let mut command = Command::new(program);
    command
        .args(["auth", "status", "--text"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    account_environment.apply(&mut command);
    host_env::apply_to_command(&mut command);
    crate::process::hide_console(&mut command);
    match command.status() {
        Ok(status) if status.success() => Some(true),
        Ok(status) if status.code() == Some(1) => Some(false),
        Ok(_) | Err(_) => stored_credential_status(),
    }
}

#[allow(clippy::too_many_arguments)]
fn spawn(
    agent_kind: AgentKind,
    cwd: &Path,
    lane: &SessionLane,
    permission_mode: &str,
    resume_cli_session_id: Option<&str>,
    proxy_port: Option<u16>,
    project: &str,
    savings_database_path: &Path,
    model: Option<&str>,
    effort: Option<&str>,
    nested_codex_home: Option<&Path>,
    ponytail_mode: PonytailMode,
    routing: TaskRoutingContext<'_>,
    extension_manager: &crate::extensions::ExtensionManager,
) -> Result<Child, String> {
    let binary = find_binary().ok_or_else(|| {
        "Claude Code was not found in PATH or a supported install location".to_string()
    })?;
    let mut command = gui_command_with_program_and_crux(
        agent_kind,
        binary,
        cwd,
        lane,
        permission_mode,
        resume_cli_session_id,
        proxy_port,
        project,
        savings_database_path,
        model,
        effort,
        nested_codex_home,
        ponytail_mode,
        routing,
        crate::extensions::crux_enabled_for_project(Some(extension_manager), Some(cwd)),
    )?;
    command
        .spawn()
        .map_err(|error| format!("failed to start Claude Code: {error}"))
}

#[allow(clippy::too_many_arguments)]
fn gui_command_with_program_and_crux(
    agent_kind: AgentKind,
    binary: PathBuf,
    cwd: &Path,
    lane: &SessionLane,
    permission_mode: &str,
    resume_cli_session_id: Option<&str>,
    proxy_port: Option<u16>,
    project: &str,
    savings_database_path: &Path,
    model: Option<&str>,
    effort: Option<&str>,
    nested_codex_home: Option<&Path>,
    ponytail_mode: PonytailMode,
    routing: TaskRoutingContext<'_>,
    crux_enabled: bool,
) -> Result<Command, String> {
    let permission_args = permission_flags(agent_kind, permission_mode)?.cli_args();
    let mut rtk_args = Vec::new();
    add_interactive_settings(
        &mut rtk_args,
        lane,
        project,
        crate::exec::bundled_rtk_binary_path().as_deref(),
        None,
        false,
    )?;
    let mut command = Command::new(binary);
    command
        .arg("--print")
        .arg("--input-format")
        .arg("stream-json")
        .arg("--output-format")
        .arg("stream-json")
        .arg("--replay-user-messages")
        .arg("--verbose")
        .arg("--mcp-config")
        .arg(agent_mcp_config(
            savings_database_path,
            project,
            routing.clone(),
            crux_enabled,
        )?)
        .args(rtk_args)
        .args(permission_args)
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    if let Some(cli_session_id) = resume_cli_session_id {
        command.arg("--resume").arg(cli_session_id);
    }
    apply_model_argument(&mut command, model);
    apply_effort_argument(&mut command, effort);
    if let Some(text) = appended_system_prompt(ponytail_mode, &routing) {
        command.arg("--append-system-prompt").arg(text);
    }

    apply_lane_environment(
        &mut command,
        lane,
        proxy_port,
        project,
        nested_codex_home,
        false,
    )?;
    apply_savings_stack_environment(&mut command, project);
    host_env::apply_to_command(&mut command);
    crate::process::hide_console(&mut command);
    Ok(command)
}

#[cfg(test)]
#[allow(clippy::too_many_arguments)]
fn gui_command_with_program(
    binary: PathBuf,
    cwd: &Path,
    lane: &SessionLane,
    permission_mode: &str,
    resume_cli_session_id: Option<&str>,
    proxy_port: Option<u16>,
    project: &str,
    savings_database_path: &Path,
    model: Option<&str>,
    effort: Option<&str>,
    nested_codex_home: Option<&Path>,
    ponytail_mode: PonytailMode,
) -> Result<Command, String> {
    gui_command_with_program_and_crux(
        AgentKind::Claude,
        binary,
        cwd,
        lane,
        permission_mode,
        resume_cli_session_id,
        proxy_port,
        project,
        savings_database_path,
        model,
        effort,
        nested_codex_home,
        ponytail_mode,
        TaskRoutingContext::default(),
        false,
    )
}

fn apply_model_argument(command: &mut Command, model: Option<&str>) {
    if let Some(model) = model.map(str::trim).filter(|value| !value.is_empty()) {
        command.arg("--model").arg(model);
    }
}

fn apply_effort_argument(command: &mut Command, effort: Option<&str>) {
    if let Some(effort) = effort.map(str::trim).filter(|value| !value.is_empty()) {
        command.arg("--effort").arg(effort);
    }
}

const SAVINGS_MCP_TOOL_TIMEOUT_MS: u64 = 120_000;

/// Claude Code keeps only the last `--append-system-prompt` value, so Halv
/// joins the Ponytail and the routing text into one value.
fn appended_system_prompt(
    ponytail_mode: PonytailMode,
    routing: &TaskRoutingContext<'_>,
) -> Option<String> {
    let parts = [ponytail_instructions(ponytail_mode), routing.instructions()]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
    (!parts.is_empty()).then(|| parts.join("\n\n"))
}

fn agent_mcp_config(
    database_path: &Path,
    session_id: &str,
    routing: TaskRoutingContext<'_>,
    crux_enabled: bool,
) -> Result<OsString, String> {
    let savings = crate::savings_mcp::launch_config(
        database_path,
        session_id,
        routing.coordinator_model,
        routing.conversation_id,
        routing.worker_lane,
        routing.tier_agents.clone(),
        routing.workflow_routing,
    )?;
    let mut servers = serde_json::Map::new();
    if crux_enabled {
        servers.insert(
            "crux".to_string(),
            json!({
                "type": "stdio",
                "command": "crux",
                "args": ["--profile", "slim"],
                "env": {
                    "PATH": crate::extensions::crux_path_environment(),
                    // MCP servers can start with a sanitized environment;
                    // the ledger path must be explicit.
                    "CRUX_SAVINGS_LEDGER": crate::exec::crux_session_ledger_path(session_id)
                        .to_string_lossy(),
                },
            }),
        );
    }
    servers.insert("halv-savings".to_string(), savings_server_config(savings));
    let config = json!({ "mcpServers": servers });
    serde_json::to_string(&config)
        .map(OsString::from)
        .map_err(|error| format!("failed to encode agent MCP configuration: {error}"))
}

fn savings_server_config(savings: crate::savings_mcp::SavingsMcpLaunchConfig) -> Value {
    let mut server = json!({
        "type": "stdio",
        "command": savings.command,
        "args": savings.args,
        // A per-call limit. A full route_tasks batch takes about 80 s
        // at worst (64 briefs, eight at a time, 10 s each).
        "timeout": SAVINGS_MCP_TOOL_TIMEOUT_MS,
    });
    // On Linux the server needs the D-Bus session bus to read the Halv
    // session token from the keychain.
    if !savings.env.is_empty() {
        server["env"] = json!(savings.env);
    }
    server
}

#[allow(clippy::too_many_arguments)]
fn interactive_launch_spec(
    agent_kind: AgentKind,
    cwd: &Path,
    lane: &SessionLane,
    permission_mode: &str,
    resume_cli_session_id: Option<&str>,
    assigned_cli_session_id: Option<&str>,
    proxy_port: Option<u16>,
    project: &str,
    savings_database_path: &Path,
    nested_codex_home: Option<&Path>,
    ponytail_mode: PonytailMode,
    claude_subscription_proxy: bool,
    routing: TaskRoutingContext<'_>,
    claude_ultracode: bool,
    extension_manager: &crate::extensions::ExtensionManager,
    agent_status: bool,
) -> Result<LaunchSpec, String> {
    let program = find_binary().ok_or_else(|| {
        "Claude Code was not found in PATH or supported install location".to_string()
    })?;
    let bundled_rtk = crate::exec::bundled_rtk_binary_path();
    let status_hooks = agent_status
        .then(|| {
            crate::agent_status::claude_status_hooks_for(&program, || {
                super::cached_claude_version(&program)
            })
        })
        .flatten();
    interactive_launch_spec_with_program_and_crux(
        agent_kind,
        program,
        cwd,
        lane,
        permission_mode,
        resume_cli_session_id,
        assigned_cli_session_id,
        proxy_port,
        project,
        savings_database_path,
        nested_codex_home,
        ponytail_mode,
        claude_subscription_proxy,
        routing,
        claude_ultracode,
        bundled_rtk.as_deref(),
        status_hooks.as_ref(),
        crate::extensions::crux_enabled_for_project(Some(extension_manager), Some(cwd)),
    )
}

#[allow(clippy::too_many_arguments)]
fn interactive_launch_spec_with_program_and_crux(
    agent_kind: AgentKind,
    program: PathBuf,
    cwd: &Path,
    lane: &SessionLane,
    permission_mode: &str,
    resume_cli_session_id: Option<&str>,
    assigned_cli_session_id: Option<&str>,
    proxy_port: Option<u16>,
    project: &str,
    savings_database_path: &Path,
    nested_codex_home: Option<&Path>,
    ponytail_mode: PonytailMode,
    claude_subscription_proxy: bool,
    routing: TaskRoutingContext<'_>,
    claude_ultracode: bool,
    bundled_rtk: Option<&Path>,
    status_hooks: Option<&crate::agent_status::ClaudeStatusHooks>,
    crux_enabled: bool,
) -> Result<LaunchSpec, String> {
    let routing = routing.for_lane(crate::task_routing::Lane::Claude);
    let mut args = vec![
        OsString::from("--mcp-config"),
        agent_mcp_config(
            savings_database_path,
            project,
            routing.clone(),
            crux_enabled,
        )?,
    ];
    add_interactive_settings(
        &mut args,
        lane,
        project,
        bundled_rtk,
        status_hooks,
        claude_ultracode,
    )?;
    args.extend(permission_flags(agent_kind, permission_mode)?.cli_args());
    if routing.enabled && canonical_permission_mode(permission_mode)? == CanonicalMode::Never {
        // `dontAsk` denies every tool that the user did not allow before.
        args.push(OsString::from("--allowedTools"));
        args.push(OsString::from("mcp__halv-savings"));
    }
    if let Some(text) = appended_system_prompt(ponytail_mode, &routing) {
        args.push(OsString::from("--append-system-prompt"));
        args.push(OsString::from(text));
    }
    if let Some(cli_session_id) = resume_cli_session_id {
        if session_file_exists(cwd, cli_session_id) {
            args.push(OsString::from("--resume"));
            args.push(OsString::from(cli_session_id));
        } else {
            // Claude never persisted this session (an assigned id whose pane
            // ran no turn). `--resume` exits 1 with "No conversation found";
            // a fresh launch under the same id loses nothing and keeps the
            // pane resumable.
            args.push(OsString::from("--session-id"));
            args.push(OsString::from(cli_session_id));
        }
    } else if let Some(cli_session_id) = assigned_cli_session_id {
        // A fresh interactive session gets a Halv-assigned session id, so the
        // saved terminal can resume it later with `--resume`.
        args.push(OsString::from("--session-id"));
        args.push(OsString::from(cli_session_id));
    }

    let mut environment_command = Command::new(&program);
    environment_command.current_dir(cwd);
    apply_lane_environment(
        &mut environment_command,
        lane,
        proxy_port,
        project,
        nested_codex_home,
        claude_subscription_proxy,
    )?;
    host_env::apply_to_command(&mut environment_command);
    let (env, env_remove) = command_environment(&environment_command);
    Ok(LaunchSpec {
        program,
        args,
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
    assigned_cli_session_id: Option<&str>,
    proxy_port: Option<u16>,
    project: &str,
    savings_database_path: &Path,
    nested_codex_home: Option<&Path>,
    ponytail_mode: PonytailMode,
    claude_subscription_proxy: bool,
    bundled_rtk: Option<&Path>,
) -> Result<LaunchSpec, String> {
    interactive_launch_spec_with_program_and_crux(
        AgentKind::Claude,
        program,
        cwd,
        lane,
        permission_mode,
        resume_cli_session_id,
        assigned_cli_session_id,
        proxy_port,
        project,
        savings_database_path,
        nested_codex_home,
        ponytail_mode,
        claude_subscription_proxy,
        TaskRoutingContext::default(),
        false,
        bundled_rtk,
        None,
        false,
    )
}

/// Writes the per-session Claude settings file and passes it with
/// `--settings`. The file holds the RTK Bash hook when the bundled RTK
/// exists, the Halv status hooks of a terminal pane, and `"ultracode": true`
/// when the user starts Claude panes in ultracode. It holds no other key: a
/// flag setting overrides the same key of the user and of Orca.
fn add_interactive_settings(
    args: &mut Vec<OsString>,
    lane: &SessionLane,
    session_id: &str,
    bundled_rtk: Option<&Path>,
    status_hooks: Option<&crate::agent_status::ClaudeStatusHooks>,
    ultracode: bool,
) -> Result<(), String> {
    if !matches!(
        lane,
        SessionLane::ClaudeSubscription
            | SessionLane::ClaudeApiKey { .. }
            | SessionLane::ZaiGlm { .. }
            | SessionLane::GrokApiKey { .. }
    ) {
        return Ok(());
    }
    if bundled_rtk.is_some_and(|bundled_rtk| !bundled_rtk.is_absolute()) {
        return Err("the bundled RTK hook path must be absolute".to_string());
    }
    // Ultracode needs an Anthropic model. GLM and Grok lanes never get it.
    let ultracode = ultracode
        && matches!(
            lane,
            SessionLane::ClaudeSubscription | SessionLane::ClaudeApiKey { .. }
        );
    if bundled_rtk.is_none() && status_hooks.is_none() && !ultracode {
        return Ok(());
    }

    let mut hooks = serde_json::Map::new();
    if let Some(bundled_rtk) = bundled_rtk {
        let hook_command = format!("{} hook claude", shell_command_path(bundled_rtk)?);
        hooks.insert(
            "PreToolUse".to_string(),
            json!([{
                "matcher": "Bash",
                "hooks": [{
                    "type": "command",
                    "command": hook_command,
                }],
            }]),
        );
    }
    for (event, group) in status_hooks.into_iter().flat_map(|hooks| hooks.groups()) {
        if let Value::Array(groups) = hooks
            .entry(event.to_string())
            .or_insert_with(|| Value::Array(Vec::new()))
        {
            groups.push(group);
        }
    }
    let mut settings = json!({ "hooks": hooks });
    if ultracode {
        settings["ultracode"] = Value::Bool(true);
    }
    let settings = serde_json::to_vec(&settings)
        .map_err(|error| format!("failed to encode the session Claude settings: {error}"))?;
    let settings_path =
        session_files::directory(session_id)?.join(session_files::CLAUDE_SETTINGS_FILE_NAME);
    crate::exec::apply_patch::atomic_write_file(&settings_path, &settings).map_err(|error| {
        format!(
            "failed to write the session Claude settings `{}`: {error}",
            settings_path.display()
        )
    })?;
    args.push(OsString::from("--settings"));
    args.push(settings_path.into_os_string());
    Ok(())
}

fn shell_command_path(path: &Path) -> Result<String, String> {
    let path = path
        .to_str()
        .ok_or_else(|| "the bundled RTK hook path must be valid UTF-8".to_string())?;
    if path.contains(['\0', '\r', '\n']) {
        return Err("the bundled RTK hook path contains an invalid character".to_string());
    }
    if path
        .chars()
        .all(|character| character.is_ascii_alphanumeric() || "/._-".contains(character))
    {
        return Ok(path.to_string());
    }
    #[cfg(windows)]
    return Ok(format!("\"{}\"", path.replace('"', "\\\"")));
    #[cfg(not(windows))]
    return Ok(format!("'{}'", path.replace('\'', "'\\''")));
}

fn ponytail_instructions(mode: PonytailMode) -> Option<String> {
    let instructions = ponytail::instructions(mode)?;
    (!claude_ponytail_plugin_active()).then_some(instructions)
}

fn claude_ponytail_plugin_active() -> bool {
    claude_config_directory().is_some_and(|directory| directory.join(".ponytail-active").exists())
}

fn apply_lane_environment(
    command: &mut Command,
    lane: &SessionLane,
    proxy_port: Option<u16>,
    project: &str,
    nested_codex_home: Option<&Path>,
    claude_subscription_proxy: bool,
) -> Result<(), String> {
    match lane {
        SessionLane::ClaudeSubscription => {
            // A new subscription session uses the active wallet accounts. The
            // nested Codex home already carries the Codex account.
            crate::wallet::session_environment(
                crate::wallet::WalletProvider::Claude,
                nested_codex_home.is_none(),
            )?
            .apply(command);
            if claude_subscription_proxy {
                let port = proxy_port.ok_or_else(|| {
                    "the Claude subscription lane requires a managed Headroom proxy".to_string()
                })?;
                let upstream = env::var("ANTHROPIC_BASE_URL")
                    .ok()
                    .map(|value| value.trim().to_string())
                    .filter(|value| !value.is_empty())
                    .unwrap_or_else(|| ANTHROPIC_DEFAULT_BASE_URL.to_string());
                command
                    .env(
                        "ANTHROPIC_BASE_URL",
                        format!("http://127.0.0.1:{port}/p/{project}"),
                    )
                    .env(
                        "ANTHROPIC_CUSTOM_HEADERS",
                        format!("X-Headroom-Base-Url: {upstream}"),
                    )
                    .env("API_TIMEOUT_MS", "3000000");
            }
        }
        SessionLane::ClaudeApiKey {
            credential,
            base_url,
        } => {
            let api_key = credential.api_key()?;
            if api_key.is_empty() {
                return Err("the Claude API key cannot be empty".to_string());
            }
            let upstream = base_url
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .unwrap_or(ANTHROPIC_DEFAULT_BASE_URL);
            command.env("ANTHROPIC_API_KEY", api_key);
            apply_anthropic_route(command, proxy_port, project, upstream);
        }
        SessionLane::CodexChatGpt { .. } => {
            return Err("the Codex ChatGPT lane cannot be used with Claude".to_string());
        }
        SessionLane::ZaiGlm {
            credential,
            model_overrides,
        } => {
            let api_key = credential.api_key()?;
            if api_key.is_empty() {
                return Err("the GLM Coding Plan API key cannot be empty".to_string());
            }
            command
                .env_remove("ANTHROPIC_API_KEY")
                .env("ANTHROPIC_AUTH_TOKEN", api_key);
            apply_anthropic_route(command, proxy_port, project, ZAI_ANTHROPIC_BASE_URL);
            command
                .env("API_TIMEOUT_MS", "3000000")
                .env(
                    "ANTHROPIC_DEFAULT_SONNET_MODEL",
                    model_override(
                        model_overrides
                            .as_ref()
                            .and_then(|values| values.sonnet.as_deref()),
                        ZAI_DEFAULT_SONNET_MODEL,
                    ),
                )
                .env(
                    "ANTHROPIC_DEFAULT_OPUS_MODEL",
                    model_override(
                        model_overrides
                            .as_ref()
                            .and_then(|values| values.opus.as_deref()),
                        ZAI_DEFAULT_OPUS_MODEL,
                    ),
                )
                .env(
                    "ANTHROPIC_DEFAULT_HAIKU_MODEL",
                    model_override(
                        model_overrides
                            .as_ref()
                            .and_then(|values| values.haiku.as_deref()),
                        ZAI_DEFAULT_HAIKU_MODEL,
                    ),
                );
        }
        SessionLane::GrokApiKey {
            credential,
            model_overrides,
        } => {
            let api_key = credential.api_key()?;
            if api_key.is_empty() {
                return Err("the Grok API key cannot be empty".to_string());
            }
            command
                .env_remove("ANTHROPIC_API_KEY")
                .env("ANTHROPIC_AUTH_TOKEN", api_key);
            apply_anthropic_route(command, proxy_port, project, GROK_ANTHROPIC_BASE_URL);
            command
                .env("API_TIMEOUT_MS", "3000000")
                .env(
                    "ANTHROPIC_DEFAULT_SONNET_MODEL",
                    model_override(
                        model_overrides
                            .as_ref()
                            .and_then(|values| values.sonnet.as_deref()),
                        GROK_DEFAULT_SONNET_MODEL,
                    ),
                )
                .env(
                    "ANTHROPIC_DEFAULT_OPUS_MODEL",
                    model_override(
                        model_overrides
                            .as_ref()
                            .and_then(|values| values.opus.as_deref()),
                        GROK_DEFAULT_OPUS_MODEL,
                    ),
                )
                .env(
                    "ANTHROPIC_DEFAULT_HAIKU_MODEL",
                    model_override(
                        model_overrides
                            .as_ref()
                            .and_then(|values| values.haiku.as_deref()),
                        GROK_DEFAULT_HAIKU_MODEL,
                    ),
                );
        }
        SessionLane::KimiApiKey { .. } | SessionLane::KimiSubscription => {
            return Err("a Kimi lane cannot be used with Claude".to_string());
        }
        SessionLane::Custom { env } => {
            for (name, value) in env {
                if name.is_empty() || name.contains('=') || name.contains('\0') {
                    return Err(format!("invalid custom environment variable name `{name}`"));
                }
                if value.contains('\0') {
                    return Err(format!("custom environment variable `{name}` contains NUL"));
                }
                command.env(name, value);
            }
        }
    }
    command.env("HEADROOM_PROJECT", project);
    if let Some(codex_home) = nested_codex_home {
        command.env("CODEX_HOME", codex_home);
    }
    Ok(())
}

/// Points an Anthropic-format lane at the session prefix of the Halv Engine.
/// No port means the engine is off, so the lane goes direct to `upstream`.
fn apply_anthropic_route(
    command: &mut Command,
    proxy_port: Option<u16>,
    project: &str,
    upstream: &str,
) {
    match proxy_port {
        Some(port) => {
            command
                .env(
                    "ANTHROPIC_BASE_URL",
                    format!("http://127.0.0.1:{port}/p/{project}"),
                )
                .env(
                    "ANTHROPIC_CUSTOM_HEADERS",
                    format!("X-Headroom-Base-Url: {upstream}"),
                );
        }
        None => {
            command.env("ANTHROPIC_BASE_URL", upstream);
        }
    }
}

fn model_override<'a>(value: Option<&'a str>, default: &'a str) -> &'a str {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(default)
}

fn user_message_line(text: &str) -> Result<String, String> {
    let message = serde_json::json!({
        "type": "user",
        "message": {
            "role": "user",
            "content": [{ "type": "text", "text": text }]
        }
    });
    serde_json::to_string(&message)
        .map_err(|error| format!("failed to encode Claude user message: {error}"))
}

#[derive(Default)]
struct ClaudeEventParser {
    tool_names: HashMap<String, String>,
    previous_total_cost_usd: Option<f64>,
}

impl ClaudeEventParser {
    fn parse_line(&mut self, line: &str) -> Vec<AgentEvent> {
        let value = match serde_json::from_str::<Value>(line) {
            Ok(value) => value,
            Err(error) => {
                return vec![AgentEvent::ProtocolError {
                    message: format!("invalid Claude stream JSON: {error}"),
                }];
            }
        };

        match value.get("type").and_then(Value::as_str) {
            Some("system") => self.parse_system(&value).into_iter().collect(),
            Some("assistant") => self.parse_assistant(&value),
            Some("user") => self.parse_user(&value),
            Some("result") => vec![self.parse_result(&value)],
            _ => Vec::new(),
        }
    }

    fn parse_system(&self, value: &Value) -> Option<AgentEvent> {
        if value.get("subtype").and_then(Value::as_str) != Some("init") {
            return None;
        }
        let cli_session_id = value.get("session_id")?.as_str()?.to_string();
        let model = value
            .get("model")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        Some(AgentEvent::SessionInit {
            cli_session_id,
            model,
        })
    }

    fn parse_assistant(&mut self, value: &Value) -> Vec<AgentEvent> {
        let Some(content) = value
            .get("message")
            .and_then(|message| message.get("content"))
            .and_then(Value::as_array)
        else {
            return Vec::new();
        };

        let mut events = Vec::new();
        for item in content {
            match item.get("type").and_then(Value::as_str) {
                Some("text") => {
                    if let Some(text) = item.get("text").and_then(Value::as_str) {
                        events.push(AgentEvent::TextDelta {
                            text: text.to_string(),
                        });
                    }
                }
                Some("tool_use") => {
                    let name = item
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or("unknown")
                        .to_string();
                    if let Some(id) = item.get("id").and_then(Value::as_str) {
                        self.tool_names.insert(id.to_string(), name.clone());
                    }
                    let summary_json = item
                        .get("input")
                        .map(Value::to_string)
                        .unwrap_or_else(|| "null".to_string());
                    events.push(AgentEvent::ToolCallStarted { name, summary_json });
                }
                _ => {}
            }
        }
        events
    }

    fn parse_user(&mut self, value: &Value) -> Vec<AgentEvent> {
        let Some(content) = value
            .get("message")
            .and_then(|message| message.get("content"))
            .and_then(Value::as_array)
        else {
            return Vec::new();
        };

        content
            .iter()
            .filter(|item| item.get("type").and_then(Value::as_str) == Some("tool_result"))
            .map(|item| {
                let name = item
                    .get("tool_use_id")
                    .and_then(Value::as_str)
                    .and_then(|id| self.tool_names.remove(id))
                    .unwrap_or_else(|| "unknown".to_string());
                AgentEvent::ToolCallCompleted { name }
            })
            .collect()
    }

    fn parse_result(&mut self, value: &Value) -> AgentEvent {
        if value.get("is_error").and_then(Value::as_bool) == Some(true) {
            let result = value
                .get("result")
                .and_then(Value::as_str)
                .unwrap_or("Claude Code returned an error");
            let message = if result.to_ascii_lowercase().contains("not logged in") {
                "Open Agent Sessions and sign in to Claude Code before sending.".to_string()
            } else {
                format!("Claude Code turn failed: {result}")
            };
            return AgentEvent::ProtocolError { message };
        }
        let usage = value.get("usage").unwrap_or(&Value::Null);
        let total_cost_usd = value
            .get("total_cost_usd")
            .and_then(Value::as_f64)
            .filter(|cost| cost.is_finite() && *cost >= 0.0)
            .map(|cost| {
                let turn_cost = self
                    .previous_total_cost_usd
                    .filter(|previous| cost >= *previous)
                    .map(|previous| cost - previous)
                    .unwrap_or(cost);
                self.previous_total_cost_usd = Some(cost);
                turn_cost
            });
        AgentEvent::TurnCompleted {
            usage: TurnUsage {
                input_tokens: usage
                    .get("input_tokens")
                    .and_then(Value::as_u64)
                    .unwrap_or(0),
                output_tokens: usage
                    .get("output_tokens")
                    .and_then(Value::as_u64)
                    .unwrap_or(0),
                cache_read_tokens: usage
                    .get("cache_read_input_tokens")
                    .and_then(Value::as_u64)
                    .unwrap_or(0),
                cache_creation_tokens: usage
                    .get("cache_creation_input_tokens")
                    .and_then(Value::as_u64)
                    .unwrap_or(0),
                total_cost_usd,
            },
        }
    }
}

pub(super) fn transcript_usage(cli_session_id: &str) -> Option<TurnUsage> {
    let home = home_directory()?;
    transcript_usage_in_home(&home, cli_session_id)
}

fn transcript_usage_in_home(home: &Path, cli_session_id: &str) -> Option<TurnUsage> {
    if cli_session_id.is_empty()
        || !cli_session_id
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
    {
        return None;
    }
    let projects = home.join(".claude").join("projects");
    let transcript_name = format!("{cli_session_id}.jsonl");
    let mut session_directory = None;
    let main_transcript = fs::read_dir(projects)
        .ok()?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .find_map(|project_directory| {
            let transcript = project_directory.join(&transcript_name);
            transcript.is_file().then(|| {
                session_directory = Some(project_directory.join(cli_session_id));
                transcript
            })
        })?;

    let mut paths = vec![main_transcript];
    if let Some(subagents) = session_directory.map(|path| path.join("subagents")) {
        paths.extend(
            fs::read_dir(subagents)
                .into_iter()
                .flatten()
                .filter_map(Result::ok)
                .map(|entry| entry.path())
                .filter(|path| path.extension().and_then(|value| value.to_str()) == Some("jsonl")),
        );
    }

    let mut usage = TurnUsage::default();
    let mut seen_message_ids = HashSet::new();
    for path in paths {
        let Ok(file) = File::open(path) else {
            continue;
        };
        for line in BufReader::new(file).lines().map_while(Result::ok) {
            let Ok(value) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            if value.get("type").and_then(Value::as_str) != Some("assistant") {
                continue;
            }
            let Some(message_usage) = value
                .get("message")
                .and_then(|message| message.get("usage"))
            else {
                continue;
            };
            // Claude Code writes one line per content block and repeats the
            // usage of the whole response on each line. Count a response once.
            let response_id = value
                .get("message")
                .and_then(|message| message.get("id"))
                .and_then(Value::as_str)
                .or_else(|| value.get("uuid").and_then(Value::as_str));
            if let Some(id) = response_id {
                if !seen_message_ids.insert(id.to_string()) {
                    continue;
                }
            }
            usage.input_tokens = usage.input_tokens.saturating_add(
                message_usage
                    .get("input_tokens")
                    .and_then(Value::as_u64)
                    .unwrap_or_default(),
            );
            usage.output_tokens = usage.output_tokens.saturating_add(
                message_usage
                    .get("output_tokens")
                    .and_then(Value::as_u64)
                    .unwrap_or_default(),
            );
            usage.cache_read_tokens = usage.cache_read_tokens.saturating_add(
                message_usage
                    .get("cache_read_input_tokens")
                    .and_then(Value::as_u64)
                    .unwrap_or_default(),
            );
            usage.cache_creation_tokens = usage.cache_creation_tokens.saturating_add(
                message_usage
                    .get("cache_creation_input_tokens")
                    .and_then(Value::as_u64)
                    .unwrap_or_default(),
            );
        }
    }
    Some(usage)
}

pub(super) fn find_binary() -> Option<PathBuf> {
    if let Some(binary) = install::managed_binary(ManagedAgent::Claude) {
        return Some(binary);
    }
    let home = home_directory();
    let mut seen = HashSet::new();
    if let Some(binary) = binary_candidates(env::var_os("PATH"), home.as_deref())
        .into_iter()
        .find(|candidate| seen.insert(candidate.clone()) && cli_binary::is_executable(candidate))
    {
        return Some(binary);
    }
    desktop_bundle_candidates(home.as_deref())
        .into_iter()
        .find(|candidate| seen.insert(candidate.clone()) && cli_binary::is_executable(candidate))
}

fn binary_candidates(path: Option<OsString>, home: Option<&Path>) -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(path) = path {
        for directory in env::split_paths(&path) {
            add_binary_candidates(&mut candidates, &directory);
        }
    }

    if let Some(home) = home {
        candidates.push(home.join(".claude/local/claude"));
        candidates.push(home.join(".npm-global/bin/claude"));
        candidates.push(home.join(".local/bin/claude"));
    }
    candidates.extend([
        PathBuf::from("/opt/homebrew/bin/claude"),
        PathBuf::from("/usr/local/bin/claude"),
        PathBuf::from("/usr/bin/claude"),
    ]);

    if let Some(home) = home {
        add_versioned_binary_candidates(
            &mut candidates,
            &home.join(".nvm/versions/node"),
            Path::new("bin/claude"),
        );
        candidates.push(home.join(".volta/bin/claude"));
        add_versioned_binary_candidates(
            &mut candidates,
            &home.join(".local/share/fnm/node-versions"),
            Path::new("installation/bin/claude"),
        );
        candidates.push(home.join(".local/share/mise/shims/claude"));
        candidates.push(home.join(".asdf/shims/claude"));
        candidates.push(home.join(".bun/bin/claude"));
        candidates.push(home.join("Library/pnpm/claude"));
    }

    candidates
}

fn desktop_bundle_candidates(home: Option<&Path>) -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    add_desktop_bundle_candidates(&mut candidates, Path::new("/Applications/Claude.app"));
    if let Some(home) = home {
        add_desktop_bundle_candidates(&mut candidates, &home.join("Applications/Claude.app"));
    }
    candidates
}

pub(super) fn add_versioned_binary_candidates(
    candidates: &mut Vec<PathBuf>,
    versions_directory: &Path,
    binary_suffix: &Path,
) {
    let Ok(entries) = fs::read_dir(versions_directory) else {
        return;
    };
    let mut versions = entries
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|file_type| file_type.is_dir()))
        .map(|entry| entry.path())
        .collect::<Vec<_>>();
    versions.sort_by(|left, right| compare_version_paths(right, left));
    candidates.extend(
        versions
            .into_iter()
            .map(|version| version.join(binary_suffix)),
    );
}

fn compare_version_paths(left: &Path, right: &Path) -> Ordering {
    let components = |path: &Path| {
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default()
            .split(|character: char| !character.is_ascii_digit())
            .filter(|component| !component.is_empty())
            .filter_map(|component| component.parse::<u64>().ok())
            .collect::<Vec<_>>()
    };
    components(left).cmp(&components(right)).then_with(|| {
        left.file_name()
            .unwrap_or_default()
            .cmp(right.file_name().unwrap_or_default())
    })
}

const CLAUDE_DESKTOP_MAX_DEPTH: usize = 6;

fn add_desktop_bundle_candidates(candidates: &mut Vec<PathBuf>, bundle: &Path) {
    let mut directories = VecDeque::from([(bundle.to_path_buf(), 0)]);
    while let Some((directory, depth)) = directories.pop_front() {
        if depth >= CLAUDE_DESKTOP_MAX_DEPTH {
            continue;
        }
        let Ok(entries) = fs::read_dir(directory) else {
            continue;
        };
        for entry in entries.filter_map(Result::ok) {
            let path = entry.path();
            let name_matches = entry
                .file_name()
                .to_str()
                .is_some_and(|name| name == "claude" || name == "claude-code");
            if name_matches && cli_binary::is_executable(&path) {
                candidates.push(path.clone());
            }
            if entry.file_type().is_ok_and(|file_type| file_type.is_dir()) {
                directories.push_back((path, depth + 1));
            }
        }
    }
}

fn add_binary_candidates(candidates: &mut Vec<PathBuf>, directory: &Path) {
    cli_binary::push_candidates(candidates, directory, CLAUDE_BINARY_NAME, HostOs::current());
}

// Claude stores sessions as
// <config dir>/projects/<cwd with '/' and '.' as '-'>/<session id>.jsonl.
// An unknown config dir returns true so the launch still attempts --resume.
fn session_file_exists(cwd: &Path, cli_session_id: &str) -> bool {
    let Some(project_directory) = transcript_project_directory(cwd) else {
        return true;
    };
    project_directory
        .join(format!("{cli_session_id}.jsonl"))
        .exists()
}

pub(super) fn transcript_projects_directory() -> Option<PathBuf> {
    env::var_os("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .or_else(|| home_directory().map(|home| home.join(".claude")))
        .map(|config_dir| config_dir.join("projects"))
}

pub(super) fn transcript_project_directory(cwd: &Path) -> Option<PathBuf> {
    transcript_projects_directory()
        .map(|projects_directory| transcript_project_directory_in(&projects_directory, cwd))
}

pub(super) fn transcript_project_directory_in(projects_directory: &Path, cwd: &Path) -> PathBuf {
    let slug: String = cwd
        .to_string_lossy()
        .chars()
        .map(|character| {
            if character == '/' || character == '\\' || character == '.' {
                '-'
            } else {
                character
            }
        })
        .collect();
    projects_directory.join(slug)
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
    fn parses_claude_local_plan_windows() {
        let windows = parse_usage_windows(
            r#"{
                "cachedUsageUtilization": {
                    "utilization": {
                        "five_hour": {
                            "utilization": 11,
                            "resets_at": "2026-08-17T14:20:00Z"
                        },
                        "seven_day": {
                            "utilization": 17.5,
                            "resets_at": "2026-08-21T23:00:00Z"
                        }
                    }
                }
            }"#,
        );

        assert_eq!(
            windows,
            vec![
                ClaudeUsageWindow {
                    label: "5h".to_string(),
                    used_percent: Some(11.0),
                    resets_at: Some("2026-08-17T14:20:00Z".to_string()),
                },
                ClaudeUsageWindow {
                    label: "7d".to_string(),
                    used_percent: Some(17.5),
                    resets_at: Some("2026-08-21T23:00:00Z".to_string()),
                },
            ],
        );
    }

    #[test]
    fn returns_unknown_claude_windows_without_usage_state() {
        assert_eq!(parse_usage_windows("{}"), unknown_usage_windows());
        assert_eq!(parse_usage_windows("not-json"), unknown_usage_windows());
    }

    #[test]
    fn usage_result_serializes_windows_with_a_safe_unavailable_reason() {
        let result = ClaudeUsageWindowsResult {
            windows: unknown_usage_windows(),
            unavailable_reason: Some(UsageUnavailable::Http(401).user_reason()),
        };

        let value = serde_json::to_value(result).expect("usage result should serialize");
        assert!(value["windows"].is_array());
        assert_eq!(
            value["unavailableReason"],
            "usage request failed (HTTP 401)"
        );
    }

    #[test]
    fn stored_credential_status_is_none_when_credentials_are_missing() {
        with_test_credentials(None, || {
            assert_eq!(stored_credential_status(), None);
        });
    }

    #[test]
    fn stored_credential_status_is_true_before_expiration() {
        let future = current_unix_millis() + 60_000;
        let credentials = json!({
            "claudeAiOauth": {
                "accessToken": "test-token",
                "expiresAt": future,
            }
        })
        .to_string();
        with_test_credentials(Some(&credentials), || {
            assert_eq!(stored_credential_status(), Some(true));
        });
    }

    #[test]
    fn stored_credential_status_is_false_after_expiration() {
        let expired = current_unix_millis().saturating_sub(60_000);
        let credentials = json!({
            "claudeAiOauth": {
                "accessToken": "test-token",
                "expiresAt": expired,
            }
        })
        .to_string();
        with_test_credentials(Some(&credentials), || {
            assert_eq!(stored_credential_status(), Some(false));
        });
    }

    #[test]
    fn stored_credential_status_is_true_without_expiration() {
        let credentials = json!({
            "claudeAiOauth": {
                "accessToken": "test-token",
            }
        })
        .to_string();
        with_test_credentials(Some(&credentials), || {
            assert_eq!(stored_credential_status(), Some(true));
        });
    }

    #[test]
    fn valid_file_credential_wins_after_stale_keychain_json() {
        let now = 10_000_u128;
        let keychain = json!({
            "claudeAiOauth": {
                "accessToken": "stale-token",
                "expiresAt": 9_999,
            }
        })
        .to_string();
        let file = json!({
            "claudeAiOauth": {
                "accessToken": "file-token",
                "expiresAt": 20_000,
            }
        })
        .to_string();
        let config_dir = Path::new("/custom/claude");

        let credential = load_credential_with(
            Some(config_dir),
            now,
            || Ok(Some(keychain)),
            |path| {
                assert_eq!(path, config_dir.join(".credentials.json"));
                Ok(Some(file))
            },
        )
        .expect("the valid file credential should win");

        assert_eq!(credential.access_token, "file-token");
    }

    #[test]
    fn valid_file_credential_wins_after_keychain_has_no_token() {
        let keychain = json!({
            "claudeAiOauth": {
                "accessToken": "   ",
            }
        })
        .to_string();
        let file = json!({
            "claudeAiOauth": {
                "accessToken": "file-token",
            }
        })
        .to_string();

        let credential = load_credential_with(
            Some(Path::new("/custom/claude")),
            100,
            || Ok(Some(keychain)),
            |_| Ok(Some(file)),
        )
        .expect("the valid file credential should win");

        assert_eq!(credential.access_token, "file-token");
    }

    #[test]
    fn credential_loader_honors_custom_claude_config_dir() {
        let _guard = super::super::CLAUDE_CONFIG_DIR_TEST_LOCK
            .lock()
            .expect("Claude config test lock poisoned");
        let config_dir = test_directory("custom-credential-directory");
        let credential_path = config_dir.join(".credentials.json");
        fs::write(
            &credential_path,
            json!({
                "claudeAiOauth": {
                    "accessToken": "custom-directory-token",
                }
            })
            .to_string(),
        )
        .expect("custom credential fixture should write");
        let previous = env::var_os("CLAUDE_CONFIG_DIR");
        env::set_var("CLAUDE_CONFIG_DIR", &config_dir);

        let credential = load_credential().expect("custom credential should load");

        match previous {
            Some(value) => env::set_var("CLAUDE_CONFIG_DIR", value),
            None => env::remove_var("CLAUDE_CONFIG_DIR"),
        }
        assert_eq!(credential.access_token, "custom-directory-token");
        let _ = fs::remove_dir_all(config_dir);
    }

    #[test]
    fn local_usage_fallback_honors_custom_claude_config_dir() {
        let _guard = super::super::CLAUDE_CONFIG_DIR_TEST_LOCK
            .lock()
            .expect("Claude config test lock poisoned");
        let config_dir = test_directory("custom-usage-directory");
        fs::write(
            config_dir.join(".claude.json"),
            r#"{
                "cachedUsageUtilization": {
                    "utilization": {
                        "five_hour": { "utilization": 23 }
                    }
                }
            }"#,
        )
        .expect("custom usage fixture should write");
        let previous = env::var_os("CLAUDE_CONFIG_DIR");
        env::set_var("CLAUDE_CONFIG_DIR", &config_dir);

        let windows = usage_windows();

        match previous {
            Some(value) => env::set_var("CLAUDE_CONFIG_DIR", value),
            None => env::remove_var("CLAUDE_CONFIG_DIR"),
        }
        assert_eq!(windows[0].used_percent, Some(23.0));
        let _ = fs::remove_dir_all(config_dir);
    }

    #[test]
    fn expired_file_credential_reports_token_expired() {
        let expired = json!({
            "claudeAiOauth": {
                "accessToken": "expired-token",
                "expiresAt": 99,
            }
        })
        .to_string();
        let error = load_credential_with(
            Some(Path::new("/custom/claude")),
            100,
            || Ok(None),
            |_| Ok(Some(expired)),
        )
        .expect_err("the expired credential should be unavailable");

        assert_eq!(
            error.sources_tried,
            vec![CredentialSource::Keychain, CredentialSource::File]
        );
        assert_eq!(error.reason, CredentialUnavailableReason::TokenExpired);
    }

    #[test]
    fn keychain_request_failure_is_retained_when_file_is_missing() {
        let error = load_credential_with(
            Some(Path::new("/custom/claude")),
            100,
            || Err(CredentialUnavailableReason::KeychainRequestFailed),
            |_| Ok(None),
        )
        .expect_err("missing credentials should remain unavailable");

        assert_eq!(
            error.reason,
            CredentialUnavailableReason::KeychainRequestFailed
        );
    }

    #[test]
    fn version_manager_candidates_use_the_newest_versions_first() {
        let home = test_directory("version-manager-candidates");
        let nvm_old = home.join(".nvm/versions/node/v9.10.0/bin/claude");
        let nvm_new = home.join(".nvm/versions/node/v22.1.0/bin/claude");
        let fnm_old = home.join(".local/share/fnm/node-versions/v18.2.0/installation/bin/claude");
        let fnm_new = home.join(".local/share/fnm/node-versions/v21.4.0/installation/bin/claude");
        let other_candidates = [
            home.join(".volta/bin/claude"),
            home.join(".local/share/mise/shims/claude"),
            home.join(".asdf/shims/claude"),
            home.join(".bun/bin/claude"),
            home.join("Library/pnpm/claude"),
        ];
        for path in [
            &nvm_old,
            &nvm_new,
            &fnm_old,
            &fnm_new,
            &other_candidates[0],
            &other_candidates[1],
            &other_candidates[2],
            &other_candidates[3],
            &other_candidates[4],
        ] {
            write_test_executable(path);
        }

        let candidates = binary_candidates(None, Some(&home));
        let position = |path: &Path| {
            candidates
                .iter()
                .position(|candidate| candidate == path)
                .expect("version manager candidate should exist")
        };
        assert!(position(&nvm_new) < position(&nvm_old));
        assert!(position(&fnm_new) < position(&fnm_old));
        for candidate in other_candidates {
            assert!(candidates.contains(&candidate));
        }

        let _ = fs::remove_dir_all(home);
    }

    #[test]
    fn desktop_bundle_search_is_bounded_and_accepts_supported_names() {
        let root = test_directory("desktop-bundle");
        let bundle = root.join("Claude.app");
        let claude = bundle.join("Contents/Resources/runtime/bin/claude");
        let claude_code = bundle.join("Contents/Resources/claude-code");
        let too_deep = bundle.join("a/b/c/d/e/f/claude");
        write_test_executable(&claude);
        write_test_executable(&claude_code);
        write_test_executable(&too_deep);
        write_test_executable(&bundle.join("Contents/Resources/runtime/bin/claude-helper"));

        let mut candidates = Vec::new();
        add_desktop_bundle_candidates(&mut candidates, &bundle);

        assert!(candidates.contains(&claude));
        assert!(candidates.contains(&claude_code));
        assert!(!candidates.contains(&too_deep));
        assert_eq!(candidates.len(), 2);

        let _ = fs::remove_dir_all(root);
    }

    fn with_test_credentials(contents: Option<&str>, assertion: impl FnOnce()) {
        let _guard = super::super::CLAUDE_CONFIG_DIR_TEST_LOCK
            .lock()
            .expect("Claude config test lock poisoned");
        let config_dir = test_directory("stored-credentials");
        if let Some(contents) = contents {
            fs::write(config_dir.join(".credentials.json"), contents)
                .expect("credentials fixture should write");
        }
        let previous = env::var_os("CLAUDE_CONFIG_DIR");
        env::set_var("CLAUDE_CONFIG_DIR", &config_dir);

        assertion();

        match previous {
            Some(value) => env::set_var("CLAUDE_CONFIG_DIR", value),
            None => env::remove_var("CLAUDE_CONFIG_DIR"),
        }
        let _ = fs::remove_dir_all(config_dir);
    }

    fn current_unix_millis() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should follow the Unix epoch")
            .as_millis() as u64
    }

    fn test_directory(name: &str) -> PathBuf {
        let directory =
            env::temp_dir().join(format!("halv-claude-{name}-{}", rand::random::<u64>()));
        fs::create_dir_all(&directory).expect("test directory should exist");
        directory
    }

    fn write_test_executable(path: &Path) {
        fs::create_dir_all(path.parent().expect("test path should have a parent"))
            .expect("test parent directory should exist");
        fs::write(path, "#!/bin/sh\n").expect("test executable should write");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut permissions = fs::metadata(path)
                .expect("test executable metadata should exist")
                .permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(path, permissions)
                .expect("test executable permissions should update");
        }
    }

    #[test]
    fn savings_server_config_passes_the_keychain_environment() {
        let launch = |env: std::collections::BTreeMap<String, String>| {
            crate::savings_mcp::SavingsMcpLaunchConfig {
                command: "/opt/Halv/halv".to_string(),
                args: vec!["--mcp-savings".to_string()],
                env,
            }
        };
        let server = savings_server_config(launch(std::collections::BTreeMap::from([(
            "DBUS_SESSION_BUS_ADDRESS".to_string(),
            "unix:path=/run/user/1000/bus".to_string(),
        )])));
        assert_eq!(
            server["env"],
            json!({ "DBUS_SESSION_BUS_ADDRESS": "unix:path=/run/user/1000/bus" })
        );
        assert_eq!(server["command"], "/opt/Halv/halv");
        assert_eq!(server["timeout"], SAVINGS_MCP_TOOL_TIMEOUT_MS);

        let server = savings_server_config(launch(Default::default()));
        assert!(server.get("env").is_none());
    }

    #[test]
    fn agent_mcp_config_registers_crux_and_halv_savings() {
        let config = agent_mcp_config(
            Path::new("/tmp/halv.sqlite3"),
            "session-123",
            TaskRoutingContext::default(),
            true,
        )
        .expect("Claude MCP configuration should build");
        let config: Value = serde_json::from_str(
            config
                .to_str()
                .expect("Claude MCP configuration should be UTF-8"),
        )
        .expect("Claude MCP configuration should be JSON");
        let crux = &config["mcpServers"]["crux"];
        assert_eq!(crux["type"], "stdio");
        assert_eq!(crux["command"], "crux");
        assert_eq!(crux["args"], json!(["--profile", "slim"]));
        assert!(crux["env"]["PATH"]
            .as_str()
            .is_some_and(|value| !value.is_empty()));
        let savings = &config["mcpServers"]["halv-savings"];
        assert_eq!(savings["type"], "stdio");
        assert!(savings["command"]
            .as_str()
            .is_some_and(|value| !value.is_empty()));
        let savings_args = savings["args"]
            .as_array()
            .expect("savings MCP arguments should be an array");
        assert_eq!(savings_args.first(), Some(&json!("--mcp-savings")));
        assert!(savings_args
            .windows(2)
            .any(|pair| pair == [json!("--session"), json!("session-123")]));
    }

    #[test]
    fn crux_gate_matrix_controls_claude_registration() {
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

        for (activation, census, builtin_enabled, has_root, expected) in cases {
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
            let config = agent_mcp_config(
                Path::new("/tmp/halv.sqlite3"),
                "session-123",
                TaskRoutingContext::default(),
                crux_enabled,
            )
            .expect("Claude MCP configuration should build");
            let config: Value = serde_json::from_str(
                config
                    .to_str()
                    .expect("Claude MCP configuration should be UTF-8"),
            )
            .expect("Claude MCP configuration should be JSON");
            let has_crux = config["mcpServers"]
                .as_object()
                .is_some_and(|servers| servers.contains_key("crux"));
            assert_eq!(has_crux, expected);
            assert!(config["mcpServers"]["halv-savings"].is_object());
        }
    }

    #[test]
    fn interactive_subscription_uses_session_rtk_hook_settings() {
        let root = test_directory("rtk-hook-settings");
        let rtk = root.join("rtk bundle/rtk's binary");
        write_test_executable(&rtk);
        let rtk = rtk
            .canonicalize()
            .expect("RTK fixture path should canonicalize");
        let user_settings = root.join("user/settings.json");
        fs::create_dir_all(
            user_settings
                .parent()
                .expect("user settings should have a parent"),
        )
        .expect("user settings directory should exist");
        fs::write(&user_settings, r#"{"theme":"dark"}"#)
            .expect("user settings fixture should write");
        let session_id = format!("session/test-{}", rand::random::<u64>());
        let mut args = Vec::new();

        add_interactive_settings(
            &mut args,
            &SessionLane::ClaudeSubscription,
            &session_id,
            Some(&rtk),
            None,
            false,
        )
        .expect("session settings should write");

        assert_eq!(args[0], "--settings");
        let settings_path = PathBuf::from(&args[1]);
        assert_eq!(
            settings_path.parent(),
            Some(
                session_files::directory(&session_id)
                    .expect("session directory should exist")
                    .as_path()
            )
        );
        assert!(!settings_path.starts_with(crate::exec::rtk_state_directory()));
        let settings: Value = serde_json::from_slice(
            &fs::read(&settings_path).expect("session settings should read"),
        )
        .expect("session settings should be JSON");
        assert_eq!(settings["hooks"]["PreToolUse"][0]["matcher"], "Bash");
        assert_eq!(
            settings["hooks"]["PreToolUse"][0]["hooks"][0]["type"],
            "command"
        );
        assert_eq!(
            settings["hooks"]["PreToolUse"][0]["hooks"][0]["command"],
            format!("{} hook claude", shell_command_path(&rtk).unwrap())
        );
        assert_eq!(
            fs::read_to_string(&user_settings).expect("user settings should remain readable"),
            r#"{"theme":"dark"}"#
        );

        session_files::remove(&session_id);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn interactive_claude_adapter_lanes_add_rtk_settings() {
        let lanes = [
            SessionLane::ClaudeApiKey {
                credential: super::super::SessionCredential::ApiKey {
                    api_key: "test-key".to_string(),
                },
                base_url: None,
            },
            SessionLane::ZaiGlm {
                credential: super::super::SessionCredential::ApiKey {
                    api_key: "zai-key".to_string(),
                },
                model_overrides: None,
            },
            SessionLane::GrokApiKey {
                credential: super::super::SessionCredential::ApiKey {
                    api_key: "grok-key".to_string(),
                },
                model_overrides: None,
            },
        ];
        for lane in lanes {
            let mut args = Vec::new();
            add_interactive_settings(
                &mut args,
                &lane,
                "session-123",
                Some(Path::new("/absolute/rtk")),
                None,
                false,
            )
            .expect("Claude adapter settings should build");

            assert_eq!(args[0], "--settings");
            let _ = fs::remove_file(&args[1]);
        }
    }

    #[test]
    fn interactive_subscription_without_bundled_rtk_has_no_settings() {
        let mut args = Vec::new();
        add_interactive_settings(
            &mut args,
            &SessionLane::ClaudeSubscription,
            "session-123",
            None,
            None,
            false,
        )
        .expect("missing bundled RTK should keep the launch valid");

        assert!(args.is_empty());
    }

    #[test]
    fn interactive_settings_carry_status_hooks_with_or_without_rtk() {
        let status_hooks = crate::agent_status::ClaudeStatusHooks::new(
            Path::new("/opt/halv/bin/halv-agent-hook"),
            Some("2.1.299"),
        )
        .expect("status hooks should build");
        let read = |args: &[OsString]| -> Value {
            assert_eq!(args[0], "--settings");
            let settings = serde_json::from_slice(&fs::read(&args[1]).expect("settings read"))
                .expect("settings are JSON");
            let _ = fs::remove_file(&args[1]);
            settings
        };

        let mut args = Vec::new();
        add_interactive_settings(
            &mut args,
            &SessionLane::ClaudeSubscription,
            "session-status-only",
            None,
            Some(&status_hooks),
            false,
        )
        .expect("status settings should write");
        let settings = read(&args);
        let object = settings.as_object().expect("settings object");
        assert_eq!(object.keys().collect::<Vec<_>>(), ["hooks"]);
        let hooks = settings["hooks"].as_object().expect("hooks object");
        assert_eq!(hooks.len(), 12);
        assert!(!hooks.contains_key("Elicitation"));
        assert!(!hooks.contains_key("statusLine"));
        assert_eq!(
            settings["hooks"]["Stop"][0]["hooks"][0]["command"],
            "/opt/halv/bin/halv-agent-hook agent-hook claude"
        );

        let mut args = Vec::new();
        add_interactive_settings(
            &mut args,
            &SessionLane::ZaiGlm {
                credential: super::super::SessionCredential::ApiKey {
                    api_key: "zai-key".to_string(),
                },
                model_overrides: None,
            },
            "session-status-rtk",
            Some(Path::new("/absolute/rtk")),
            Some(&status_hooks),
            false,
        )
        .expect("merged settings should write");
        let settings = read(&args);
        let pre_tool_use = settings["hooks"]["PreToolUse"]
            .as_array()
            .expect("PreToolUse groups");
        assert_eq!(pre_tool_use.len(), 2);
        assert_eq!(pre_tool_use[0]["matcher"], "Bash");
        assert_eq!(
            pre_tool_use[0]["hooks"][0]["command"],
            "/absolute/rtk hook claude"
        );
        assert_eq!(pre_tool_use[1]["matcher"], "*");

        // A lane that does not run Claude Code gets no settings file.
        let mut args = Vec::new();
        add_interactive_settings(
            &mut args,
            &SessionLane::CodexChatGpt { proxied: false },
            "session-codex",
            None,
            Some(&status_hooks),
            false,
        )
        .expect("other lanes stay valid");
        assert!(args.is_empty());
    }

    #[test]
    fn gui_and_terminal_use_the_same_permission_arguments() {
        for (mode, expected) in [
            ("read_only", vec!["--permission-mode", "plan"]),
            ("ask", vec!["--permission-mode", "default"]),
            ("on_request", vec!["--permission-mode", "auto"]),
            ("on_failure", vec!["--permission-mode", "acceptEdits"]),
            ("never", vec!["--permission-mode", "dontAsk"]),
            ("skip_all", vec!["--dangerously-skip-permissions"]),
        ] {
            let expected = expected.into_iter().map(OsString::from).collect::<Vec<_>>();
            assert_eq!(
                permission_flags(AgentKind::Claude, mode)
                    .expect("Claude permission mode should map")
                    .cli_args(),
                expected
            );
            let command = gui_command_with_program(
                PathBuf::from("/tmp/claude"),
                Path::new("/tmp"),
                &SessionLane::ClaudeSubscription,
                mode,
                None,
                None,
                "session-123",
                Path::new("/tmp/halv.sqlite3"),
                None,
                None,
                None,
                PonytailMode::Off,
            )
            .expect("Claude GUI command should build");
            let gui_args = command.get_args().map(OsString::from).collect::<Vec<_>>();
            let terminal = interactive_launch_spec_with_program(
                PathBuf::from("/tmp/claude"),
                Path::new("/tmp"),
                &SessionLane::ClaudeSubscription,
                mode,
                None,
                None,
                None,
                "session-123",
                Path::new("/tmp/halv.sqlite3"),
                None,
                PonytailMode::Off,
                false,
                None,
            )
            .expect("Claude terminal specification should build");

            assert!(gui_args.ends_with(&expected));
            assert!(terminal.args.ends_with(&expected));
        }
    }

    #[test]
    fn grok_and_zai_keep_default_permissions_for_on_request_launches() {
        let lanes = [
            (
                AgentKind::Grok,
                SessionLane::GrokApiKey {
                    credential: super::super::SessionCredential::ApiKey {
                        api_key: "grok-test-key".to_string(),
                    },
                    model_overrides: None,
                },
            ),
            (
                AgentKind::Zai,
                SessionLane::ZaiGlm {
                    credential: super::super::SessionCredential::ApiKey {
                        api_key: "zai-test-key".to_string(),
                    },
                    model_overrides: None,
                },
            ),
        ];

        for (agent_kind, lane) in lanes {
            let command = gui_command_with_program_and_crux(
                agent_kind,
                PathBuf::from("/tmp/claude"),
                Path::new("/tmp"),
                &lane,
                "on_request",
                None,
                Some(9123),
                "session-123",
                Path::new("/tmp/halv.sqlite3"),
                None,
                None,
                None,
                PonytailMode::Off,
                TaskRoutingContext::default(),
                false,
            )
            .expect("Claude-compatible GUI command should build");
            let gui_args = command.get_args().map(OsString::from).collect::<Vec<_>>();
            let terminal = interactive_launch_spec_with_program_and_crux(
                agent_kind,
                PathBuf::from("/tmp/claude"),
                Path::new("/tmp"),
                &lane,
                "on_request",
                None,
                None,
                Some(9123),
                "session-123",
                Path::new("/tmp/halv.sqlite3"),
                None,
                PonytailMode::Off,
                false,
                TaskRoutingContext::default(),
                false,
                None,
                None,
                false,
            )
            .expect("Claude-compatible terminal specification should build");
            let expected = ["--permission-mode", "default"];

            assert!(gui_args.windows(2).any(|pair| pair == expected));
            assert!(terminal.args.windows(2).any(|pair| pair == expected));
        }
    }

    #[test]
    fn ponytail_prompt_tracks_mode_and_skips_the_active_claude_plugin() {
        let _guard = super::super::CLAUDE_CONFIG_DIR_TEST_LOCK
            .lock()
            .expect("Claude config test lock poisoned");
        let config_dir =
            env::temp_dir().join(format!("halv-claude-ponytail-{}", rand::random::<u64>()));
        fs::create_dir_all(&config_dir).expect("Claude config directory should exist");
        let previous = env::var_os("CLAUDE_CONFIG_DIR");
        env::set_var("CLAUDE_CONFIG_DIR", &config_dir);

        let gui_args = |mode| {
            gui_command_with_program(
                PathBuf::from("/tmp/claude"),
                Path::new("/tmp"),
                &SessionLane::ClaudeSubscription,
                "ask",
                None,
                None,
                "session-123",
                Path::new("/tmp/halv.sqlite3"),
                None,
                None,
                None,
                mode,
            )
            .expect("Claude GUI command should build")
            .get_args()
            .map(OsString::from)
            .collect::<Vec<_>>()
        };
        let terminal_args = |mode| {
            interactive_launch_spec_with_program(
                PathBuf::from("/tmp/claude"),
                Path::new("/tmp"),
                &SessionLane::ClaudeSubscription,
                "ask",
                None,
                None,
                None,
                "session-123",
                Path::new("/tmp/halv.sqlite3"),
                None,
                mode,
                false,
                None,
            )
            .expect("Claude terminal specification should build")
            .args
        };
        let appended_prompt = |args: &[OsString]| {
            args.windows(2)
                .find(|pair| pair[0] == "--append-system-prompt")
                .map(|pair| pair[1].to_string_lossy().into_owned())
        };

        assert_eq!(appended_prompt(&gui_args(PonytailMode::Off)), None);
        assert_eq!(appended_prompt(&terminal_args(PonytailMode::Off)), None);
        for args in [
            gui_args(PonytailMode::Full),
            terminal_args(PonytailMode::Full),
        ] {
            assert!(appended_prompt(&args)
                .is_some_and(|text| text.contains("PONYTAIL MODE ACTIVE — level: full")));
        }

        fs::write(config_dir.join(".ponytail-active"), "active\n")
            .expect("Ponytail marker should write");
        assert_eq!(appended_prompt(&gui_args(PonytailMode::Full)), None);
        assert_eq!(appended_prompt(&terminal_args(PonytailMode::Full)), None);

        match previous {
            Some(value) => env::set_var("CLAUDE_CONFIG_DIR", value),
            None => env::remove_var("CLAUDE_CONFIG_DIR"),
        }
        let _ = fs::remove_dir_all(config_dir);
    }

    #[test]
    fn fresh_interactive_spec_pins_the_assigned_session_id() {
        let _guard = super::super::CLAUDE_CONFIG_DIR_TEST_LOCK
            .lock()
            .expect("Claude config test lock poisoned");
        let spec = interactive_launch_spec_with_program(
            PathBuf::from("/tmp/claude"),
            Path::new("/tmp"),
            &SessionLane::ClaudeSubscription,
            "ask",
            None,
            Some("11111111-1111-4111-8111-111111111111"),
            None,
            "session-123",
            Path::new("/tmp/halv.sqlite3"),
            None,
            PonytailMode::Off,
            false,
            None,
        )
        .expect("Claude terminal specification should build");
        let args = spec
            .args
            .iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert!(args
            .windows(2)
            .any(|pair| pair == ["--session-id", "11111111-1111-4111-8111-111111111111"]));
        assert!(!args.iter().any(|arg| arg == "--resume"));

        let config_dir = std::env::temp_dir().join(format!(
            "halv-claude-config-{:016x}",
            std::process::id() as u64 ^ 0xc1a0
        ));
        let project_dir = config_dir.join("projects/-tmp");
        fs::create_dir_all(&project_dir).expect("test directory should exist");
        fs::write(
            project_dir.join("22222222-2222-4222-8222-222222222222.jsonl"),
            "{}\n",
        )
        .expect("session fixture should write");
        let previous = env::var_os("CLAUDE_CONFIG_DIR");
        env::set_var("CLAUDE_CONFIG_DIR", &config_dir);
        let spec_args = |resume: &str| {
            interactive_launch_spec_with_program(
                PathBuf::from("/tmp/claude"),
                Path::new("/tmp"),
                &SessionLane::ClaudeSubscription,
                "ask",
                Some(resume),
                Some("11111111-1111-4111-8111-111111111111"),
                None,
                "session-123",
                Path::new("/tmp/halv.sqlite3"),
                None,
                PonytailMode::Off,
                false,
                None,
            )
            .expect("Claude terminal specification should build")
            .args
            .iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>()
        };
        let resumed_args = spec_args("22222222-2222-4222-8222-222222222222");
        // A session Claude never persisted must not be resumed: --resume
        // exits 1, while --session-id reopens the same id cleanly.
        let unpersisted_args = spec_args("33333333-3333-4333-8333-333333333333");
        match previous {
            Some(value) => env::set_var("CLAUDE_CONFIG_DIR", value),
            None => env::remove_var("CLAUDE_CONFIG_DIR"),
        }
        let _ = fs::remove_dir_all(&config_dir);

        assert!(resumed_args
            .windows(2)
            .any(|pair| pair == ["--resume", "22222222-2222-4222-8222-222222222222"]));
        assert!(!resumed_args.iter().any(|arg| arg == "--session-id"));
        assert!(unpersisted_args
            .windows(2)
            .any(|pair| pair == ["--session-id", "33333333-3333-4333-8333-333333333333"]));
        assert!(!unpersisted_args.iter().any(|arg| arg == "--resume"));
    }

    #[test]
    fn requested_model_adds_the_claude_cli_model_flag() {
        let mut command = Command::new("claude");
        apply_model_argument(&mut command, Some("sonnet"));
        let args = command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert_eq!(args, ["--model", "sonnet"]);
    }

    #[test]
    fn requested_effort_adds_the_claude_cli_effort_flag() {
        let command = gui_command_with_program(
            PathBuf::from("/tmp/claude"),
            Path::new("/tmp"),
            &SessionLane::ClaudeSubscription,
            "ask",
            None,
            None,
            "session-123",
            Path::new("/tmp/halv.sqlite3"),
            Some("sonnet"),
            Some("high"),
            None,
            PonytailMode::Off,
        )
        .expect("Claude GUI command should build");
        let args = command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert!(args.windows(2).any(|pair| pair == ["--effort", "high"]));
    }

    #[test]
    fn missing_effort_omits_the_claude_cli_effort_flag() {
        let command = gui_command_with_program(
            PathBuf::from("/tmp/claude"),
            Path::new("/tmp"),
            &SessionLane::ClaudeSubscription,
            "ask",
            None,
            None,
            "session-123",
            Path::new("/tmp/halv.sqlite3"),
            Some("sonnet"),
            None,
            None,
            PonytailMode::Off,
        )
        .expect("Claude GUI command should build");
        assert!(!command.get_args().any(|arg| arg == "--effort"));
    }

    #[test]
    fn subscription_proxy_preference_controls_terminal_routing() {
        let _guard = super::super::CLAUDE_CONFIG_DIR_TEST_LOCK
            .lock()
            .expect("Claude environment test lock poisoned");
        let previous_base_url = env::var_os("ANTHROPIC_BASE_URL");
        env::remove_var("ANTHROPIC_BASE_URL");
        let root = test_directory("subscription-proxy");
        let rtk = root.join("rtk");
        write_test_executable(&rtk);
        let rtk = rtk
            .canonicalize()
            .expect("RTK fixture path should canonicalize");
        let session_id = format!("subscription-proxy-{}", rand::random::<u64>());
        let build = |enabled| {
            interactive_launch_spec_with_program(
                PathBuf::from("/tmp/claude"),
                Path::new("/tmp"),
                &SessionLane::ClaudeSubscription,
                "ask",
                None,
                None,
                Some(9123),
                &session_id,
                Path::new("/tmp/halv.sqlite3"),
                None,
                PonytailMode::Off,
                enabled,
                Some(&rtk),
            )
            .expect("Claude subscription terminal specification should build")
        };

        let disabled = build(false);
        assert_eq!(launch_spec_env(&disabled, "ANTHROPIC_BASE_URL"), None);
        assert_eq!(launch_spec_env(&disabled, "ANTHROPIC_CUSTOM_HEADERS"), None);

        let enabled = build(true);
        assert_eq!(
            launch_spec_env(&enabled, "ANTHROPIC_BASE_URL"),
            Some(format!("http://127.0.0.1:9123/p/{session_id}").as_str())
        );
        assert_eq!(
            launch_spec_env(&enabled, "ANTHROPIC_CUSTOM_HEADERS"),
            Some("X-Headroom-Base-Url: https://api.anthropic.com")
        );
        assert_eq!(launch_spec_env(&enabled, "API_TIMEOUT_MS"), Some("3000000"));
        let settings_index = enabled
            .args
            .iter()
            .position(|argument| argument == "--settings")
            .expect("proxied subscription should retain RTK settings");
        let settings_path = PathBuf::from(&enabled.args[settings_index + 1]);
        let settings: Value = serde_json::from_slice(
            &fs::read(&settings_path).expect("session RTK settings should read"),
        )
        .expect("session RTK settings should be JSON");
        assert_eq!(settings["hooks"]["PreToolUse"][0]["matcher"], "Bash");

        env::set_var("ANTHROPIC_BASE_URL", " https://anthropic.example/custom ");
        let custom_upstream = build(true);
        assert_eq!(
            launch_spec_env(&custom_upstream, "ANTHROPIC_CUSTOM_HEADERS"),
            Some("X-Headroom-Base-Url: https://anthropic.example/custom")
        );

        match previous_base_url {
            Some(value) => env::set_var("ANTHROPIC_BASE_URL", value),
            None => env::remove_var("ANTHROPIC_BASE_URL"),
        }
        let _ = fs::remove_file(settings_path);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn api_key_lane_routes_through_the_session_proxy() {
        for (base_url, expected_upstream) in [
            (None, ANTHROPIC_DEFAULT_BASE_URL),
            (
                Some(" https://anthropic.example/v1 "),
                "https://anthropic.example/v1",
            ),
        ] {
            let lane = SessionLane::ClaudeApiKey {
                credential: super::super::SessionCredential::ApiKey {
                    api_key: "anthropic-test-key".to_string(),
                },
                base_url: base_url.map(str::to_string),
            };
            let command = gui_command_with_program(
                PathBuf::from("/tmp/claude"),
                Path::new("/tmp"),
                &lane,
                "default",
                None,
                Some(9123),
                "session-123",
                Path::new("/tmp/halv.sqlite3"),
                None,
                None,
                None,
                PonytailMode::Off,
            )
            .expect("Claude API-key GUI command should build");
            let expected_header = format!("X-Headroom-Base-Url: {expected_upstream}");

            assert_eq!(
                command_env(&command, "ANTHROPIC_BASE_URL"),
                Some("http://127.0.0.1:9123/p/session-123")
            );
            assert_eq!(
                command_env(&command, "ANTHROPIC_CUSTOM_HEADERS"),
                Some(expected_header.as_str())
            );
            assert_eq!(
                command_env(&command, "ANTHROPIC_API_KEY"),
                Some("anthropic-test-key")
            );
        }
    }

    #[test]
    fn zai_lane_sets_proxy_auth_timeout_and_model_environment() {
        let lane = SessionLane::ZaiGlm {
            credential: super::super::SessionCredential::ApiKey {
                api_key: "zai-test-key".to_string(),
            },
            model_overrides: Some(super::super::ZaiModelOverrides {
                sonnet: Some("glm-sonnet-test".to_string()),
                opus: None,
                haiku: Some(" ".to_string()),
            }),
        };
        let command = gui_command_with_program(
            PathBuf::from("/tmp/claude"),
            Path::new("/tmp"),
            &lane,
            "default",
            None,
            Some(9123),
            "session-123",
            Path::new("/tmp/halv.sqlite3"),
            Some("sonnet"),
            None,
            None,
            PonytailMode::Off,
        )
        .expect("Z.ai GUI command should build");

        assert_eq!(
            command_env(&command, "ANTHROPIC_AUTH_TOKEN"),
            Some("zai-test-key")
        );
        assert_eq!(
            command_env(&command, "ANTHROPIC_BASE_URL"),
            Some("http://127.0.0.1:9123/p/session-123")
        );
        assert_eq!(
            command_env(&command, "ANTHROPIC_CUSTOM_HEADERS"),
            Some("X-Headroom-Base-Url: https://api.z.ai/api/anthropic")
        );
        assert_eq!(command_env(&command, "API_TIMEOUT_MS"), Some("3000000"));
        assert_eq!(
            command_env(&command, "ANTHROPIC_DEFAULT_SONNET_MODEL"),
            Some("glm-sonnet-test")
        );
        assert_eq!(
            command_env(&command, "ANTHROPIC_DEFAULT_OPUS_MODEL"),
            Some(ZAI_DEFAULT_OPUS_MODEL)
        );
        assert_eq!(
            command_env(&command, "ANTHROPIC_DEFAULT_HAIKU_MODEL"),
            Some(ZAI_DEFAULT_HAIKU_MODEL)
        );
        assert!(command.get_envs().any(|(name, value)| {
            name == std::ffi::OsStr::new("ANTHROPIC_API_KEY") && value.is_none()
        }));
        assert!(command_env(&command, "PATH").is_some());
        assert!(command_env(&command, super::super::exec::RTK_DB_PATH_ENV).is_some());
    }

    #[test]
    fn grok_lane_sets_proxy_auth_timeout_and_model_environment() {
        let lane = SessionLane::GrokApiKey {
            credential: super::super::SessionCredential::ApiKey {
                api_key: "xai-test-key".to_string(),
            },
            model_overrides: Some(super::super::GrokModelOverrides {
                sonnet: Some("grok-sonnet-test".to_string()),
                opus: None,
                haiku: Some(" ".to_string()),
            }),
        };
        let command = gui_command_with_program(
            PathBuf::from("/tmp/claude"),
            Path::new("/tmp"),
            &lane,
            "default",
            None,
            Some(9123),
            "session-123",
            Path::new("/tmp/halv.sqlite3"),
            Some("sonnet"),
            None,
            None,
            PonytailMode::Off,
        )
        .expect("Grok GUI command should build");

        assert_eq!(
            command_env(&command, "ANTHROPIC_AUTH_TOKEN"),
            Some("xai-test-key")
        );
        assert_eq!(
            command_env(&command, "ANTHROPIC_BASE_URL"),
            Some("http://127.0.0.1:9123/p/session-123")
        );
        assert_eq!(
            command_env(&command, "ANTHROPIC_CUSTOM_HEADERS"),
            Some("X-Headroom-Base-Url: https://api.x.ai")
        );
        assert_eq!(command_env(&command, "API_TIMEOUT_MS"), Some("3000000"));
        assert_eq!(
            command_env(&command, "ANTHROPIC_DEFAULT_SONNET_MODEL"),
            Some("grok-sonnet-test")
        );
        assert_eq!(
            command_env(&command, "ANTHROPIC_DEFAULT_OPUS_MODEL"),
            Some(GROK_DEFAULT_OPUS_MODEL)
        );
        assert_eq!(
            command_env(&command, "ANTHROPIC_DEFAULT_HAIKU_MODEL"),
            Some(GROK_DEFAULT_HAIKU_MODEL)
        );
        assert!(command.get_envs().any(|(name, value)| {
            name == std::ffi::OsStr::new("ANTHROPIC_API_KEY") && value.is_none()
        }));
    }

    #[test]
    fn proxy_lanes_go_direct_when_the_halv_engine_is_off() {
        let credential = |api_key: &str| super::super::SessionCredential::ApiKey {
            api_key: api_key.to_string(),
        };
        let cases = [
            (
                SessionLane::ClaudeApiKey {
                    credential: credential("anthropic-test-key"),
                    base_url: None,
                },
                ANTHROPIC_DEFAULT_BASE_URL,
            ),
            (
                SessionLane::ClaudeApiKey {
                    credential: credential("anthropic-test-key"),
                    base_url: Some(" https://anthropic.example/v1 ".to_string()),
                },
                "https://anthropic.example/v1",
            ),
            (
                SessionLane::ZaiGlm {
                    credential: credential("zai-test-key"),
                    model_overrides: None,
                },
                ZAI_ANTHROPIC_BASE_URL,
            ),
            (
                SessionLane::GrokApiKey {
                    credential: credential("xai-test-key"),
                    model_overrides: None,
                },
                GROK_ANTHROPIC_BASE_URL,
            ),
        ];
        for (lane, upstream) in cases {
            let command = gui_command_with_program(
                PathBuf::from("/tmp/claude"),
                Path::new("/tmp"),
                &lane,
                "default",
                None,
                None,
                "session-123",
                Path::new("/tmp/halv.sqlite3"),
                None,
                None,
                None,
                PonytailMode::Off,
            )
            .expect("a lane without an engine port should build");

            assert_eq!(command_env(&command, "ANTHROPIC_BASE_URL"), Some(upstream));
            assert_eq!(command_env(&command, "ANTHROPIC_CUSTOM_HEADERS"), None);
            match lane {
                SessionLane::ClaudeApiKey { .. } => assert_eq!(
                    command_env(&command, "ANTHROPIC_API_KEY"),
                    Some("anthropic-test-key")
                ),
                SessionLane::ZaiGlm { .. } => {
                    assert_eq!(
                        command_env(&command, "ANTHROPIC_AUTH_TOKEN"),
                        Some("zai-test-key")
                    );
                    assert_eq!(command_env(&command, "API_TIMEOUT_MS"), Some("3000000"));
                    assert_eq!(
                        command_env(&command, "ANTHROPIC_DEFAULT_OPUS_MODEL"),
                        Some(ZAI_DEFAULT_OPUS_MODEL)
                    );
                }
                _ => {
                    assert_eq!(
                        command_env(&command, "ANTHROPIC_AUTH_TOKEN"),
                        Some("xai-test-key")
                    );
                    assert_eq!(command_env(&command, "API_TIMEOUT_MS"), Some("3000000"));
                    assert_eq!(
                        command_env(&command, "ANTHROPIC_DEFAULT_OPUS_MODEL"),
                        Some(GROK_DEFAULT_OPUS_MODEL)
                    );
                }
            }
        }
    }

    fn command_env<'a>(command: &'a Command, name: &str) -> Option<&'a str> {
        command.get_envs().find_map(|(candidate, value)| {
            (candidate == std::ffi::OsStr::new(name))
                .then_some(value)
                .flatten()
                .and_then(std::ffi::OsStr::to_str)
        })
    }

    fn launch_spec_env<'a>(spec: &'a LaunchSpec, name: &str) -> Option<&'a str> {
        spec.env
            .iter()
            .find_map(|(candidate, value)| (candidate == name).then_some(value))
            .and_then(|value| value.to_str())
    }

    #[test]
    fn parses_init_line() {
        let mut parser = ClaudeEventParser::default();
        let events = parser.parse_line(
            r#"{"type":"system","subtype":"init","session_id":"b4ea27c5-f1f1-4a09-96f5-93431917ed66","model":"claude-sonnet-4-20250514","permissionMode":"default"}"#,
        );

        assert_eq!(
            events,
            vec![AgentEvent::SessionInit {
                cli_session_id: "b4ea27c5-f1f1-4a09-96f5-93431917ed66".to_string(),
                model: "claude-sonnet-4-20250514".to_string(),
            }]
        );
    }

    #[test]
    fn parses_assistant_text_and_tool_use_lines() {
        let mut parser = ClaudeEventParser::default();
        let events = parser.parse_line(
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"I will inspect the file."},{"type":"tool_use","id":"toolu_01A","name":"Read","input":{"file_path":"/tmp/demo.rs"}}]},"session_id":"session-1"}"#,
        );

        assert_eq!(
            events,
            vec![
                AgentEvent::TextDelta {
                    text: "I will inspect the file.".to_string(),
                },
                AgentEvent::ToolCallStarted {
                    name: "Read".to_string(),
                    summary_json: r#"{"file_path":"/tmp/demo.rs"}"#.to_string(),
                },
            ]
        );

        let completed = parser.parse_line(
            r#"{"type":"user","message":{"role":"user","content":[{"tool_use_id":"toolu_01A","type":"tool_result","content":"fn main() {}"}]},"session_id":"session-1"}"#,
        );
        assert_eq!(
            completed,
            vec![AgentEvent::ToolCallCompleted {
                name: "Read".to_string(),
            }]
        );
    }

    #[test]
    fn parses_result_usage_line() {
        let mut parser = ClaudeEventParser::default();
        let events = parser.parse_line(
            r#"{"type":"result","subtype":"success","session_id":"session-1","total_cost_usd":0.0125,"usage":{"input_tokens":11,"output_tokens":22,"cache_read_input_tokens":33,"cache_creation_input_tokens":44}}"#,
        );

        assert_eq!(
            events,
            vec![AgentEvent::TurnCompleted {
                usage: TurnUsage {
                    input_tokens: 11,
                    output_tokens: 22,
                    cache_read_tokens: 33,
                    cache_creation_tokens: 44,
                    total_cost_usd: Some(0.0125),
                },
            }]
        );
    }

    #[test]
    fn maps_logged_out_result_to_an_actionable_protocol_error() {
        let mut parser = ClaudeEventParser::default();
        assert_eq!(
            parser.parse_line(
                r#"{"type":"result","is_error":true,"result":"Not logged in · Please run /login"}"#,
            ),
            vec![AgentEvent::ProtocolError {
                message: "Open Agent Sessions and sign in to Claude Code before sending."
                    .to_string(),
            }],
        );
    }

    #[test]
    fn converts_claude_session_cost_to_turn_cost() {
        let mut parser = ClaudeEventParser::default();
        let first = parser.parse_line(r#"{"type":"result","total_cost_usd":0.0125,"usage":{}}"#);
        let second = parser.parse_line(r#"{"type":"result","total_cost_usd":0.0200,"usage":{}}"#);

        assert!(matches!(
            first.as_slice(),
            [AgentEvent::TurnCompleted {
                usage: TurnUsage {
                    total_cost_usd: Some(cost),
                    ..
                }
            }] if (*cost - 0.0125).abs() < f64::EPSILON
        ));
        assert!(matches!(
            second.as_slice(),
            [AgentEvent::TurnCompleted {
                usage: TurnUsage {
                    total_cost_usd: Some(cost),
                    ..
                }
            }] if (*cost - 0.0075).abs() < f64::EPSILON
        ));
    }

    #[test]
    fn counts_a_response_split_over_lines_once() {
        let home = env::temp_dir().join(format!(
            "halv-claude-usage-split-{}",
            crate::storage::new_uuid_v4()
        ));
        let project = home.join(".claude/projects/demo");
        fs::create_dir_all(&project).expect("transcript directory should be created");
        let line = |uuid: &str| {
            format!(
                r#"{{"type":"assistant","uuid":"{uuid}","message":{{"id":"msg_1","usage":{{"input_tokens":1,"output_tokens":2,"cache_read_input_tokens":3,"cache_creation_input_tokens":4}}}}}}"#
            )
        };
        fs::write(
            project.join("session-9.jsonl"),
            format!("{}\n{}\n", line("block-1"), line("block-2")),
        )
        .expect("main transcript should be written");

        let usage = transcript_usage_in_home(&home, "session-9")
            .expect("transcript usage should be available");
        assert_eq!(usage.cache_read_tokens, 3);
        assert_eq!(usage.output_tokens, 2);
        fs::remove_dir_all(home).expect("transcript fixture should be removed");
    }

    #[test]
    fn reads_main_and_subagent_transcript_usage_once() {
        let home = env::temp_dir().join(format!(
            "halv-claude-usage-{}",
            crate::storage::new_uuid_v4()
        ));
        let project = home.join(".claude/projects/demo");
        let subagents = project.join("session-123/subagents");
        fs::create_dir_all(&subagents).expect("transcript directories should be created");
        fs::write(
            project.join("session-123.jsonl"),
            concat!(
                r#"{"type":"assistant","uuid":"main-1","message":{"usage":{"input_tokens":1,"output_tokens":2,"cache_read_input_tokens":3,"cache_creation_input_tokens":4}}}"#,
                "\n",
                r#"{"type":"user","uuid":"ignored","message":{"usage":{"input_tokens":100}}}"#,
                "\n"
            ),
        )
        .expect("main transcript should be written");
        fs::write(
            subagents.join("agent-a.jsonl"),
            concat!(
                r#"{"type":"assistant","uuid":"agent-1","message":{"usage":{"input_tokens":10,"output_tokens":20,"cache_read_input_tokens":30,"cache_creation_input_tokens":40}}}"#,
                "\n",
                r#"{"type":"assistant","uuid":"main-1","message":{"usage":{"input_tokens":1,"output_tokens":2,"cache_read_input_tokens":3,"cache_creation_input_tokens":4}}}"#,
                "\n"
            ),
        )
        .expect("subagent transcript should be written");

        let usage = transcript_usage_in_home(&home, "session-123")
            .expect("transcript usage should be available");
        assert_eq!(usage.input_tokens, 11);
        assert_eq!(usage.output_tokens, 22);
        assert_eq!(usage.cache_read_tokens, 33);
        assert_eq!(usage.cache_creation_tokens, 44);
        fs::remove_dir_all(home).expect("transcript fixture should be removed");
    }

    #[test]
    fn malformed_lines_report_protocol_errors_and_unknown_types_are_ignored() {
        let mut parser = ClaudeEventParser::default();
        assert!(matches!(
            parser.parse_line("{not-json").as_slice(),
            [AgentEvent::ProtocolError { .. }]
        ));
        assert!(parser
            .parse_line(r#"{"type":"rate_limit_event"}"#)
            .is_empty());
    }

    #[test]
    fn user_message_uses_the_stream_json_protocol() {
        let line = user_message_line("Hello \"Claude\"\nnext line")
            .expect("user message should serialize");
        let value: Value = serde_json::from_str(&line).expect("user message should parse");
        assert_eq!(value["type"], "user");
        assert_eq!(
            value["message"]["content"][0]["text"],
            "Hello \"Claude\"\nnext line"
        );
    }

    #[test]
    fn golden_claude_launch_protocol_and_parser_contract() {
        let lane = SessionLane::Custom {
            env: vec![("CLAUDE_GOLDEN".to_string(), "fixed".to_string())],
        };
        let permission_modes = [
            ("read_only", vec!["--permission-mode", "plan"]),
            ("ask", vec!["--permission-mode", "default"]),
            ("on_request", vec!["--permission-mode", "auto"]),
            ("on_failure", vec!["--permission-mode", "acceptEdits"]),
            ("never", vec!["--permission-mode", "dontAsk"]),
            ("skip_all", vec!["--dangerously-skip-permissions"]),
        ];

        for (mode, expected_flags) in permission_modes {
            let expected_flags = expected_flags
                .into_iter()
                .map(OsString::from)
                .collect::<Vec<_>>();
            assert_eq!(
                permission_flags(AgentKind::Claude, mode)
                    .expect("Claude permission flags should build")
                    .cli_args(),
                expected_flags
            );

            for resume in [None, Some("claude-resume")] {
                let command = gui_command_with_program(
                    PathBuf::from("/golden/claude"),
                    Path::new("/tmp"),
                    &lane,
                    mode,
                    resume,
                    None,
                    "golden-session",
                    Path::new("/tmp/golden.sqlite3"),
                    Some("sonnet"),
                    Some("high"),
                    None,
                    PonytailMode::Off,
                )
                .expect("Claude GUI launch should build");
                let mut expected_args = vec![
                    OsString::from("--print"),
                    OsString::from("--input-format"),
                    OsString::from("stream-json"),
                    OsString::from("--output-format"),
                    OsString::from("stream-json"),
                    OsString::from("--replay-user-messages"),
                    OsString::from("--verbose"),
                    OsString::from("--mcp-config"),
                    agent_mcp_config(
                        Path::new("/tmp/golden.sqlite3"),
                        "golden-session",
                        TaskRoutingContext::default(),
                        false,
                    )
                    .expect("Claude MCP configuration should build"),
                ];
                expected_args.extend(expected_flags.clone());
                if let Some(session_id) = resume {
                    expected_args.extend([OsString::from("--resume"), OsString::from(session_id)]);
                }
                expected_args.extend([
                    OsString::from("--model"),
                    OsString::from("sonnet"),
                    OsString::from("--effort"),
                    OsString::from("high"),
                ]);
                assert_eq!(command.get_program(), Path::new("/golden/claude"));
                assert_eq!(
                    command.get_args().map(OsString::from).collect::<Vec<_>>(),
                    expected_args
                );

                let terminal = interactive_launch_spec_with_program(
                    PathBuf::from("/golden/claude"),
                    Path::new("/tmp"),
                    &lane,
                    mode,
                    resume,
                    (!resume.is_some()).then_some("claude-fresh"),
                    None,
                    "golden-session",
                    Path::new("/tmp/golden.sqlite3"),
                    None,
                    PonytailMode::Off,
                    false,
                    None,
                )
                .expect("Claude interactive launch should build");
                let mut expected_terminal_args = vec![
                    OsString::from("--mcp-config"),
                    agent_mcp_config(
                        Path::new("/tmp/golden.sqlite3"),
                        "golden-session",
                        TaskRoutingContext::default(),
                        false,
                    )
                    .expect("Claude MCP configuration should build"),
                ];
                expected_terminal_args.extend(expected_flags.clone());
                expected_terminal_args.extend([
                    OsString::from("--session-id"),
                    OsString::from(resume.unwrap_or("claude-fresh")),
                ]);
                assert_eq!(terminal.program, PathBuf::from("/golden/claude"));
                assert_eq!(terminal.args, expected_terminal_args);
                assert_eq!(
                    terminal.env,
                    vec![
                        (OsString::from("CLAUDE_GOLDEN"), OsString::from("fixed")),
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
            user_message_line("Hello \"Claude\"\nnext line")
                .expect("Claude user message should encode"),
            r#"{"message":{"content":[{"text":"Hello \"Claude\"\nnext line","type":"text"}],"role":"user"},"type":"user"}"#
        );
        let mut parser = ClaudeEventParser::default();
        assert_eq!(
            parser.parse_line(
                r#"{"type":"system","subtype":"init","session_id":"claude-session","model":"sonnet"}"#,
            ),
            vec![AgentEvent::SessionInit {
                cli_session_id: "claude-session".to_string(),
                model: "sonnet".to_string(),
            }]
        );
    }

    #[test]
    fn adapter_login_launch_spec_is_exact() {
        assert_eq!(
            login_launch_spec_with_program(PathBuf::from("/golden/claude")),
            LaunchSpec {
                program: PathBuf::from("/golden/claude"),
                args: vec![OsString::from("auth"), OsString::from("login")],
                env: Vec::new(),
                env_remove: Vec::new(),
            }
        );
    }

    #[test]
    fn task_routing_appends_the_coordinator_instructions() {
        let _guard = super::super::CLAUDE_CONFIG_DIR_TEST_LOCK
            .lock()
            .expect("Claude config test lock poisoned");
        let spec = |enabled: bool| {
            interactive_launch_spec_with_program_and_crux(
                AgentKind::Claude,
                PathBuf::from("/tmp/claude"),
                Path::new("/tmp"),
                &SessionLane::ClaudeSubscription,
                "ask",
                None,
                None,
                None,
                "session-123",
                Path::new("/tmp/halv.sqlite3"),
                None,
                PonytailMode::Off,
                false,
                TaskRoutingContext {
                    enabled,
                    coordinator_model: Some("opus"),
                    conversation_id: Some("conversation-1"),
                    ..TaskRoutingContext::default()
                },
                false,
                None,
                None,
                false,
            )
            .expect("Claude terminal specification should build")
            .args
        };
        let routed = spec(true);
        let appended = routed
            .windows(2)
            .filter(|pair| pair[0] == "--append-system-prompt")
            .map(|pair| pair[1].to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert!(appended
            .iter()
            .any(|text| text.contains("# Halv task routing")));
        assert!(appended.iter().any(|text| text
            .contains("Use the Codex hierarchy when the requested `workerLane` is `codex`")));
        assert!(appended
            .iter()
            .any(|text| text
                .contains("A saved tier assignment can override it for the selected tier")));
        assert!(appended.iter().any(
            |text| text.contains("Use the legacy flow when `workerLane` is `claude` or `auto`")
        ));
        assert!(appended
            .iter()
            .any(|text| text.contains("Set its `model` and `effort` parameters")));
        let configuration = routed
            .windows(2)
            .find(|pair| pair[0] == "--mcp-config")
            .map(|pair| pair[1].to_string_lossy().into_owned())
            .expect("the MCP configuration should exist");
        assert!(configuration.contains("--coordinator-model"));
        assert!(configuration.contains("conversation-1"));

        let plain = spec(false);
        assert!(!plain
            .windows(2)
            .any(|pair| pair[0] == "--append-system-prompt"));
    }

    #[allow(clippy::too_many_arguments)]
    fn routed_terminal_args(
        lane: &SessionLane,
        permission_mode: &str,
        ponytail_mode: PonytailMode,
        coordinator_model: Option<&str>,
        workflow_routing: bool,
        claude_ultracode: bool,
        session_id: &str,
    ) -> Vec<String> {
        interactive_launch_spec_with_program_and_crux(
            AgentKind::Claude,
            PathBuf::from("/tmp/claude"),
            Path::new("/tmp"),
            lane,
            permission_mode,
            None,
            None,
            None,
            session_id,
            Path::new("/tmp/halv.sqlite3"),
            None,
            ponytail_mode,
            false,
            TaskRoutingContext {
                enabled: true,
                coordinator_model,
                conversation_id: Some("conversation-1"),
                workflow_routing,
                ..TaskRoutingContext::default()
            },
            claude_ultracode,
            None,
            None,
            false,
        )
        .expect("Claude terminal specification should build")
        .args
        .into_iter()
        .map(|argument| argument.to_string_lossy().into_owned())
        .collect()
    }

    fn flag_values(args: &[String], flag: &str) -> Vec<String> {
        args.windows(2)
            .filter(|pair| pair[0] == flag)
            .map(|pair| pair[1].clone())
            .collect()
    }

    #[test]
    fn workflow_routing_joins_one_system_prompt_and_drops_a_codex_hint() {
        let _guard = super::super::CLAUDE_CONFIG_DIR_TEST_LOCK
            .lock()
            .expect("Claude config test lock poisoned");
        let args = routed_terminal_args(
            &SessionLane::ClaudeSubscription,
            "ask",
            PonytailMode::Full,
            Some("gpt-6-astra"),
            true,
            false,
            "session-workflow-1",
        );
        let prompts = flag_values(&args, "--append-system-prompt");
        assert_eq!(prompts.len(), 1);
        let prompt = &prompts[0];
        assert!(prompt.contains("# Halv task routing"));
        let workflow = prompt
            .find("## Workflow routing")
            .expect("the workflow section should exist");
        let hierarchy = prompt.find("## Codex hierarchy").unwrap();
        assert!(workflow < hierarchy);
        let configuration = flag_values(&args, "--mcp-config").remove(0);
        assert!(!configuration.contains("--coordinator-model"));
        assert!(!configuration.contains("--workflow-routing"));
        let configuration: Value = serde_json::from_str(&configuration).unwrap();
        assert_eq!(
            configuration["mcpServers"]["halv-savings"]["timeout"],
            SAVINGS_MCP_TOOL_TIMEOUT_MS
        );
        assert!(flag_values(&args, "--allowedTools").is_empty());

        let off = routed_terminal_args(
            &SessionLane::ClaudeSubscription,
            "ask",
            PonytailMode::Off,
            Some("claude-opus-5-5"),
            false,
            false,
            "session-workflow-2",
        );
        let prompt = flag_values(&off, "--append-system-prompt").remove(0);
        assert!(prompt.contains("# Halv task routing"));
        assert!(!prompt.contains("## Workflow routing"));
        let configuration = flag_values(&off, "--mcp-config").remove(0);
        assert!(configuration.contains("--coordinator-model"));
        assert!(configuration.contains("--workflow-routing"));
    }

    #[test]
    fn routing_in_dont_ask_mode_allows_the_halv_tools() {
        let _guard = super::super::CLAUDE_CONFIG_DIR_TEST_LOCK
            .lock()
            .expect("Claude config test lock poisoned");
        let args = routed_terminal_args(
            &SessionLane::ClaudeSubscription,
            "never",
            PonytailMode::Off,
            None,
            true,
            false,
            "session-workflow-3",
        );
        assert_eq!(
            flag_values(&args, "--allowedTools"),
            vec!["mcp__halv-savings"]
        );
    }

    #[test]
    fn the_ultracode_preference_writes_the_session_settings() {
        let _guard = super::super::CLAUDE_CONFIG_DIR_TEST_LOCK
            .lock()
            .expect("Claude config test lock poisoned");
        let settings = |lane: &SessionLane, ultracode: bool, session_id: &str| {
            let mut args = Vec::new();
            add_interactive_settings(&mut args, lane, session_id, None, None, ultracode)
                .expect("session settings should build");
            let path = args.get(1).map(PathBuf::from);
            let value = path
                .as_ref()
                .map(|path| serde_json::from_slice::<Value>(&fs::read(path).unwrap()).unwrap());
            if let Some(path) = path {
                let _ = fs::remove_file(path);
            }
            value
        };
        let on = settings(&SessionLane::ClaudeSubscription, true, "session-ultra-1")
            .expect("ultracode needs a settings file without RTK");
        assert_eq!(on["ultracode"], true);
        assert_eq!(
            settings(&SessionLane::ClaudeSubscription, false, "session-ultra-2"),
            None
        );
        let glm = SessionLane::ZaiGlm {
            credential: super::super::SessionCredential::ApiKey {
                api_key: "zai-test-key".to_string(),
            },
            model_overrides: None,
        };
        assert_eq!(settings(&glm, true, "session-ultra-3"), None);
    }
}
