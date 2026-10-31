use crate::{
    app_data, exec, projects, secret_store,
    storage::{CruxIndexState, Storage},
};
use base64::{
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
    Engine as _,
};
use chrono::Utc;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    ffi::{OsStr, OsString},
    fs,
    io::{BufRead, BufReader as StdBufReader, Write},
    path::{Path, PathBuf},
    process::Command as StdCommand,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex, OnceLock,
    },
    time::Duration,
};
use tauri::Manager;
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader, Lines},
    net::TcpListener,
    process::{Child, ChildStdin, ChildStdout, Command},
    sync::{mpsc, oneshot},
};
use url::Url;

const CONFIG_FILE: &str = "extensions.json";
const MAX_SKILL_BYTES: u64 = 256 * 1024;
const MAX_SKILLS: usize = 128;
const MAX_PLUGIN_FILES: usize = 2_000;
const MAX_PLUGIN_BYTES: u64 = 256 * 1024 * 1024;
const MCP_PROTOCOL_VERSION: &str = "2025-11-25";
const KEYRING_SERVICE: &str = "Halv";
const MAX_MCP_TOOLS: usize = 256;
const MAX_MCP_MESSAGE_BYTES: usize = 4 * 1024 * 1024;
const MAX_MCP_LIST_PAGES: usize = 16;
const MAX_MCP_RESULT_IMAGES: usize = 8;
const MAX_MCP_ELICITATIONS_PER_REQUEST: usize = 8;
const MCP_STDIO_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(2);
const MAX_MCP_ELICITATION_MESSAGE_CHARS: usize = 4_000;
const MAX_MCP_OAUTH_METADATA_BYTES: usize = 256 * 1024;
const MCP_OAUTH_CALLBACK_PATH: &str = "/oauth/callback";
const MCP_OAUTH_LOGIN_TIMEOUT_SECS: u64 = 5 * 60;
const MCP_OAUTH_REFRESH_WINDOW_SECS: i64 = 60;
const BUILTIN_CRUX_ID: &str = "builtin.crux";
pub(crate) const TERMINAL_CRUX_ID: &str = "crux";
const BUILTIN_CRUX_NAME: &str = "Crux code navigation";
const CRUX_INDEX_RELATIVE_PATH: &str = ".scip-nav/index.scip";
const CRUX_FILE_THRESHOLD: u64 = 200;
const CRUX_LINE_THRESHOLD: u64 = 75_000;
const CRUX_INDEX_STATS_MAX_CHARS: usize = 32_000;
const CRUX_LOGIN_SHELL_TIMEOUT: Duration = Duration::from_secs(2);
const CRUX_LOGIN_SHELL_OUTPUT_MAX_BYTES: usize = 8 * 1024;
const CRUX_TOOLS: [&str; 6] = [
    "scip_index",
    "scip_find",
    "scip_map",
    "scip_outline",
    "scip_expand",
    "scip_architecture",
];
// Derived from the 2026-08-13 crux-bench paired runs (Django + SymPy, Codex CLI).
// Method: cache-discounted per-question delta (vanilla minus crux), restricted to
// pairs where both arms answered correctly and the crux arm made scip calls,
// divided by observed scip_map calls: about 1,900 tokens per call.
// scip_find and scip_outline had no calls in that run; their values keep the
// previous table's ratio to scip_map (0.21 and 0.13).
const CRUX_SAVINGS_ESTIMATES: [(&str, u64); 5] = [
    ("scip_index", 0),
    ("scip_find", 400),
    ("scip_map", 1_900),
    ("scip_outline", 250),
    ("scip_expand", 0),
];
static CRUX_BINARY_MISSING_LOGGED: AtomicBool = AtomicBool::new(false);
static CRUX_LOGIN_SHELL_TOOL_DIRS: OnceLock<Vec<PathBuf>> = OnceLock::new();
type ExtensionResult<T> = Result<T, String>;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct ExtensionConfig {
    skill_enabled: HashMap<String, bool>,
    plugins: Vec<PluginDescriptor>,
    mcp_servers: Vec<McpServerConfig>,
    builtin_mcp_enabled: HashMap<String, bool>,
    crux_activation: CruxActivation,
    // Crux indexers run the build tools of a project. Halv indexes only the
    // projects in this set, or projects that it indexed before.
    crux_approved_projects: BTreeSet<i64>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CruxActivation {
    #[default]
    Auto,
    AlwaysOn,
    AlwaysOff,
}

#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CruxProjectCensus {
    pub languages: BTreeMap<String, CruxLanguageCensus>,
}

#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CruxLanguageCensus {
    pub files: u64,
    pub lines: u64,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CruxIndexStatusState {
    Indexed,
    Indexing,
    WaitingForIndex,
    ApprovalRequired,
    Off,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CruxIndexStatus {
    pub state: CruxIndexStatusState,
    pub document_count: Option<u64>,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CruxWorkspaceOpenResult {
    pub approval_required: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SkillDescriptor {
    pub id: String,
    pub name: String,
    pub description: String,
    pub source: String,
    pub scope: String,
    pub root: String,
    pub enabled: bool,
    pub fingerprint: String,
    pub display_name: String,
    pub short_description: String,
    pub allow_implicit_invocation: bool,
    pub default_prompt: Option<String>,
    pub dependency_tools: Vec<String>,
    pub plugin_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PluginDescriptor {
    pub id: String,
    pub name: String,
    pub version: String,
    pub root: String,
    pub enabled: bool,
    pub installed_at: String,
    pub contributes_skills: bool,
    pub contributes_mcp: bool,
    pub unsupported_components: Vec<String>,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub display_name: String,
    #[serde(default)]
    pub developer_name: Option<String>,
    #[serde(default)]
    pub capabilities: Vec<String>,
    #[serde(default)]
    pub skill_count: usize,
    #[serde(default)]
    pub mcp_server_count: usize,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PluginCandidate {
    pub id: String,
    pub name: String,
    pub display_name: String,
    pub version: String,
    pub description: String,
    pub path: String,
    pub capabilities: Vec<String>,
    pub installed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum McpTransport {
    Stdio,
    StreamableHttp,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum McpServerSource {
    #[default]
    User,
    Plugin,
    BuiltIn,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum McpAuthMode {
    #[default]
    None,
    Bearer,
    Oauth,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct McpServerConfig {
    pub id: String,
    pub name: String,
    pub transport: McpTransport,
    pub command: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
    pub cwd: Option<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default)]
    pub env_vars: Vec<String>,
    pub url: Option<String>,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    #[serde(default)]
    pub env_http_headers: BTreeMap<String, String>,
    #[serde(default)]
    pub bearer_token_env_var: Option<String>,
    #[serde(default)]
    pub auth_mode: McpAuthMode,
    #[serde(default)]
    pub oauth_scopes: Vec<String>,
    #[serde(default)]
    pub oauth_client_id: Option<String>,
    pub enabled: bool,
    pub required: bool,
    pub startup_timeout_ms: u64,
    pub tool_timeout_ms: u64,
    #[serde(default)]
    pub enabled_tools: Vec<String>,
    #[serde(default)]
    pub disabled_tools: Vec<String>,
    pub approval_mode: String,
    #[serde(default)]
    pub tool_approval_modes: BTreeMap<String, String>,
    #[serde(default)]
    pub trust_tool_annotations: bool,
    #[serde(default)]
    pub source: McpServerSource,
    #[serde(default)]
    pub plugin_id: Option<String>,
    #[serde(default)]
    pub plugin_name: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtensionsSnapshot {
    pub skills: Vec<SkillDescriptor>,
    pub plugins: Vec<PluginDescriptor>,
    pub available_plugins: Vec<PluginCandidate>,
    pub mcp_servers: Vec<McpServerConfig>,
    pub mcp_auth: Vec<McpAuthStatus>,
    pub crux_activation: CruxActivation,
    pub crux_enabled: bool,
    pub crux_census: Option<CruxProjectCensus>,
    pub crux_index_status: CruxIndexStatus,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct McpToolDescriptor {
    pub server_id: String,
    pub server_name: String,
    pub name: String,
    pub wire_name: String,
    pub description: String,
    pub input_schema: Value,
    pub approval_mode: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpToolImage {
    pub media_type: String,
    pub data: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpToolCallResult {
    pub text: String,
    pub images: Vec<McpToolImage>,
    pub is_error: bool,
}

#[derive(Debug)]
pub struct McpElicitationRequest {
    pub mode: String,
    pub message: String,
    pub requested_schema: Option<Value>,
    pub url: Option<String>,
    pub responder: oneshot::Sender<McpElicitationResponse>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct McpElicitationResponse {
    pub action: String,
    pub content: Option<Value>,
}

pub type McpElicitationSender = mpsc::Sender<McpElicitationRequest>;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct McpToolAnnotations {
    pub title: Option<String>,
    pub read_only_hint: bool,
    pub destructive_hint: bool,
    pub idempotent_hint: bool,
    pub open_world_hint: bool,
}

impl Default for McpToolAnnotations {
    fn default() -> Self {
        Self {
            title: None,
            read_only_hint: false,
            destructive_hint: true,
            idempotent_hint: false,
            open_world_hint: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct McpToolMetadata {
    pub name: String,
    pub annotations: McpToolAnnotations,
    pub annotations_trusted: bool,
    pub classified_as_write: bool,
    pub configured_approval_mode: String,
    pub effective_approval_mode: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct McpAuthStatus {
    pub server_id: String,
    pub mode: McpAuthMode,
    pub state: String,
    pub login_supported: bool,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct McpOAuthCredentials {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    token_type: Option<String>,
    #[serde(default)]
    scope: Option<String>,
    #[serde(default)]
    expires_at_unix: Option<i64>,
    #[serde(default)]
    client_id: String,
    #[serde(default)]
    token_endpoint: String,
    #[serde(default)]
    authorization_server: String,
    #[serde(default)]
    resource: String,
}

#[derive(Debug, Deserialize)]
struct McpOAuthTokenResponse {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    token_type: Option<String>,
    #[serde(default)]
    scope: Option<String>,
    #[serde(default)]
    expires_in: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct McpProtectedResourceMetadata {
    #[serde(default)]
    resource: Option<String>,
    #[serde(default)]
    authorization_servers: Vec<String>,
    #[serde(default)]
    scopes_supported: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct McpAuthorizationServerMetadata {
    issuer: String,
    authorization_endpoint: String,
    token_endpoint: String,
    #[serde(default)]
    registration_endpoint: Option<String>,
    #[serde(default)]
    code_challenge_methods_supported: Vec<String>,
    #[serde(default)]
    client_id_metadata_document_supported: bool,
}

#[derive(Debug, Deserialize)]
struct McpDynamicRegistrationResponse {
    client_id: String,
    #[serde(default)]
    token_endpoint_auth_method: Option<String>,
}

#[derive(Debug)]
struct McpOAuthDiscovery {
    resource: String,
    challenged_scopes: Vec<String>,
    protected: McpProtectedResourceMetadata,
    authorization_server: String,
    metadata: McpAuthorizationServerMetadata,
}

#[derive(Debug)]
struct McpOAuthFlow {
    verifier: String,
    challenge: String,
    state: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpConnectionResult {
    pub server_id: String,
    pub server_name: String,
    pub protocol_version: String,
    pub server_capabilities: Value,
    pub server_instructions: Option<String>,
    pub server_info: Option<Value>,
    pub tools: Vec<McpToolDescriptor>,
    pub tool_metadata: Vec<McpToolMetadata>,
}

#[derive(Debug, Clone, PartialEq)]
struct McpInitializeResult {
    protocol_version: String,
    capabilities: Value,
    instructions: Option<String>,
    server_info: Option<Value>,
}

#[derive(Debug, Clone)]
struct ParsedMcpTool {
    descriptor: McpToolDescriptor,
    metadata: McpToolMetadata,
}

#[derive(Debug, Clone)]
struct ParsedMcpElicitation {
    mode: String,
    message: String,
    requested_schema: Option<Value>,
    url: Option<String>,
}

#[derive(Clone, Default)]
pub struct ExtensionManager {
    inner: Arc<Mutex<ExtensionManagerInner>>,
}

#[derive(Default)]
struct ExtensionManagerInner {
    app_data_dir: Option<PathBuf>,
    config: ExtensionConfig,
    crux_workspaces: HashMap<PathBuf, CruxWorkspaceRuntime>,
}

#[derive(Debug, Clone, Default)]
struct CruxWorkspaceRuntime {
    census: Option<CruxProjectCensus>,
    approved: bool,
    task_running: bool,
    force_reindex: bool,
}

impl ExtensionManager {
    pub fn set_app_data_dir(&self, path: PathBuf) -> ExtensionResult<()> {
        fs::create_dir_all(&path)
            .map_err(|error| format!("failed to create extension data directory: {error}"))?;
        // MCP headers and env values can hold secrets. An earlier release
        // created the file with the default mode.
        app_data::restrict_file(&path.join(CONFIG_FILE));
        let config = load_config(&path)?;
        let mut inner = self.inner.lock().expect("extension manager lock poisoned");
        inner.app_data_dir = Some(path);
        inner.config = config;
        Ok(())
    }

    fn read<T>(&self, read: impl FnOnce(&ExtensionConfig) -> T) -> T {
        read(
            &self
                .inner
                .lock()
                .expect("extension manager lock poisoned")
                .config,
        )
    }

    fn update<T>(
        &self,
        update: impl FnOnce(&mut ExtensionConfig, &Path) -> ExtensionResult<T>,
    ) -> ExtensionResult<T> {
        let mut inner = self.inner.lock().expect("extension manager lock poisoned");
        let data_dir = inner
            .app_data_dir
            .clone()
            .ok_or_else(|| "extension manager is not initialized".to_string())?;
        let value = update(&mut inner.config, &data_dir)?;
        save_config(&data_dir, &inner.config)?;
        Ok(value)
    }

    fn crux_census(&self, root: &Path) -> Option<CruxProjectCensus> {
        self.inner
            .lock()
            .expect("extension manager lock poisoned")
            .crux_workspaces
            .get(root)
            .and_then(|runtime| runtime.census.clone())
    }

    fn crux_approved(&self, root: &Path) -> bool {
        self.inner
            .lock()
            .expect("extension manager lock poisoned")
            .crux_workspaces
            .get(root)
            .is_some_and(|runtime| runtime.approved)
    }

    fn set_crux_approved(&self, root: &Path, approved: bool) {
        self.inner
            .lock()
            .expect("extension manager lock poisoned")
            .crux_workspaces
            .entry(root.to_path_buf())
            .or_default()
            .approved = approved;
    }

    fn crux_task_running(&self, root: &Path) -> bool {
        self.inner
            .lock()
            .expect("extension manager lock poisoned")
            .crux_workspaces
            .get(root)
            .is_some_and(|runtime| runtime.task_running)
    }

    fn begin_crux_task(&self, root: &Path, force_reindex: bool) -> bool {
        let mut inner = self.inner.lock().expect("extension manager lock poisoned");
        let runtime = inner.crux_workspaces.entry(root.to_path_buf()).or_default();
        runtime.force_reindex |= force_reindex;
        if runtime.task_running {
            return false;
        }
        runtime.task_running = true;
        true
    }

    fn set_crux_census(&self, root: &Path, census: CruxProjectCensus) {
        self.inner
            .lock()
            .expect("extension manager lock poisoned")
            .crux_workspaces
            .entry(root.to_path_buf())
            .or_default()
            .census = Some(census);
    }

    #[cfg(test)]
    pub(crate) fn set_crux_test_state(
        &self,
        root: &Path,
        activation: CruxActivation,
        builtin_enabled: bool,
        census: CruxProjectCensus,
    ) {
        let mut inner = self.inner.lock().expect("extension manager lock poisoned");
        inner.config.crux_activation = activation;
        inner
            .config
            .builtin_mcp_enabled
            .insert(BUILTIN_CRUX_ID.to_string(), builtin_enabled);
        let runtime = inner.crux_workspaces.entry(root.to_path_buf()).or_default();
        runtime.census = Some(census);
        runtime.approved = true;
    }

    fn take_crux_force_reindex(&self, root: &Path) -> bool {
        let mut inner = self.inner.lock().expect("extension manager lock poisoned");
        let runtime = inner.crux_workspaces.entry(root.to_path_buf()).or_default();
        std::mem::take(&mut runtime.force_reindex)
    }

    fn finish_crux_task(&self, root: &Path) -> bool {
        let mut inner = self.inner.lock().expect("extension manager lock poisoned");
        let runtime = inner.crux_workspaces.entry(root.to_path_buf()).or_default();
        runtime.task_running = false;
        runtime.force_reindex
    }
}

#[derive(Clone)]
struct ProjectContext {
    root: PathBuf,
    trusted: bool,
}

#[derive(Debug, Clone)]
struct ValidatedPlugin {
    id: String,
    name: String,
    version: String,
    description: String,
    display_name: String,
    developer_name: Option<String>,
    capabilities: Vec<String>,
}

#[derive(Debug, Clone)]
struct SkillUiMetadata {
    display_name: Option<String>,
    short_description: Option<String>,
    default_prompt: Option<String>,
    allow_implicit_invocation: bool,
    dependency_tools: Vec<String>,
}

impl Default for SkillUiMetadata {
    fn default() -> Self {
        Self {
            display_name: None,
            short_description: None,
            default_prompt: None,
            allow_implicit_invocation: true,
            dependency_tools: Vec::new(),
        }
    }
}

#[derive(Debug, Clone)]
struct SkillRoot {
    root: PathBuf,
    scope: String,
    source: String,
    plugin_id: Option<String>,
    plugin_relative_root: Option<String>,
}

#[tauri::command]
pub async fn extensions_snapshot(
    manager: tauri::State<'_, ExtensionManager>,
    storage: tauri::State<'_, Storage>,
    project_id: Option<i64>,
    workspace_id: Option<String>,
) -> ExtensionResult<ExtensionsSnapshot> {
    let project = project_context(storage.inner(), project_id, workspace_id.as_deref())?;
    let skills = discover_skills(manager.inner(), project.as_ref())?;
    let mut config = manager.read(Clone::clone);
    config.plugins = config
        .plugins
        .iter()
        .map(plugin_snapshot_descriptor)
        .collect::<ExtensionResult<Vec<_>>>()?;
    let root = project.as_ref().map(|project| project.root.as_path());
    let crux = crux_effective_context(manager.inner(), root);
    let crux_census = root.and_then(|root| manager.crux_census(root));
    let crux_index_status = crux_index_status(
        manager.inner(),
        storage.inner(),
        &config,
        project_id,
        root,
        crux_census.as_ref(),
    )?;
    let mut mcp_servers = effective_mcp_servers(&config, crux.as_ref())?;
    let mcp_auth = mcp_servers.iter().map(mcp_auth_status).collect();
    for server in &mut mcp_servers {
        server.env.clear();
    }
    Ok(ExtensionsSnapshot {
        skills,
        plugins: config.plugins.clone(),
        available_plugins: discover_codex_plugin_candidates(&config),
        mcp_servers,
        mcp_auth,
        crux_activation: config.crux_activation,
        crux_enabled: builtin_mcp_enabled(&config, BUILTIN_CRUX_ID),
        crux_census,
        crux_index_status,
    })
}

#[tauri::command]
pub async fn crux_workspace_open(
    app: tauri::AppHandle,
    manager: tauri::State<'_, ExtensionManager>,
    storage: tauri::State<'_, Storage>,
    project_id: i64,
    workspace_id: Option<String>,
) -> ExtensionResult<CruxWorkspaceOpenResult> {
    let root = crux_command_root(storage.inner(), project_id, workspace_id.as_deref())?;
    let approval_required =
        crux_approval_required(manager.inner(), storage.inner(), project_id, &root).await;
    schedule_crux_workspace(app, project_id, root, false);
    Ok(CruxWorkspaceOpenResult { approval_required })
}

#[tauri::command]
pub async fn crux_approve_indexing(
    app: tauri::AppHandle,
    manager: tauri::State<'_, ExtensionManager>,
    storage: tauri::State<'_, Storage>,
    project_id: i64,
    workspace_id: Option<String>,
) -> ExtensionResult<()> {
    let root = crux_command_root(storage.inner(), project_id, workspace_id.as_deref())?;
    manager.update(|config, _| {
        config.crux_approved_projects.insert(project_id);
        Ok(())
    })?;
    // A census task can run for this root now. Force a rerun so that the
    // index starts after the approval.
    schedule_crux_workspace(app, project_id, root, true);
    Ok(())
}

#[tauri::command]
pub async fn crux_reindex_now(
    app: tauri::AppHandle,
    storage: tauri::State<'_, Storage>,
    project_id: i64,
    workspace_id: Option<String>,
) -> ExtensionResult<()> {
    let root = crux_command_root(storage.inner(), project_id, workspace_id.as_deref())?;
    schedule_crux_workspace(app, project_id, root, true);
    Ok(())
}

#[tauri::command]
pub async fn crux_set_activation(
    app: tauri::AppHandle,
    manager: tauri::State<'_, ExtensionManager>,
    storage: tauri::State<'_, Storage>,
    activation: CruxActivation,
    project_id: Option<i64>,
    workspace_id: Option<String>,
) -> ExtensionResult<()> {
    manager.update(|config, _| {
        config.crux_activation = activation;
        Ok(())
    })?;
    if let Some(project_id) = project_id {
        let root = crux_command_root(storage.inner(), project_id, workspace_id.as_deref())?;
        schedule_crux_workspace(app, project_id, root, false);
    }
    Ok(())
}

#[tauri::command]
pub async fn skills_set_enabled(
    manager: tauri::State<'_, ExtensionManager>,
    skill_id: String,
    enabled: bool,
) -> ExtensionResult<()> {
    manager.update(|config, _| {
        config.skill_enabled.insert(skill_id, enabled);
        Ok(())
    })
}

#[tauri::command]
pub async fn skill_read(
    manager: tauri::State<'_, ExtensionManager>,
    storage: tauri::State<'_, Storage>,
    skill_id: String,
    project_id: Option<i64>,
    workspace_id: Option<String>,
) -> ExtensionResult<String> {
    let project = project_context(storage.inner(), project_id, workspace_id.as_deref())?;
    let skill = discover_skills(manager.inner(), project.as_ref())?
        .into_iter()
        .find(|skill| skill.id == skill_id)
        .ok_or_else(|| "skill is unavailable".to_string())?;
    read_skill(Path::new(&skill.root))
}

#[tauri::command]
pub async fn plugins_install_local(
    manager: tauri::State<'_, ExtensionManager>,
    path: String,
) -> ExtensionResult<PluginDescriptor> {
    let source = canonical_directory(Path::new(path.trim()))?;
    let validated = validate_plugin(&source)?;
    parse_plugin_mcp_servers(&source, &validated.id, &validated.display_name, true)?;
    manager.update(|config, data_dir| {
        let destination = data_dir.join("plugins").join(&validated.id).join(format!(
            "{}-{}",
            validated.version,
            short_hash(&source.to_string_lossy())
        ));
        if destination.exists() {
            fs::remove_dir_all(&destination)
                .map_err(|error| format!("failed to replace plugin: {error}"))?;
        }
        if let Err(error) = copy_plugin(&source, &destination) {
            let _ = fs::remove_dir_all(&destination);
            return Err(error);
        }
        let mcp_servers = match parse_plugin_mcp_servers(
            &destination,
            &validated.id,
            &validated.display_name,
            true,
        ) {
            Ok(servers) => servers,
            Err(error) => {
                let _ = fs::remove_dir_all(&destination);
                return Err(error);
            }
        };
        let skill_count = plugin_skill_roots(&destination, &validated.id, &validated.display_name)?
            .iter()
            .map(|root| count_skill_directories(&root.root))
            .sum();
        let descriptor = PluginDescriptor {
            id: validated.id.clone(),
            name: validated.name.clone(),
            version: validated.version.clone(),
            root: destination.to_string_lossy().into_owned(),
            enabled: true,
            installed_at: chrono::Utc::now().to_rfc3339(),
            contributes_skills: skill_count > 0,
            contributes_mcp: !mcp_servers.is_empty(),
            unsupported_components: unsupported_components(&destination),
            description: validated.description.clone(),
            display_name: validated.display_name.clone(),
            developer_name: validated.developer_name.clone(),
            capabilities: validated.capabilities.clone(),
            skill_count,
            mcp_server_count: mcp_servers.len(),
        };
        let previous = config
            .plugins
            .iter()
            .find(|plugin| plugin.id == validated.id)
            .cloned();
        config.plugins.retain(|plugin| plugin.id != validated.id);
        config.plugins.push(descriptor.clone());
        config.plugins.sort_by(|a, b| a.name.cmp(&b.name));
        if let Some(previous) = previous {
            let previous_root = PathBuf::from(previous.root);
            if previous_root != destination
                && previous_root.starts_with(data_dir.join("plugins"))
                && previous_root.exists()
            {
                let _ = fs::remove_dir_all(previous_root);
            }
        }
        Ok(descriptor)
    })
}

#[tauri::command]
pub async fn plugins_set_enabled(
    manager: tauri::State<'_, ExtensionManager>,
    plugin_id: String,
    enabled: bool,
) -> ExtensionResult<PluginDescriptor> {
    manager.update(|config, _| {
        let plugin = config
            .plugins
            .iter_mut()
            .find(|plugin| plugin.id == plugin_id)
            .ok_or_else(|| "plugin was not found".to_string())?;
        plugin.enabled = enabled;
        Ok(plugin.clone())
    })
}

#[tauri::command]
pub async fn plugins_uninstall(
    manager: tauri::State<'_, ExtensionManager>,
    plugin_id: String,
) -> ExtensionResult<()> {
    manager.update(|config, data_dir| {
        let index = config
            .plugins
            .iter()
            .position(|plugin| plugin.id == plugin_id)
            .ok_or_else(|| "plugin was not found".to_string())?;
        let plugin = config.plugins.remove(index);
        let root = PathBuf::from(&plugin.root);
        for server in
            parse_plugin_mcp_servers(&root, &plugin.id, plugin_display_name(&plugin), false)?
        {
            clear_mcp_bearer_token(&server.id)?;
            clear_mcp_oauth_token(&server.id)?;
        }
        if root.starts_with(data_dir.join("plugins")) && root.exists() {
            fs::remove_dir_all(root)
                .map_err(|error| format!("failed to remove plugin files: {error}"))?;
        }
        Ok(())
    })
}

#[tauri::command]
pub async fn mcp_server_upsert(
    manager: tauri::State<'_, ExtensionManager>,
    mut server: McpServerConfig,
) -> ExtensionResult<McpServerConfig> {
    ensure_mcp_server_editable(server.id.trim())?;
    server.source = McpServerSource::User;
    server.plugin_id = None;
    server.plugin_name = None;
    if !server.id.trim().is_empty()
        && find_plugin_mcp_server(manager.inner(), server.id.trim())?.is_some()
    {
        return Err("MCP server id conflicts with a plugin-provided server".to_string());
    }
    validate_mcp(&mut server)?;
    manager.update(|config, _| {
        config.mcp_servers.retain(|item| item.id != server.id);
        config.mcp_servers.push(server.clone());
        config.mcp_servers.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(server)
    })
}

#[tauri::command]
pub async fn mcp_server_remove(
    manager: tauri::State<'_, ExtensionManager>,
    server_id: String,
) -> ExtensionResult<()> {
    ensure_mcp_server_removable(&server_id)?;
    if find_plugin_mcp_server(manager.inner(), &server_id)?.is_some() {
        return Err("plugin-provided MCP servers are removed with their plugin".to_string());
    }
    manager.update(|config, _| {
        if !config
            .mcp_servers
            .iter()
            .any(|server| server.id == server_id)
        {
            return Err("MCP server was not found".to_string());
        }
        clear_mcp_bearer_token(&server_id)?;
        clear_mcp_oauth_token(&server_id)?;
        config.mcp_servers.retain(|server| server.id != server_id);
        Ok(())
    })
}

#[tauri::command]
pub async fn mcp_server_set_bearer_token(
    manager: tauri::State<'_, ExtensionManager>,
    server_id: String,
    token: String,
) -> ExtensionResult<()> {
    let exists = find_effective_mcp_server(manager.inner(), &server_id, None)?
        .is_some_and(|server| matches!(server.transport, McpTransport::StreamableHttp));
    if !exists {
        return Err("HTTP MCP server was not found".to_string());
    }
    let token = token.trim();
    if token.is_empty() || token.len() > 16 * 1024 {
        return Err("MCP bearer token is empty or too large".to_string());
    }
    write_mcp_bearer_token(&server_id, token)
}

#[tauri::command]
pub async fn mcp_server_clear_bearer_token(
    manager: tauri::State<'_, ExtensionManager>,
    server_id: String,
) -> ExtensionResult<()> {
    let exists = find_effective_mcp_server(manager.inner(), &server_id, None)?.is_some();
    if !exists {
        return Err("MCP server was not found".to_string());
    }
    clear_mcp_bearer_token(&server_id)
}

#[tauri::command]
pub async fn mcp_server_oauth_login(
    manager: tauri::State<'_, ExtensionManager>,
    server_id: String,
) -> ExtensionResult<McpAuthStatus> {
    let server = find_effective_mcp_server(manager.inner(), &server_id, None)?
        .ok_or_else(|| "MCP server was not found".to_string())?;
    if !matches!(server.transport, McpTransport::StreamableHttp)
        || !matches!(server.auth_mode, McpAuthMode::Oauth)
    {
        return Err("OAuth login is available only for OAuth HTTP MCP servers".to_string());
    }
    let credentials = run_mcp_oauth_login(&server).await?;
    store_mcp_oauth_credentials(&server.id, &credentials)?;
    Ok(mcp_auth_status(&server))
}

#[tauri::command]
pub async fn mcp_server_oauth_logout(
    manager: tauri::State<'_, ExtensionManager>,
    server_id: String,
) -> ExtensionResult<McpAuthStatus> {
    let server = find_effective_mcp_server(manager.inner(), &server_id, None)?
        .ok_or_else(|| "MCP server was not found".to_string())?;
    if !matches!(server.auth_mode, McpAuthMode::Oauth) {
        return Err("MCP server is not configured for OAuth".to_string());
    }
    clear_mcp_oauth_token(&server.id)?;
    Ok(mcp_auth_status(&server))
}

#[tauri::command]
pub async fn mcp_server_set_enabled(
    app: tauri::AppHandle,
    manager: tauri::State<'_, ExtensionManager>,
    storage: tauri::State<'_, Storage>,
    server_id: String,
    enabled: bool,
    project_id: Option<i64>,
    workspace_id: Option<String>,
) -> ExtensionResult<McpServerConfig> {
    if server_id == BUILTIN_CRUX_ID {
        manager.update(|config, _| {
            config
                .builtin_mcp_enabled
                .insert(BUILTIN_CRUX_ID.to_string(), enabled);
            Ok(())
        })?;
        if let Some(project_id) = project_id {
            let root = crux_command_root(storage.inner(), project_id, workspace_id.as_deref())?;
            schedule_crux_workspace(app, project_id, root.clone(), false);
            let binary = locate_crux_binary()
                .ok_or_else(|| "bundled Crux binary is unavailable".to_string())?;
            return Ok(builtin_crux_server(&binary, &root, enabled));
        }
        let binary =
            locate_crux_binary().ok_or_else(|| "bundled Crux binary is unavailable".to_string())?;
        return Ok(builtin_crux_server(&binary, Path::new("."), enabled));
    }
    if find_plugin_mcp_server(manager.inner(), &server_id)?.is_some() {
        return Err(
            "plugin-provided MCP servers are controlled by their plugin enablement".to_string(),
        );
    }
    manager.update(|config, _| {
        let server = config
            .mcp_servers
            .iter_mut()
            .find(|server| server.id == server_id)
            .ok_or_else(|| "MCP server was not found".to_string())?;
        server.enabled = enabled;
        Ok(server.clone())
    })
}

#[tauri::command]
pub async fn mcp_server_test(
    manager: tauri::State<'_, ExtensionManager>,
    storage: tauri::State<'_, Storage>,
    server_id: String,
    project_id: Option<i64>,
    workspace_id: Option<String>,
) -> ExtensionResult<McpConnectionResult> {
    let root = project_id
        .map(|project_id| crux_command_root(storage.inner(), project_id, workspace_id.as_deref()))
        .transpose()?;
    let server = find_effective_mcp_server(manager.inner(), &server_id, root.as_deref())?
        .ok_or_else(|| "MCP server was not found".to_string())?;
    connect_and_list_tools(&server).await
}

pub async fn enabled_mcp_tools(
    manager: &ExtensionManager,
    project_root: Option<&Path>,
    plan_mode: bool,
) -> ExtensionResult<Vec<McpToolDescriptor>> {
    let config = manager.read(Clone::clone);
    let crux = crux_effective_context(manager, project_root);
    let servers = effective_mcp_servers(&config, crux.as_ref())?
        .into_iter()
        .filter(|server| server.enabled && (!plan_mode || server.id == BUILTIN_CRUX_ID))
        .collect::<Vec<_>>();
    let mut tools = Vec::new();
    for server in servers {
        match connect_and_list_tools(&server).await {
            Ok(result) => {
                for tool in result.tools {
                    if tools.len() >= MAX_MCP_TOOLS {
                        return Err("enabled MCP tools exceed the safety limit".to_string());
                    }
                    tools.push(tool);
                }
            }
            Err(error) if server.required => {
                return Err(format!(
                    "required MCP server `{}` failed: {error}",
                    server.name
                ));
            }
            Err(error) => {
                eprintln!(
                    "optional MCP server `{}` is unavailable: {error}",
                    server.name
                );
            }
        }
    }
    Ok(tools)
}

pub async fn call_mcp_tool(
    manager: &ExtensionManager,
    descriptor: &McpToolDescriptor,
    arguments: Value,
    elicitation_sender: Option<McpElicitationSender>,
    project_root: Option<&Path>,
) -> ExtensionResult<McpToolCallResult> {
    let server = find_effective_mcp_server(manager, &descriptor.server_id, project_root)?
        .filter(|server| server.enabled)
        .ok_or_else(|| "MCP server is disabled or was removed".to_string())?;
    let mut arguments = arguments;
    if descriptor.server_id == BUILTIN_CRUX_ID {
        let project_root =
            project_root.ok_or_else(|| "Crux requires an active project".to_string())?;
        let object = arguments
            .as_object_mut()
            .ok_or_else(|| "Crux tool arguments must be an object".to_string())?;
        object.insert(
            "project_root".to_string(),
            Value::String(project_root.to_string_lossy().into_owned()),
        );
    }
    let result =
        connect_and_call_tool(&server, &descriptor.name, arguments, elicitation_sender).await?;
    let encoded = serde_json::to_string(&result)
        .map_err(|error| format!("failed to encode MCP tool result: {error}"))?;
    if encoded.len() > MAX_MCP_MESSAGE_BYTES {
        return Err("MCP tool result exceeded the safety limit".to_string());
    }
    normalize_mcp_tool_result(&result)
}

#[derive(Debug, Clone)]
struct CruxEffectiveContext {
    binary: PathBuf,
    approved: bool,
    root: PathBuf,
    census: Option<CruxProjectCensus>,
}

fn effective_mcp_servers(
    config: &ExtensionConfig,
    crux: Option<&CruxEffectiveContext>,
) -> ExtensionResult<Vec<McpServerConfig>> {
    let mut servers = config.mcp_servers.clone();
    for server in &mut servers {
        server.source = McpServerSource::User;
        server.plugin_id = None;
        server.plugin_name = None;
    }
    for plugin in &config.plugins {
        let mut bundled = parse_plugin_mcp_servers(
            Path::new(&plugin.root),
            &plugin.id,
            plugin_display_name(plugin),
            true,
        )?;
        for server in &mut bundled {
            server.enabled &= plugin.enabled;
        }
        servers.extend(bundled);
    }
    let mut seen = std::collections::HashSet::new();
    for server in &servers {
        if !seen.insert(server.id.clone()) {
            return Err(format!("duplicate MCP server id `{}`", server.id));
        }
        if server.id == BUILTIN_CRUX_ID {
            return Err("MCP server id `builtin.crux` is reserved".to_string());
        }
    }
    servers.sort_by(|left, right| {
        left.name
            .to_lowercase()
            .cmp(&right.name.to_lowercase())
            .then_with(|| left.id.cmp(&right.id))
    });
    if let Some(crux) = crux.filter(|crux| crux_context_enabled(config, crux)) {
        servers.insert(0, builtin_crux_server(&crux.binary, &crux.root, true));
    }
    Ok(servers)
}

fn find_effective_mcp_server(
    manager: &ExtensionManager,
    server_id: &str,
    project_root: Option<&Path>,
) -> ExtensionResult<Option<McpServerConfig>> {
    let config = manager.read(Clone::clone);
    let crux = crux_effective_context(manager, project_root);
    Ok(effective_mcp_servers(&config, crux.as_ref())?
        .into_iter()
        .find(|server| server.id == server_id))
}

fn builtin_mcp_enabled(config: &ExtensionConfig, server_id: &str) -> bool {
    config
        .builtin_mcp_enabled
        .get(server_id)
        .copied()
        .unwrap_or(true)
}

pub(crate) fn crux_enabled_for_project(
    manager: Option<&ExtensionManager>,
    project_root: Option<&Path>,
) -> bool {
    let Some(manager) = manager else {
        eprintln!("Crux was disabled because the extension manager was unavailable.");
        return false;
    };
    let config = manager.read(Clone::clone);
    crux_effective_context(manager, project_root)
        .as_ref()
        .is_some_and(|crux| crux_context_enabled(&config, crux))
}

fn crux_context_enabled(config: &ExtensionConfig, crux: &CruxEffectiveContext) -> bool {
    // An agent can call scip_index without a prompt, so an unapproved
    // project gets no Crux server.
    crux.binary.is_file()
        && crux.approved
        && builtin_mcp_enabled(config, BUILTIN_CRUX_ID)
        && crux_gate_passes(config.crux_activation, crux.census.as_ref())
}

fn crux_effective_context(
    manager: &ExtensionManager,
    project_root: Option<&Path>,
) -> Option<CruxEffectiveContext> {
    let root = project_root?.to_path_buf();
    let binary = locate_crux_binary_with_diagnostic()?;
    Some(CruxEffectiveContext {
        binary,
        approved: manager.crux_approved(&root),
        census: manager.crux_census(&root),
        root,
    })
}

fn crux_gate_passes(activation: CruxActivation, census: Option<&CruxProjectCensus>) -> bool {
    match activation {
        CruxActivation::AlwaysOn => true,
        CruxActivation::AlwaysOff => false,
        CruxActivation::Auto => {
            census.is_some_and(|census| census.languages.values().any(crux_language_qualifies))
        }
    }
}

fn crux_language_qualifies(language: &CruxLanguageCensus) -> bool {
    language.files >= CRUX_FILE_THRESHOLD || language.lines >= CRUX_LINE_THRESHOLD
}

fn crux_indexing_approved(
    config: &ExtensionConfig,
    storage: &Storage,
    project_id: i64,
    root: &Path,
) -> bool {
    if config.crux_approved_projects.contains(&project_id) {
        return true;
    }
    // Halv writes an index state only after it indexed that root, and a
    // repository cannot write it. Thus an earlier index counts as approval.
    let project_root = storage
        .get_project(project_id)
        .ok()
        .map(|project| project.root_path);
    std::iter::once(root.to_string_lossy().into_owned())
        .chain(project_root)
        .any(|path| matches!(storage.crux_index_state(&path), Ok(Some(_))))
}

// Halv calls this before it removes a project. SQLite can give the id of a
// removed project to a new project, so the approval must end here. Storage
// deletes the Crux index states of the project with the project row.
pub(crate) fn crux_revoke_project_approval(
    manager: &ExtensionManager,
    project_id: i64,
    roots: &[PathBuf],
) -> ExtensionResult<()> {
    if manager.read(|config| config.crux_approved_projects.contains(&project_id)) {
        manager.update(|config, _| {
            config.crux_approved_projects.remove(&project_id);
            Ok(())
        })?;
    }
    for root in roots {
        manager.set_crux_approved(root, false);
    }
    Ok(())
}

// A Crux task calls this after its census. The function takes the force flag
// before it reads the approval. An approval that comes after this call sets
// the flag again, and the task then runs one more time.
fn crux_task_approval(
    manager: &ExtensionManager,
    storage: &Storage,
    project_id: i64,
    root: &Path,
) -> (ExtensionConfig, bool) {
    let _ = manager.take_crux_force_reindex(root);
    let config = manager.read(Clone::clone);
    let approved = crux_indexing_approved(&config, storage, project_id, root);
    manager.set_crux_approved(root, approved);
    (config, approved)
}

fn crux_should_index(config: &ExtensionConfig, census: &CruxProjectCensus, approved: bool) -> bool {
    approved
        && builtin_mcp_enabled(config, BUILTIN_CRUX_ID)
        && crux_gate_passes(config.crux_activation, Some(census))
}

// The UI asks for approval only when Crux would index the project.
async fn crux_approval_required(
    manager: &ExtensionManager,
    storage: &Storage,
    project_id: i64,
    root: &Path,
) -> bool {
    let config = manager.read(Clone::clone);
    if !builtin_mcp_enabled(&config, BUILTIN_CRUX_ID)
        || config.crux_activation == CruxActivation::AlwaysOff
        || crux_indexing_approved(&config, storage, project_id, root)
    {
        return false;
    }
    let census = match manager.crux_census(root) {
        Some(census) => census,
        None => {
            let census_root = root.to_path_buf();
            let census = tauri::async_runtime::spawn_blocking(move || {
                build_crux_project_census(&census_root)
            })
            .await
            .unwrap_or_default();
            manager.set_crux_census(root, census.clone());
            census
        }
    };
    crux_should_index(&config, &census, true)
}

fn crux_index_status(
    manager: &ExtensionManager,
    storage: &Storage,
    config: &ExtensionConfig,
    project_id: Option<i64>,
    root: Option<&Path>,
    census: Option<&CruxProjectCensus>,
) -> ExtensionResult<CruxIndexStatus> {
    let enabled = builtin_mcp_enabled(config, BUILTIN_CRUX_ID);
    if !enabled || config.crux_activation == CruxActivation::AlwaysOff {
        return Ok(CruxIndexStatus {
            state: CruxIndexStatusState::Off,
            document_count: None,
        });
    }
    let Some(root) = root else {
        return Ok(CruxIndexStatus {
            state: CruxIndexStatusState::WaitingForIndex,
            document_count: None,
        });
    };
    if manager.crux_task_running(root) {
        return Ok(CruxIndexStatus {
            state: CruxIndexStatusState::Indexing,
            document_count: None,
        });
    }
    if !crux_gate_passes(config.crux_activation, census) {
        return Ok(CruxIndexStatus {
            state: if census.is_some() {
                CruxIndexStatusState::Off
            } else {
                CruxIndexStatusState::WaitingForIndex
            },
            document_count: None,
        });
    }
    if !project_id
        .is_some_and(|project_id| crux_indexing_approved(config, storage, project_id, root))
    {
        return Ok(CruxIndexStatus {
            state: CruxIndexStatusState::ApprovalRequired,
            document_count: None,
        });
    }
    let project_path = root.to_string_lossy();
    let persisted = storage
        .crux_index_state(&project_path)
        .map_err(|error| format!("failed to load Crux index state: {error}"))?;
    if root.join(CRUX_INDEX_RELATIVE_PATH).is_file() {
        if let Some(persisted) = persisted {
            return Ok(CruxIndexStatus {
                state: CruxIndexStatusState::Indexed,
                document_count: crux_document_count(&persisted.last_index_stats),
            });
        }
    }
    Ok(CruxIndexStatus {
        state: CruxIndexStatusState::WaitingForIndex,
        document_count: None,
    })
}

fn crux_document_count(stats: &str) -> Option<u64> {
    serde_json::from_str::<Value>(stats)
        .ok()
        .as_ref()
        .and_then(crux_document_count_from_value)
        .or_else(|| crux_document_count_from_text(stats))
}

fn crux_document_count_from_value(value: &Value) -> Option<u64> {
    match value {
        Value::Object(values) => values
            .iter()
            .find_map(|(key, value)| {
                matches!(
                    key.as_str(),
                    "documents" | "document_count" | "documentCount"
                )
                .then(|| value.as_u64())
                .flatten()
            })
            .or_else(|| values.values().find_map(crux_document_count_from_value)),
        Value::Array(values) => values.iter().find_map(crux_document_count_from_value),
        Value::String(value) => crux_document_count_from_text(value),
        _ => None,
    }
}

fn crux_document_count_from_text(text: &str) -> Option<u64> {
    let lower = text.to_ascii_lowercase();
    let suffix = text.get(lower.find("documents")? + "documents".len()..)?;
    let digits = suffix
        .trim_start_matches(|character: char| {
            character.is_whitespace() || matches!(character, ':' | '=')
        })
        .chars()
        .take_while(|character| character.is_ascii_digit() || *character == ',')
        .filter(|character| *character != ',')
        .collect::<String>();
    (!digits.is_empty()).then(|| digits.parse().ok()).flatten()
}

pub(crate) fn crux_savings_estimate(server_id: &str, tool_name: &str) -> Option<u64> {
    (matches!(server_id, BUILTIN_CRUX_ID | TERMINAL_CRUX_ID))
        .then(|| {
            CRUX_SAVINGS_ESTIMATES
                .iter()
                .find_map(|(name, estimate)| (*name == tool_name).then_some(*estimate))
        })
        .flatten()
}

fn builtin_crux_server(binary: &Path, root: &Path, enabled: bool) -> McpServerConfig {
    McpServerConfig {
        id: BUILTIN_CRUX_ID.to_string(),
        name: BUILTIN_CRUX_NAME.to_string(),
        transport: McpTransport::Stdio,
        command: Some(binary.to_string_lossy().into_owned()),
        args: vec!["--profile".to_string(), "slim".to_string()],
        cwd: Some(root.to_string_lossy().into_owned()),
        env: BTreeMap::from([("PATH".to_string(), crux_path_environment())]),
        env_vars: Vec::new(),
        url: None,
        headers: BTreeMap::new(),
        env_http_headers: BTreeMap::new(),
        bearer_token_env_var: None,
        auth_mode: McpAuthMode::None,
        oauth_scopes: Vec::new(),
        oauth_client_id: None,
        enabled,
        required: false,
        startup_timeout_ms: 30_000,
        tool_timeout_ms: 600_000,
        enabled_tools: CRUX_TOOLS.iter().map(|tool| (*tool).to_string()).collect(),
        disabled_tools: Vec::new(),
        approval_mode: "approve".to_string(),
        tool_approval_modes: BTreeMap::new(),
        trust_tool_annotations: true,
        source: McpServerSource::BuiltIn,
        plugin_id: None,
        plugin_name: None,
    }
}

fn ensure_mcp_server_removable(server_id: &str) -> ExtensionResult<()> {
    if server_id == BUILTIN_CRUX_ID {
        return Err("built-in MCP servers cannot be removed".to_string());
    }
    Ok(())
}

fn ensure_mcp_server_editable(server_id: &str) -> ExtensionResult<()> {
    if server_id == BUILTIN_CRUX_ID {
        return Err("built-in MCP servers cannot be edited".to_string());
    }
    Ok(())
}

fn crux_command_root(
    storage: &Storage,
    project_id: i64,
    workspace_id: Option<&str>,
) -> ExtensionResult<PathBuf> {
    project_context(storage, Some(project_id), workspace_id)?
        .map(|project| project.root)
        .ok_or_else(|| "Crux requires an active project".to_string())
}

fn schedule_crux_workspace(
    app: tauri::AppHandle,
    project_id: i64,
    root: PathBuf,
    force_reindex: bool,
) {
    let manager = app.state::<ExtensionManager>().inner().clone();
    if !manager.begin_crux_task(&root, force_reindex) {
        return;
    }
    tauri::async_runtime::spawn(async move {
        let census_root = root.clone();
        let census = match tauri::async_runtime::spawn_blocking(move || {
            build_crux_project_census(&census_root)
        })
        .await
        {
            Ok(census) => census,
            Err(error) => {
                eprintln!("Crux census failed for {}: {error}", root.display());
                CruxProjectCensus::default()
            }
        };
        manager.set_crux_census(&root, census.clone());

        let (config, approved) =
            crux_task_approval(&manager, app.state::<Storage>().inner(), project_id, &root);
        if crux_should_index(&config, &census, approved) {
            if let Some(binary) = locate_crux_binary_with_diagnostic() {
                let languages = crux_qualifying_languages(&census);
                if let Err(error) = index_crux_workspace(&app, &root, &binary, &languages).await {
                    eprintln!(
                        "Crux background index failed for {}: {error}",
                        root.display()
                    );
                }
            }
        }

        let rerun = manager.finish_crux_task(&root);
        if rerun {
            schedule_crux_workspace(app, project_id, root, true);
        }
    });
}

async fn index_crux_workspace(
    app: &tauri::AppHandle,
    root: &Path,
    binary: &Path,
    languages: &[&str],
) -> ExtensionResult<()> {
    let project_path = root.to_string_lossy().into_owned();
    let git_head = crux_git_head(root);
    let index_path = root.join(CRUX_INDEX_RELATIVE_PATH);

    let server = builtin_crux_server(binary, root, true);
    let arguments = crux_index_arguments(&project_path, languages);
    let raw = connect_and_call_tool(&server, "scip_index", arguments, None).await?;
    let result = normalize_mcp_tool_result(&raw)?;
    if result.is_error {
        return Err(format!("scip_index returned an error: {}", result.text));
    }
    if !index_path.is_file() {
        return Err("scip_index completed without creating .scip-nav/index.scip".to_string());
    }
    let stats = serde_json::to_string(raw.get("structuredContent").unwrap_or(&raw))
        .unwrap_or(result.text)
        .chars()
        .take(CRUX_INDEX_STATS_MAX_CHARS)
        .collect::<String>();
    app.state::<Storage>()
        .upsert_crux_index_state(&CruxIndexState {
            project_path,
            last_indexed_git_head: git_head,
            last_index_stats: stats,
            last_index_time: Utc::now().to_rfc3339(),
        })
        .map_err(|error| format!("failed to store Crux index state: {error}"))
}

fn crux_index_arguments(project_path: &str, languages: &[&str]) -> serde_json::Value {
    if languages.is_empty() {
        json!({ "project_root": project_path })
    } else {
        json!({ "project_root": project_path, "languages": languages })
    }
}

fn crux_language_name(family: &str) -> Option<&'static str> {
    match family {
        "typescript_javascript" => Some("typescript"),
        "python" => Some("python"),
        "rust" => Some("rust"),
        "dart" => Some("dart"),
        "java_kotlin" => Some("java"),
        "c_cpp" => Some("cpp"),
        _ => None,
    }
}

fn crux_qualifying_languages(census: &CruxProjectCensus) -> Vec<&'static str> {
    let mut languages = census
        .languages
        .iter()
        .filter(|(_, language)| crux_language_qualifies(language))
        .filter_map(|(family, census)| {
            crux_language_name(family).map(|language| (language, census))
        })
        .collect::<Vec<_>>();

    languages.sort_by_key(|(_, census)| std::cmp::Reverse((census.lines, census.files)));
    languages
        .into_iter()
        .map(|(language, _)| language)
        .collect()
}

fn crux_git_head(root: &Path) -> Option<String> {
    let output = exec::managed_git_command(root)
        .ok()?
        .args(["rev-parse", "--verify", "HEAD"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let head = String::from_utf8(output.stdout).ok()?;
    let head = head.trim();
    (!head.is_empty()).then(|| head.to_string())
}

fn build_crux_project_census(root: &Path) -> CruxProjectCensus {
    let files = projects::git_project_files(root)
        .map(|files| files.into_iter().map(PathBuf::from).collect())
        .unwrap_or_else(|| fallback_crux_project_files(root));
    let mut census = CruxProjectCensus::default();
    for relative in files {
        let Some(language) = crux_language_for_path(&relative) else {
            continue;
        };
        let path = root.join(&relative);
        let Some(lines) = count_source_lines(&path) else {
            continue;
        };
        let language = census.languages.entry(language.to_string()).or_default();
        language.files = language.files.saturating_add(1);
        language.lines = language.lines.saturating_add(lines);
    }
    census
}

fn crux_language_for_path(path: &Path) -> Option<&'static str> {
    let extension = path.extension()?.to_str()?.to_ascii_lowercase();
    match extension.as_str() {
        "ts" | "tsx" | "js" | "jsx" => Some("typescript_javascript"),
        "py" => Some("python"),
        "rs" => Some("rust"),
        "dart" => Some("dart"),
        "java" | "kt" => Some("java_kotlin"),
        "c" | "cc" | "cpp" | "h" => Some("c_cpp"),
        _ => None,
    }
}

fn count_source_lines(path: &Path) -> Option<u64> {
    let file = fs::File::open(path).ok()?;
    let mut reader = StdBufReader::new(file);
    let mut buffer = Vec::new();
    let mut lines = 0_u64;
    loop {
        buffer.clear();
        let read = reader.read_until(b'\n', &mut buffer).ok()?;
        if read == 0 {
            return Some(lines);
        }
        lines = lines.saturating_add(1);
    }
}

fn fallback_crux_project_files(root: &Path) -> Vec<PathBuf> {
    let patterns = fs::read_to_string(root.join(".gitignore"))
        .ok()
        .map(|contents| parse_simple_gitignore(&contents))
        .unwrap_or_default();
    let mut files = Vec::new();
    walk_crux_project_files(root, root, &patterns, &mut files);
    files
}

fn walk_crux_project_files(
    root: &Path,
    directory: &Path,
    patterns: &[SimpleGitignorePattern],
    files: &mut Vec<PathBuf>,
) {
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    let mut entries = entries.flatten().collect::<Vec<_>>();
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_symlink() {
            continue;
        }
        let path = entry.path();
        let Ok(relative) = path.strip_prefix(root) else {
            continue;
        };
        if relative == Path::new(".git")
            || simple_gitignore_matches(relative, file_type.is_dir(), patterns)
        {
            continue;
        }
        if file_type.is_dir() {
            walk_crux_project_files(root, &path, patterns, files);
        } else if file_type.is_file() {
            files.push(relative.to_path_buf());
        }
    }
}

#[derive(Debug, Clone)]
struct SimpleGitignorePattern {
    value: String,
    negated: bool,
    directory_only: bool,
}

fn parse_simple_gitignore(contents: &str) -> Vec<SimpleGitignorePattern> {
    contents
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                return None;
            }
            let negated = line.starts_with('!');
            let value = line.trim_start_matches('!').trim_start_matches('/');
            let directory_only = value.ends_with('/');
            let value = value.trim_end_matches('/');
            (!value.is_empty()).then(|| SimpleGitignorePattern {
                value: value.to_string(),
                negated,
                directory_only,
            })
        })
        .collect()
}

// The fallback reads only root .gitignore rules. It cannot restore files in skipped directories.
fn simple_gitignore_matches(
    path: &Path,
    is_directory: bool,
    patterns: &[SimpleGitignorePattern],
) -> bool {
    let path = path.to_string_lossy().replace('\\', "/");
    let mut ignored = false;
    for pattern in patterns {
        let matches = if pattern.value.contains('/') {
            simple_wildcard_match(&pattern.value, &path)
                || path.starts_with(&format!("{}/", pattern.value))
        } else {
            path.split('/').enumerate().any(|(index, component)| {
                let component_is_directory = index + 1 < path.split('/').count() || is_directory;
                (!pattern.directory_only || component_is_directory)
                    && simple_wildcard_match(&pattern.value, component)
            })
        };
        if matches {
            ignored = !pattern.negated;
        }
    }
    ignored
}

fn simple_wildcard_match(pattern: &str, value: &str) -> bool {
    let pattern = pattern.as_bytes();
    let value = value.as_bytes();
    let mut previous = vec![false; value.len() + 1];
    previous[0] = true;
    for token in pattern {
        let mut current = vec![false; value.len() + 1];
        if *token == b'*' {
            current[0] = previous[0];
            for index in 1..=value.len() {
                current[index] = previous[index] || current[index - 1];
            }
        } else {
            for index in 1..=value.len() {
                current[index] =
                    previous[index - 1] && (*token == b'?' || *token == value[index - 1]);
            }
        }
        previous = current;
    }
    previous[value.len()]
}

fn locate_crux_binary_with_diagnostic() -> Option<PathBuf> {
    locate_crux_binary().or_else(|| {
        if !CRUX_BINARY_MISSING_LOGGED.swap(true, Ordering::Relaxed) {
            eprintln!("Crux built-in is unavailable: the bundled binary is missing.");
        }
        None
    })
}

pub(crate) fn crux_path_environment() -> String {
    let base_path = crate::exec::rtk_path_environment().or_else(|| std::env::var_os("PATH"));
    crux_path_environment_from(base_path.as_deref())
        .to_string_lossy()
        .into_owned()
}

pub(crate) fn crux_path_environment_from(current: Option<&OsStr>) -> OsString {
    let preferred_path = locate_crux_binary()
        .and_then(|binary| {
            crate::exec::path_with_preferred_binary(
                &binary,
                OsStr::new(crux_binary_name()),
                crate::exec::launcher_root().as_deref(),
                current,
            )
        })
        .or_else(|| current.map(OsStr::to_os_string));
    let mut candidates = vec![
        PathBuf::from("/opt/homebrew/bin"),
        PathBuf::from("/usr/local/bin"),
    ];
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        candidates.push(home.join(".cargo/bin"));
        candidates.push(home.join(".local/bin"));
    }
    candidates.extend(crux_login_shell_tool_dirs().iter().cloned());
    augment_crux_path(preferred_path.as_deref(), candidates)
}

fn augment_crux_path(
    current: Option<&OsStr>,
    candidates: impl IntoIterator<Item = PathBuf>,
) -> OsString {
    let mut paths = current
        .into_iter()
        .flat_map(std::env::split_paths)
        .collect::<Vec<_>>();
    for candidate in candidates {
        if candidate.is_dir() && !paths.contains(&candidate) {
            paths.push(candidate);
        }
    }
    std::env::join_paths(paths).unwrap_or_else(|_| current.unwrap_or_default().to_os_string())
}

fn crux_login_shell_tool_dirs() -> &'static [PathBuf] {
    CRUX_LOGIN_SHELL_TOOL_DIRS
        .get_or_init(probe_crux_login_shell_tool_dirs)
        .as_slice()
}

fn probe_crux_login_shell_tool_dirs() -> Vec<PathBuf> {
    if !Path::new("/bin/zsh").is_file() {
        return Vec::new();
    }
    let mut command = StdCommand::new("/bin/zsh");
    command.args(["-lc", "command -v node\ncommand -v npx"]);
    let Ok(output) = exec::run_process_with_limits(
        command,
        CRUX_LOGIN_SHELL_TIMEOUT,
        CRUX_LOGIN_SHELL_OUTPUT_MAX_BYTES,
    ) else {
        return Vec::new();
    };
    let mut directories = Vec::new();
    for line in String::from_utf8_lossy(&output.output).lines() {
        let path = PathBuf::from(line.trim());
        let Some(name) = path.file_name().and_then(OsStr::to_str) else {
            continue;
        };
        if !path.is_absolute() || !path.is_file() || !matches!(name, "node" | "npx") {
            continue;
        }
        let Some(directory) = path.parent().map(Path::to_path_buf) else {
            continue;
        };
        if !directories.contains(&directory) {
            directories.push(directory);
        }
    }
    directories
}

fn locate_crux_binary() -> Option<PathBuf> {
    locate_crux_binary_with_env(
        std::env::var_os("HALV_CRUX_BINARY"),
        std::env::current_exe().ok(),
        staged_crux_binary_path(),
    )
}

fn locate_crux_binary_with_env(
    explicit_override: Option<std::ffi::OsString>,
    current_exe: Option<PathBuf>,
    staged_binary: Option<PathBuf>,
) -> Option<PathBuf> {
    explicit_override
        .map(PathBuf::from)
        .and_then(executable_crux_file)
        .or_else(|| {
            current_exe.and_then(|path| {
                path.parent()
                    .map(|directory| directory.join(crux_binary_name()))
                    .and_then(executable_crux_file)
            })
        })
        .or_else(|| staged_binary.and_then(executable_crux_file))
}

fn staged_crux_binary_path() -> Option<PathBuf> {
    let target = match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => "aarch64-apple-darwin",
        ("macos", "x86_64") => "x86_64-apple-darwin",
        ("linux", "aarch64") => "aarch64-unknown-linux-gnu",
        ("linux", "x86_64") => "x86_64-unknown-linux-gnu",
        ("windows", "x86_64") => "x86_64-pc-windows-msvc",
        _ => return None,
    };
    let filename = if cfg!(windows) {
        format!("crux-{target}.exe")
    } else {
        format!("crux-{target}")
    };
    Some(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("binaries")
            .join(filename),
    )
}

fn crux_binary_name() -> &'static str {
    if cfg!(windows) {
        "crux.exe"
    } else {
        "crux"
    }
}

fn executable_crux_file(path: PathBuf) -> Option<PathBuf> {
    let metadata = fs::metadata(&path).ok()?;
    if !metadata.is_file() {
        return None;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o111 == 0 {
            return None;
        }
    }
    Some(fs::canonicalize(&path).unwrap_or(path))
}

pub fn crux_index_guidance_available(
    tools: &[McpToolDescriptor],
    project_root: Option<&Path>,
) -> bool {
    tools.iter().any(|tool| tool.server_id == BUILTIN_CRUX_ID)
        && project_root.is_some_and(|root| root.join(CRUX_INDEX_RELATIVE_PATH).is_file())
}

fn find_plugin_mcp_server(
    manager: &ExtensionManager,
    server_id: &str,
) -> ExtensionResult<Option<McpServerConfig>> {
    let config = manager.read(Clone::clone);
    for plugin in &config.plugins {
        if let Some(server) = parse_plugin_mcp_servers(
            Path::new(&plugin.root),
            &plugin.id,
            plugin_display_name(plugin),
            true,
        )?
        .into_iter()
        .find(|server| server.id == server_id)
        {
            return Ok(Some(server));
        }
    }
    Ok(None)
}

fn plugin_display_name(plugin: &PluginDescriptor) -> &str {
    if plugin.display_name.trim().is_empty() {
        &plugin.name
    } else {
        &plugin.display_name
    }
}

fn plugin_snapshot_descriptor(plugin: &PluginDescriptor) -> ExtensionResult<PluginDescriptor> {
    let root = Path::new(&plugin.root);
    let validated = validate_plugin(root)?;
    let skill_count = plugin_skill_roots(root, &plugin.id, &validated.display_name)?
        .iter()
        .map(|root| count_skill_directories(&root.root))
        .sum();
    let mcp_server_count =
        parse_plugin_mcp_servers(root, &plugin.id, &validated.display_name, true)?.len();
    let mut descriptor = plugin.clone();
    descriptor.name = validated.name;
    descriptor.version = validated.version;
    descriptor.description = validated.description;
    descriptor.display_name = validated.display_name;
    descriptor.developer_name = validated.developer_name;
    descriptor.capabilities = validated.capabilities;
    descriptor.skill_count = skill_count;
    descriptor.mcp_server_count = mcp_server_count;
    descriptor.contributes_skills = skill_count > 0;
    descriptor.contributes_mcp = mcp_server_count > 0;
    descriptor.unsupported_components = unsupported_components(root);
    Ok(descriptor)
}

async fn connect_and_list_tools(server: &McpServerConfig) -> ExtensionResult<McpConnectionResult> {
    let mut session = McpSession::connect(server, None).await?;
    let result = list_tools_with_session(server, &mut session).await;
    session.shutdown().await;
    result
}

async fn list_tools_with_session(
    server: &McpServerConfig,
    session: &mut McpSession,
) -> ExtensionResult<McpConnectionResult> {
    let initialization = session.initialize(server.startup_timeout_ms).await?;
    let mut cursor = None;
    let mut tools = Vec::new();
    let mut tool_metadata = Vec::new();
    for _ in 0..MAX_MCP_LIST_PAGES {
        let params = cursor
            .as_ref()
            .map(|cursor| json!({ "cursor": cursor }))
            .unwrap_or_else(|| json!({}));
        let result = session
            .request("tools/list", params, server.tool_timeout_ms)
            .await?;
        let page = result
            .get("tools")
            .and_then(Value::as_array)
            .ok_or_else(|| "MCP tools/list response omitted tools".to_string())?;
        for raw in page {
            if tools.len() >= MAX_MCP_TOOLS {
                return Err("MCP server exposed too many tools".to_string());
            }
            if let Some(tool) = parse_mcp_tool(server, raw, initialization.instructions.as_deref())?
            {
                tool_metadata.push(tool.metadata);
                tools.push(tool.descriptor);
            }
        }
        cursor = result
            .get("nextCursor")
            .and_then(Value::as_str)
            .map(ToString::to_string)
            .filter(|value| !value.is_empty());
        if cursor.is_none() {
            return Ok(McpConnectionResult {
                server_id: server.id.clone(),
                server_name: server.name.clone(),
                protocol_version: initialization.protocol_version,
                server_capabilities: initialization.capabilities,
                server_instructions: initialization.instructions,
                server_info: initialization.server_info,
                tools,
                tool_metadata,
            });
        }
    }
    Err("MCP tools/list exceeded the pagination limit".to_string())
}

async fn connect_and_call_tool(
    server: &McpServerConfig,
    name: &str,
    arguments: Value,
    elicitation_sender: Option<McpElicitationSender>,
) -> ExtensionResult<Value> {
    let mut session = McpSession::connect(server, elicitation_sender).await?;
    let result = async {
        session.initialize(server.startup_timeout_ms).await?;
        session
            .request(
                "tools/call",
                json!({ "name": name, "arguments": arguments }),
                server.tool_timeout_ms,
            )
            .await
    }
    .await;
    session.shutdown().await;
    result
}

fn normalize_mcp_tool_result(result: &Value) -> ExtensionResult<McpToolCallResult> {
    let object = result
        .as_object()
        .ok_or_else(|| "MCP tools/call result must be an object".to_string())?;
    let content = object
        .get("content")
        .and_then(Value::as_array)
        .ok_or_else(|| "MCP tools/call result omitted content".to_string())?;
    let mut text_parts = Vec::new();
    let mut images = Vec::new();
    for block in content {
        match block.get("type").and_then(Value::as_str) {
            Some("text") => {
                if let Some(text) = block.get("text").and_then(Value::as_str) {
                    if !text.is_empty() {
                        text_parts.push(text.to_string());
                    }
                }
            }
            Some("image") => {
                if images.len() >= MAX_MCP_RESULT_IMAGES {
                    return Err("MCP tool result included too many images".to_string());
                }
                images.push(normalize_mcp_image(block)?);
            }
            Some("resource") => {
                let Some(resource) = block.get("resource") else {
                    continue;
                };
                if let Some(text) = resource.get("text").and_then(Value::as_str) {
                    if !text.is_empty() {
                        text_parts.push(text.to_string());
                    }
                } else if resource.get("blob").is_some() {
                    text_parts.push(format!(
                        "[Binary MCP resource omitted: {}]",
                        resource
                            .get("uri")
                            .and_then(Value::as_str)
                            .unwrap_or("unnamed resource")
                    ));
                }
            }
            Some("resource_link") => {
                let name = block
                    .get("title")
                    .or_else(|| block.get("name"))
                    .and_then(Value::as_str)
                    .unwrap_or("MCP resource");
                let uri = block.get("uri").and_then(Value::as_str).unwrap_or("");
                text_parts.push(if uri.is_empty() {
                    format!("[Resource: {name}]")
                } else {
                    format!("[Resource: {name} — {uri}]")
                });
            }
            Some("audio") => {
                let media_type = block
                    .get("mimeType")
                    .and_then(Value::as_str)
                    .unwrap_or("audio");
                text_parts.push(format!(
                    "[Audio MCP content omitted from model context: {media_type}]"
                ));
            }
            Some(kind) => text_parts.push(format!("[Unsupported MCP content block: {kind}]")),
            None => text_parts.push("[Malformed MCP content block omitted]".to_string()),
        }
    }
    if let Some(structured) = object.get("structuredContent") {
        let encoded = serde_json::to_string_pretty(structured)
            .map_err(|error| format!("failed to encode MCP structured content: {error}"))?;
        let duplicate = text_parts.iter().any(|part| part.trim() == encoded.trim());
        if !duplicate {
            text_parts.push(format!("Structured result:\n{encoded}"));
        }
    }
    if text_parts.is_empty() && images.is_empty() {
        text_parts.push("MCP tool completed without content.".to_string());
    }
    Ok(McpToolCallResult {
        text: text_parts.join("\n\n"),
        images,
        is_error: object
            .get("isError")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    })
}

fn normalize_mcp_image(block: &Value) -> ExtensionResult<McpToolImage> {
    let media_type = match block
        .get("mimeType")
        .and_then(Value::as_str)
        .map(str::trim)
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("image/jpeg" | "image/jpg") => "image/jpeg",
        Some("image/png") => "image/png",
        Some("image/webp") => "image/webp",
        _ => return Err("MCP tool returned an unsupported image type".to_string()),
    };
    let data = block
        .get("data")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "MCP image content omitted data".to_string())?;
    if data.len() > MAX_MCP_MESSAGE_BYTES {
        return Err("MCP image content exceeded the safety limit".to_string());
    }
    let decoded = STANDARD
        .decode(data)
        .map_err(|_| "MCP image content was not valid base64".to_string())?;
    let detected_type = detect_mcp_image_media_type(&decoded)
        .ok_or_else(|| "MCP image content was not a valid PNG, JPEG, or WebP image".to_string())?;
    if detected_type != media_type {
        return Err("MCP image content did not match its declared type".to_string());
    }
    Ok(McpToolImage {
        media_type: media_type.to_string(),
        data: data.to_string(),
    })
}

fn detect_mcp_image_media_type(data: &[u8]) -> Option<&'static str> {
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

fn parse_mcp_tool(
    server: &McpServerConfig,
    raw: &Value,
    server_instructions: Option<&str>,
) -> ExtensionResult<Option<ParsedMcpTool>> {
    let Some(name) = raw.get("name").and_then(Value::as_str).map(str::trim) else {
        return Ok(None);
    };
    if name.is_empty() || name.len() > 128 {
        return Ok(None);
    }
    if !server.enabled_tools.is_empty() && !server.enabled_tools.iter().any(|item| item == name) {
        return Ok(None);
    }
    if server.disabled_tools.iter().any(|item| item == name) {
        return Ok(None);
    }
    let input_schema = raw
        .get("inputSchema")
        .filter(|schema| schema.is_object())
        .cloned()
        .unwrap_or_else(|| json!({ "type": "object" }));
    let base_description = raw
        .get("description")
        .and_then(Value::as_str)
        .unwrap_or("MCP tool")
        .chars()
        .take(2_000)
        .collect::<String>();
    let guidance = server_instructions
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.chars().take(512).collect::<String>());
    let description = match guidance {
        Some(guidance) => format!(
            "{base_description}\nServer instructions: {guidance}\n(via MCP server {})",
            server.name
        ),
        None => format!("{base_description} (via MCP server {})", server.name),
    };
    let mut annotations = parse_mcp_tool_annotations(raw);
    if server.id == BUILTIN_CRUX_ID && CRUX_TOOLS.contains(&name) {
        annotations.read_only_hint = true;
        annotations.destructive_hint = false;
        annotations.idempotent_hint = true;
        annotations.open_world_hint = false;
    }
    let configured_approval_mode = server
        .tool_approval_modes
        .get(name)
        .cloned()
        .unwrap_or_else(|| server.approval_mode.clone());
    let classified_as_write = !server.trust_tool_annotations || !annotations.read_only_hint;
    let effective_approval_mode = match configured_approval_mode.as_str() {
        "writes" if classified_as_write => "prompt".to_string(),
        "writes" => "approve".to_string(),
        _ => configured_approval_mode.clone(),
    };
    Ok(Some(ParsedMcpTool {
        descriptor: McpToolDescriptor {
            server_id: server.id.clone(),
            server_name: server.name.clone(),
            name: name.to_string(),
            wire_name: mcp_wire_name(&server.id, name),
            description,
            input_schema,
            approval_mode: effective_approval_mode.clone(),
        },
        metadata: McpToolMetadata {
            name: name.to_string(),
            annotations,
            annotations_trusted: server.trust_tool_annotations,
            classified_as_write,
            configured_approval_mode,
            effective_approval_mode,
        },
    }))
}

fn parse_mcp_tool_annotations(raw: &Value) -> McpToolAnnotations {
    let annotations = raw.get("annotations").filter(|value| value.is_object());
    let title = raw
        .get("title")
        .and_then(Value::as_str)
        .or_else(|| annotations?.get("title")?.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.chars().take(200).collect());
    McpToolAnnotations {
        title,
        read_only_hint: annotations
            .and_then(|value| value.get("readOnlyHint"))
            .and_then(Value::as_bool)
            .unwrap_or(false),
        destructive_hint: annotations
            .and_then(|value| value.get("destructiveHint"))
            .and_then(Value::as_bool)
            .unwrap_or(true),
        idempotent_hint: annotations
            .and_then(|value| value.get("idempotentHint"))
            .and_then(Value::as_bool)
            .unwrap_or(false),
        open_world_hint: annotations
            .and_then(|value| value.get("openWorldHint"))
            .and_then(Value::as_bool)
            .unwrap_or(true),
    }
}

fn mcp_wire_name(server_id: &str, tool_name: &str) -> String {
    let server = slug(server_id).chars().take(12).collect::<String>();
    let tool = slug(tool_name).chars().take(24).collect::<String>();
    let digest = short_hash(&format!("{server_id}\0{tool_name}"));
    format!("mcp__{server}__{tool}__{}", &digest[..12])
}

fn parse_mcp_elicitation_request(params: Option<&Value>) -> ExtensionResult<ParsedMcpElicitation> {
    let params = params
        .and_then(Value::as_object)
        .ok_or_else(|| "MCP elicitation request omitted params".to_string())?;
    let mode = params.get("mode").and_then(Value::as_str).unwrap_or("form");
    let message = params
        .get("message")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "MCP elicitation request omitted its message".to_string())?;
    if message.chars().count() > MAX_MCP_ELICITATION_MESSAGE_CHARS {
        return Err("MCP elicitation message exceeded the safety limit".to_string());
    }
    match mode {
        "form" => {
            let schema = params
                .get("requestedSchema")
                .cloned()
                .unwrap_or_else(|| json!({ "type": "object", "properties": {} }));
            validate_mcp_elicitation_schema(&schema)?;
            Ok(ParsedMcpElicitation {
                mode: mode.to_string(),
                message: message.to_string(),
                requested_schema: Some(schema),
                url: None,
            })
        }
        "url" => {
            let raw_url = params
                .get("url")
                .and_then(Value::as_str)
                .ok_or_else(|| "MCP URL elicitation omitted its URL".to_string())?;
            let url = Url::parse(raw_url)
                .map_err(|error| format!("MCP URL elicitation was invalid: {error}"))?;
            let local_http = url.scheme() == "http"
                && matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "::1"));
            if (url.scheme() != "https" && !local_http)
                || url.host_str().is_none()
                || !url.username().is_empty()
                || url.password().is_some()
                || raw_url.len() > 4_096
            {
                return Err("MCP URL elicitation must use a safe absolute HTTPS URL".to_string());
            }
            Ok(ParsedMcpElicitation {
                mode: mode.to_string(),
                message: message.to_string(),
                requested_schema: None,
                url: Some(raw_url.to_string()),
            })
        }
        _ => Err("MCP server requested an unsupported elicitation mode".to_string()),
    }
}

fn validate_mcp_elicitation_schema(schema: &Value) -> ExtensionResult<()> {
    let encoded = serde_json::to_vec(schema)
        .map_err(|error| format!("failed to encode MCP elicitation schema: {error}"))?;
    if encoded.len() > 64 * 1024 {
        return Err("MCP elicitation schema exceeded the safety limit".to_string());
    }
    let schema = schema
        .as_object()
        .ok_or_else(|| "MCP elicitation schema must be an object".to_string())?;
    if schema.get("type").and_then(Value::as_str) != Some("object") {
        return Err("MCP elicitation schema must describe an object".to_string());
    }
    let properties = schema
        .get("properties")
        .and_then(Value::as_object)
        .ok_or_else(|| "MCP elicitation schema omitted properties".to_string())?;
    if properties.len() > 64 {
        return Err("MCP elicitation schema exposed too many fields".to_string());
    }
    for (name, definition) in properties {
        if name.trim().is_empty() || name.len() > 100 {
            return Err("MCP elicitation schema included an invalid field name".to_string());
        }
        let definition = definition
            .as_object()
            .ok_or_else(|| format!("MCP elicitation field `{name}` must be an object"))?;
        let field_type = definition
            .get("type")
            .and_then(Value::as_str)
            .ok_or_else(|| format!("MCP elicitation field `{name}` omitted its type"))?;
        if !matches!(
            field_type,
            "string" | "number" | "integer" | "boolean" | "array"
        ) {
            return Err(format!(
                "MCP elicitation field `{name}` used an unsupported type"
            ));
        }
        if field_type == "array"
            && definition
                .get("items")
                .and_then(Value::as_object)
                .and_then(|items| items.get("type"))
                .and_then(Value::as_str)
                != Some("string")
        {
            return Err(format!(
                "MCP elicitation array field `{name}` must contain strings"
            ));
        }
    }
    if let Some(required) = schema.get("required") {
        let required = required
            .as_array()
            .ok_or_else(|| "MCP elicitation required list must be an array".to_string())?;
        if required.len() > properties.len()
            || required.iter().any(|name| {
                name.as_str()
                    .is_none_or(|name| !properties.contains_key(name))
            })
        {
            return Err("MCP elicitation required list was invalid".to_string());
        }
    }
    Ok(())
}

fn validate_mcp_elicitation_response(
    request: &ParsedMcpElicitation,
    mut response: McpElicitationResponse,
) -> ExtensionResult<McpElicitationResponse> {
    if !matches!(response.action.as_str(), "accept" | "decline" | "cancel") {
        return Err("MCP elicitation response used an invalid action".to_string());
    }
    if response.action != "accept" {
        response.content = None;
        return Ok(response);
    }
    if request.mode == "url" {
        response.content = None;
        return Ok(response);
    }
    let schema = request
        .requested_schema
        .as_ref()
        .and_then(Value::as_object)
        .ok_or_else(|| "MCP form elicitation schema was unavailable".to_string())?;
    let properties = schema
        .get("properties")
        .and_then(Value::as_object)
        .ok_or_else(|| "MCP form elicitation properties were unavailable".to_string())?;
    let empty = json!({});
    let content = response.content.as_ref().unwrap_or(&empty);
    let content = content
        .as_object()
        .ok_or_else(|| "MCP form elicitation response must be an object".to_string())?;
    if content.keys().any(|name| !properties.contains_key(name)) {
        return Err("MCP form elicitation response included an unknown field".to_string());
    }
    if let Some(required) = schema.get("required").and_then(Value::as_array) {
        for name in required.iter().filter_map(Value::as_str) {
            if !content.contains_key(name) {
                return Err(format!("MCP form elicitation field `{name}` is required"));
            }
        }
    }
    for (name, value) in content {
        let definition = properties
            .get(name)
            .and_then(Value::as_object)
            .ok_or_else(|| format!("MCP form elicitation field `{name}` was invalid"))?;
        let valid = match definition.get("type").and_then(Value::as_str) {
            Some("string") => value.is_string(),
            Some("number") => value.is_number(),
            Some("integer") => value.as_i64().is_some() || value.as_u64().is_some(),
            Some("boolean") => value.is_boolean(),
            Some("array") => value
                .as_array()
                .is_some_and(|items| items.iter().all(Value::is_string)),
            _ => false,
        };
        if !valid {
            return Err(format!(
                "MCP form elicitation field `{name}` had the wrong type"
            ));
        }
    }
    response.content = Some(Value::Object(content.clone()));
    Ok(response)
}

#[allow(clippy::large_enum_variant)]
enum McpSession {
    Stdio(StdioMcpSession),
    Http(HttpMcpSession),
}

impl McpSession {
    async fn connect(
        server: &McpServerConfig,
        elicitation_sender: Option<McpElicitationSender>,
    ) -> ExtensionResult<Self> {
        match server.transport {
            McpTransport::Stdio => {
                StdioMcpSession::spawn(server, elicitation_sender).map(Self::Stdio)
            }
            McpTransport::StreamableHttp => Ok(Self::Http(HttpMcpSession::new(server).await?)),
        }
    }

    async fn initialize(&mut self, timeout_ms: u64) -> ExtensionResult<McpInitializeResult> {
        let capabilities = if matches!(self, Self::Stdio(session) if session.elicitation_sender.is_some())
        {
            json!({
                "elicitation": {
                    "form": {},
                    "url": {}
                }
            })
        } else {
            json!({})
        };
        let result = self
            .request(
                "initialize",
                json!({
                    "protocolVersion": MCP_PROTOCOL_VERSION,
                    "capabilities": capabilities,
                    "clientInfo": {
                        "name": "halv",
                        "title": "Halv",
                        "version": env!("CARGO_PKG_VERSION")
                    }
                }),
                timeout_ms,
            )
            .await?;
        let protocol_version = result
            .get("protocolVersion")
            .and_then(Value::as_str)
            .ok_or_else(|| "MCP initialize response omitted protocolVersion".to_string())?
            .to_string();
        let capabilities = result
            .get("capabilities")
            .filter(|value| value.is_object())
            .cloned()
            .unwrap_or_else(|| json!({}));
        let instructions = result
            .get("instructions")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| value.chars().take(16_000).collect::<String>());
        let server_info = result
            .get("serverInfo")
            .filter(|value| value.is_object())
            .cloned();
        match self {
            Self::Stdio(session) => session.notify_initialized().await?,
            Self::Http(session) => {
                session.protocol_version = Some(protocol_version.clone());
                session.notify_initialized().await?;
            }
        }
        Ok(McpInitializeResult {
            protocol_version,
            capabilities,
            instructions,
            server_info,
        })
    }

    async fn request(
        &mut self,
        method: &str,
        params: Value,
        timeout_ms: u64,
    ) -> ExtensionResult<Value> {
        match self {
            Self::Stdio(session) => session.request(method, params, timeout_ms).await,
            Self::Http(session) => session.request(method, params, timeout_ms).await,
        }
    }

    async fn shutdown(self) {
        if let Self::Stdio(session) = self {
            session.shutdown().await;
        }
    }
}

struct StdioMcpSession {
    child: Child,
    stdin: ChildStdin,
    lines: Lines<BufReader<ChildStdout>>,
    next_id: u64,
    elicitation_sender: Option<McpElicitationSender>,
    elicitation_count: usize,
}

impl StdioMcpSession {
    fn spawn(
        server: &McpServerConfig,
        elicitation_sender: Option<McpElicitationSender>,
    ) -> ExtensionResult<Self> {
        let command = server
            .command
            .as_deref()
            .ok_or_else(|| "STDIO MCP command is missing".to_string())?;
        let mut process = Command::new(command);
        process
            .args(&server.args)
            .env_clear()
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true);
        crate::process::hide_console_tokio(&mut process);
        for name in base_environment_names() {
            if let Some(value) = std::env::var_os(name) {
                process.env(name, value);
            }
        }
        for (name, value) in &server.env {
            process.env(name, value);
        }
        for name in &server.env_vars {
            if let Some(value) = std::env::var_os(name) {
                process.env(name, value);
            }
        }
        if let Some(cwd) = server.cwd.as_deref() {
            process.current_dir(canonical_directory(Path::new(cwd))?);
        }
        let mut child = process
            .spawn()
            .map_err(|error| format!("failed to start MCP server: {error}"))?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| "failed to open MCP server stdin".to_string())?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "failed to open MCP server stdout".to_string())?;
        Ok(Self {
            child,
            stdin,
            lines: BufReader::new(stdout).lines(),
            next_id: 1,
            elicitation_sender,
            elicitation_count: 0,
        })
    }

    async fn shutdown(mut self) {
        // Wait for the exit so a fast disable and re-enable cannot race
        // two server instances. On timeout, kill_on_drop reaps the child.
        let _ = self.child.start_kill();
        let _ = tokio::time::timeout(MCP_STDIO_SHUTDOWN_TIMEOUT, self.child.wait()).await;
    }

    async fn request(
        &mut self,
        method: &str,
        params: Value,
        timeout_ms: u64,
    ) -> ExtensionResult<Value> {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        self.write(&json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params
        }))
        .await?;
        let response = tokio::time::timeout(
            std::time::Duration::from_millis(timeout_ms),
            self.read_response(id),
        )
        .await
        .map_err(|_| format!("MCP request `{method}` timed out"))??;
        rpc_result(response)
    }

    async fn notify_initialized(&mut self) -> ExtensionResult<()> {
        self.write(&json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized"
        }))
        .await
    }

    async fn write(&mut self, value: &Value) -> ExtensionResult<()> {
        let mut encoded = serde_json::to_vec(value)
            .map_err(|error| format!("failed to encode MCP message: {error}"))?;
        if encoded.len() > MAX_MCP_MESSAGE_BYTES {
            return Err("MCP message exceeded the safety limit".to_string());
        }
        encoded.push(b'\n');
        self.stdin
            .write_all(&encoded)
            .await
            .map_err(|error| format!("failed to write MCP message: {error}"))?;
        self.stdin
            .flush()
            .await
            .map_err(|error| format!("failed to flush MCP message: {error}"))
    }

    async fn read_response(&mut self, id: u64) -> ExtensionResult<Value> {
        for _ in 0..1_000 {
            let line = self
                .lines
                .next_line()
                .await
                .map_err(|error| format!("failed to read MCP message: {error}"))?
                .ok_or_else(|| "MCP server closed stdout".to_string())?;
            if line.len() > MAX_MCP_MESSAGE_BYTES {
                return Err("MCP response exceeded the safety limit".to_string());
            }
            let value: Value = serde_json::from_str(&line)
                .map_err(|error| format!("MCP server emitted invalid JSON: {error}"))?;
            if value.get("id").and_then(Value::as_u64) == Some(id) {
                return Ok(value);
            }
            if value.get("method").and_then(Value::as_str) == Some("elicitation/create")
                && value.get("id").is_some()
            {
                self.handle_elicitation_request(&value).await?;
            }
        }
        Err("MCP server emitted too many unrelated messages".to_string())
    }

    async fn handle_elicitation_request(&mut self, request: &Value) -> ExtensionResult<()> {
        self.elicitation_count = self.elicitation_count.saturating_add(1);
        if self.elicitation_count > MAX_MCP_ELICITATIONS_PER_REQUEST {
            return Err("MCP server requested too many user interactions".to_string());
        }
        let request_id = request
            .get("id")
            .cloned()
            .ok_or_else(|| "MCP elicitation request omitted id".to_string())?;
        let elicitation = parse_mcp_elicitation_request(request.get("params"))?;
        let Some(sender) = self.elicitation_sender.clone() else {
            self.write(&json!({
                "jsonrpc": "2.0",
                "id": request_id,
                "result": { "action": "cancel" }
            }))
            .await?;
            return Ok(());
        };
        let (response_sender, response_receiver) = oneshot::channel();
        sender
            .send(McpElicitationRequest {
                mode: elicitation.mode.clone(),
                message: elicitation.message.clone(),
                requested_schema: elicitation.requested_schema.clone(),
                url: elicitation.url.clone(),
                responder: response_sender,
            })
            .await
            .map_err(|_| "MCP elicitation UI was unavailable".to_string())?;
        let response = response_receiver
            .await
            .unwrap_or_else(|_| McpElicitationResponse {
                action: "cancel".to_string(),
                content: None,
            });
        let response = validate_mcp_elicitation_response(&elicitation, response)?;
        let mut result = serde_json::Map::new();
        result.insert("action".to_string(), Value::String(response.action));
        if let Some(content) = response.content {
            result.insert("content".to_string(), content);
        }
        self.write(&json!({
            "jsonrpc": "2.0",
            "id": request_id,
            "result": Value::Object(result)
        }))
        .await
    }
}

impl Drop for StdioMcpSession {
    fn drop(&mut self) {
        let _ = self.child.start_kill();
    }
}

fn base_environment_names() -> impl Iterator<Item = &'static str> {
    [
        "HOME",
        "PATH",
        "SHELL",
        "TMPDIR",
        "TEMP",
        "TMP",
        "SYSTEMROOT",
        "PATHEXT",
        "USERPROFILE",
    ]
    .into_iter()
}

struct HttpMcpSession {
    client: reqwest::Client,
    url: String,
    headers: BTreeMap<String, String>,
    session_id: Option<String>,
    protocol_version: Option<String>,
    bearer_token: Option<String>,
    oauth_server: Option<McpServerConfig>,
    next_id: u64,
}

impl HttpMcpSession {
    async fn new(server: &McpServerConfig) -> ExtensionResult<Self> {
        let mut headers = server.headers.clone();
        for (header, variable) in &server.env_http_headers {
            if let Ok(value) = std::env::var(variable) {
                headers.insert(header.clone(), value);
            }
        }
        Ok(Self {
            client: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .map_err(|error| format!("failed to create MCP HTTP client: {error}"))?,
            url: server
                .url
                .clone()
                .ok_or_else(|| "HTTP MCP URL is missing".to_string())?,
            headers,
            session_id: None,
            protocol_version: None,
            bearer_token: resolve_mcp_http_token(server).await?,
            oauth_server: matches!(server.auth_mode, McpAuthMode::Oauth).then(|| server.clone()),
            next_id: 1,
        })
    }

    async fn request(
        &mut self,
        method: &str,
        params: Value,
        timeout_ms: u64,
    ) -> ExtensionResult<Value> {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        let response = self
            .post(
                json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "method": method,
                    "params": params
                }),
                timeout_ms,
            )
            .await?;
        let value = parse_http_rpc_response(&response, id)?;
        rpc_result(value)
    }

    async fn notify_initialized(&mut self) -> ExtensionResult<()> {
        self.post(
            json!({
                "jsonrpc": "2.0",
                "method": "notifications/initialized"
            }),
            10_000,
        )
        .await
        .map(|_| ())
    }

    async fn post(&mut self, body: Value, timeout_ms: u64) -> ExtensionResult<String> {
        let mut response = self.send_post(&body, timeout_ms).await?;
        if response.status() == reqwest::StatusCode::UNAUTHORIZED {
            if let Some(server) = self.oauth_server.clone() {
                if let Some(credentials) = read_mcp_oauth_credentials(&server.id)? {
                    if credentials.refresh_token.is_some() {
                        let refreshed = refresh_mcp_oauth_credentials(&server, credentials).await?;
                        self.bearer_token = Some(refreshed.access_token.clone());
                        store_mcp_oauth_credentials(&server.id, &refreshed)?;
                        response = self.send_post(&body, timeout_ms).await?;
                    }
                }
            }
        }
        if let Some(value) = response.headers().get("MCP-Session-Id") {
            self.session_id = Some(
                value
                    .to_str()
                    .map_err(|_| "MCP session id was not valid UTF-8".to_string())?
                    .to_string(),
            );
        }
        let status = response.status();
        let bytes = response
            .bytes()
            .await
            .map_err(|error| format!("failed to read MCP HTTP response: {error}"))?;
        if bytes.len() > MAX_MCP_MESSAGE_BYTES {
            return Err("MCP HTTP response exceeded the safety limit".to_string());
        }
        let text = String::from_utf8(bytes.to_vec())
            .map_err(|_| "MCP HTTP response was not UTF-8".to_string())?;
        if !status.is_success() {
            return Err(format!(
                "MCP HTTP server returned {status}: {}",
                text.chars().take(300).collect::<String>()
            ));
        }
        Ok(text)
    }

    async fn send_post(&self, body: &Value, timeout_ms: u64) -> ExtensionResult<reqwest::Response> {
        let mut request = self
            .client
            .post(&self.url)
            .header(
                reqwest::header::ACCEPT,
                "application/json, text/event-stream",
            )
            .header(reqwest::header::CONTENT_TYPE, "application/json");
        for (name, value) in &self.headers {
            request = request.header(name, value);
        }
        if let Some(session_id) = &self.session_id {
            request = request.header("MCP-Session-Id", session_id);
        }
        if let Some(protocol) = &self.protocol_version {
            request = request.header("MCP-Protocol-Version", protocol);
        }
        if let Some(token) = &self.bearer_token {
            request = request.bearer_auth(token);
        }
        let response = tokio::time::timeout(
            std::time::Duration::from_millis(timeout_ms),
            request.json(&body).send(),
        )
        .await
        .map_err(|_| "MCP HTTP request timed out".to_string())?
        .map_err(|error| format!("MCP HTTP request failed: {error}"))?;
        Ok(response)
    }
}

fn parse_http_rpc_response(body: &str, id: u64) -> ExtensionResult<Value> {
    let body = body.trim();
    if body.is_empty() {
        return Err("MCP HTTP response was empty".to_string());
    }
    if let Ok(value) = serde_json::from_str::<Value>(body) {
        if value.get("id").and_then(Value::as_u64) == Some(id) {
            return Ok(value);
        }
    }
    for line in body.lines() {
        let Some(data) = line.trim().strip_prefix("data:") else {
            continue;
        };
        let value: Value = serde_json::from_str(data.trim())
            .map_err(|error| format!("invalid MCP SSE data: {error}"))?;
        if value.get("id").and_then(Value::as_u64) == Some(id) {
            return Ok(value);
        }
    }
    Err("MCP HTTP response omitted the matching JSON-RPC result".to_string())
}

fn rpc_result(mut response: Value) -> ExtensionResult<Value> {
    if let Some(error) = response.get("error") {
        let message = error
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("unknown JSON-RPC error");
        return Err(format!("MCP server error: {message}"));
    }
    response
        .get_mut("result")
        .map(Value::take)
        .ok_or_else(|| "MCP response omitted result".to_string())
}

// The MCP secrets go through `secret_store`. A bearer token can hold 16 KB
// and an OAuth token set can exceed the Windows credential blob limit.
fn mcp_bearer_account(server_id: &str) -> String {
    format!("mcp-bearer-{server_id}")
}

fn read_mcp_bearer_token(server_id: &str) -> ExtensionResult<Option<String>> {
    secret_store::read_secret(KEYRING_SERVICE, &mcp_bearer_account(server_id))
        .map_err(|error| format!("failed to read MCP token from Keychain: {error}"))
}

fn write_mcp_bearer_token(server_id: &str, token: &str) -> ExtensionResult<()> {
    secret_store::write_secret(KEYRING_SERVICE, &mcp_bearer_account(server_id), token)
        .map_err(|error| format!("failed to store MCP token in Keychain: {error}"))
}

fn clear_mcp_bearer_token(server_id: &str) -> ExtensionResult<()> {
    secret_store::delete_secret(KEYRING_SERVICE, &mcp_bearer_account(server_id))
        .map_err(|error| format!("failed to clear MCP token from Keychain: {error}"))
}

fn mcp_oauth_account(server_id: &str) -> String {
    format!("mcp-oauth-{server_id}")
}

fn read_mcp_oauth_credentials(server_id: &str) -> ExtensionResult<Option<McpOAuthCredentials>> {
    match secret_store::read_secret(KEYRING_SERVICE, &mcp_oauth_account(server_id)) {
        Ok(Some(encoded)) => decode_mcp_oauth_credentials(&encoded).map(Some),
        Ok(None) => Ok(None),
        Err(error) => Err(format!(
            "failed to read MCP OAuth token from Keychain: {error}"
        )),
    }
}

fn clear_mcp_oauth_token(server_id: &str) -> ExtensionResult<()> {
    secret_store::delete_secret(KEYRING_SERVICE, &mcp_oauth_account(server_id))
        .map_err(|error| format!("failed to clear MCP OAuth token from Keychain: {error}"))
}

fn decode_mcp_oauth_credentials(encoded: &str) -> ExtensionResult<McpOAuthCredentials> {
    let encoded = encoded.trim();
    if encoded.is_empty() || encoded.len() > 64 * 1024 {
        return Err("stored MCP OAuth credentials are empty or too large".to_string());
    }
    let credentials = if encoded.starts_with('{') {
        serde_json::from_str(encoded)
            .map_err(|error| format!("stored MCP OAuth credentials are invalid: {error}"))?
    } else {
        // Preserve compatibility with the original access-token-only Keychain seam.
        McpOAuthCredentials {
            access_token: encoded.to_string(),
            refresh_token: None,
            token_type: Some("Bearer".to_string()),
            scope: None,
            expires_at_unix: None,
            client_id: String::new(),
            token_endpoint: String::new(),
            authorization_server: String::new(),
            resource: String::new(),
        }
    };
    validate_mcp_oauth_credentials(credentials)
}

fn validate_mcp_oauth_credentials(
    credentials: McpOAuthCredentials,
) -> ExtensionResult<McpOAuthCredentials> {
    if credentials.access_token.trim().is_empty()
        || credentials.access_token.len() > 32 * 1024
        || credentials
            .refresh_token
            .as_ref()
            .is_some_and(|token| token.trim().is_empty() || token.len() > 32 * 1024)
        || credentials.client_id.len() > 2_048
        || credentials.token_endpoint.len() > 4_096
        || credentials.authorization_server.len() > 4_096
        || credentials.resource.len() > 4_096
    {
        return Err("stored MCP OAuth credentials failed validation".to_string());
    }
    if credentials
        .token_type
        .as_deref()
        .is_some_and(|token_type| !token_type.eq_ignore_ascii_case("bearer"))
    {
        return Err("MCP OAuth server returned an unsupported token type".to_string());
    }
    Ok(credentials)
}

fn store_mcp_oauth_credentials(
    server_id: &str,
    credentials: &McpOAuthCredentials,
) -> ExtensionResult<()> {
    let credentials = validate_mcp_oauth_credentials(credentials.clone())?;
    let encoded = serde_json::to_string(&credentials)
        .map_err(|error| format!("failed to encode MCP OAuth credentials: {error}"))?;
    secret_store::write_secret(KEYRING_SERVICE, &mcp_oauth_account(server_id), &encoded)
        .map_err(|error| format!("failed to store MCP OAuth credentials in Keychain: {error}"))
}

fn resolve_mcp_bearer_token(server: &McpServerConfig) -> ExtensionResult<Option<String>> {
    if let Some(token) = read_mcp_bearer_token(&server.id)? {
        return Ok(Some(token));
    }
    if let Some(variable) = server.bearer_token_env_var.as_deref() {
        if let Ok(token) = std::env::var(variable) {
            let token = token.trim();
            if !token.is_empty() {
                return Ok(Some(token.to_string()));
            }
        }
    }
    Ok(None)
}

async fn resolve_mcp_http_token(server: &McpServerConfig) -> ExtensionResult<Option<String>> {
    if !matches!(server.auth_mode, McpAuthMode::Oauth) {
        return resolve_mcp_bearer_token(server);
    }
    let Some(mut credentials) = read_mcp_oauth_credentials(&server.id)? else {
        return Ok(None);
    };
    if mcp_oauth_credentials_need_refresh(&credentials) {
        credentials = refresh_mcp_oauth_credentials(server, credentials).await?;
        store_mcp_oauth_credentials(&server.id, &credentials)?;
    }
    Ok(Some(credentials.access_token))
}

fn mcp_oauth_credentials_need_refresh(credentials: &McpOAuthCredentials) -> bool {
    credentials.expires_at_unix.is_some_and(|expires_at| {
        expires_at <= Utc::now().timestamp() + MCP_OAUTH_REFRESH_WINDOW_SECS
    })
}

fn mcp_auth_status(server: &McpServerConfig) -> McpAuthStatus {
    if matches!(server.transport, McpTransport::Stdio) {
        return McpAuthStatus {
            server_id: server.id.clone(),
            mode: McpAuthMode::None,
            state: "environment".to_string(),
            login_supported: false,
            message: "STDIO authentication uses the configured environment variables.".to_string(),
        };
    }
    if matches!(server.auth_mode, McpAuthMode::Oauth) {
        return match read_mcp_oauth_credentials(&server.id) {
            Ok(Some(credentials)) if mcp_oauth_credentials_need_refresh(&credentials) => {
                let refreshable = credentials.refresh_token.is_some()
                    && !credentials.client_id.is_empty()
                    && !credentials.token_endpoint.is_empty();
                McpAuthStatus {
                    server_id: server.id.clone(),
                    mode: McpAuthMode::Oauth,
                    state: if refreshable {
                        "refresh_required".to_string()
                    } else {
                        "expired".to_string()
                    },
                    login_supported: true,
                    message: if refreshable {
                        "The OAuth token will refresh on the next connection.".to_string()
                    } else {
                        "The OAuth token expired; sign in again.".to_string()
                    },
                }
            }
            Ok(Some(_)) => McpAuthStatus {
                server_id: server.id.clone(),
                mode: McpAuthMode::Oauth,
                state: "configured".to_string(),
                login_supported: true,
                message: "OAuth is connected securely through the system Keychain.".to_string(),
            },
            Ok(None) => McpAuthStatus {
                server_id: server.id.clone(),
                mode: McpAuthMode::Oauth,
                state: "login_required".to_string(),
                login_supported: true,
                message: "Sign in to authorize this MCP server.".to_string(),
            },
            Err(error) => McpAuthStatus {
                server_id: server.id.clone(),
                mode: McpAuthMode::Oauth,
                state: "error".to_string(),
                login_supported: true,
                message: error,
            },
        };
    }
    let resolved = resolve_mcp_bearer_token(server);
    match (&server.auth_mode, resolved) {
        (McpAuthMode::None, Ok(Some(_))) | (McpAuthMode::Bearer, Ok(Some(_))) => McpAuthStatus {
            server_id: server.id.clone(),
            mode: McpAuthMode::Bearer,
            state: "configured".to_string(),
            login_supported: false,
            message: "Bearer authentication is configured.".to_string(),
        },
        (McpAuthMode::None, Ok(None)) => McpAuthStatus {
            server_id: server.id.clone(),
            mode: McpAuthMode::None,
            state: "not_required".to_string(),
            login_supported: false,
            message: "No authentication is configured.".to_string(),
        },
        (McpAuthMode::Bearer, Ok(None)) => McpAuthStatus {
            server_id: server.id.clone(),
            mode: McpAuthMode::Bearer,
            state: "missing".to_string(),
            login_supported: false,
            message: "Set a bearer token or provide the configured environment variable."
                .to_string(),
        },
        (mode, Err(error)) => McpAuthStatus {
            server_id: server.id.clone(),
            mode: mode.clone(),
            state: "error".to_string(),
            login_supported: false,
            message: error,
        },
        (McpAuthMode::Oauth, _) => unreachable!("OAuth status returned above"),
    }
}

async fn run_mcp_oauth_login(server: &McpServerConfig) -> ExtensionResult<McpOAuthCredentials> {
    let client = mcp_oauth_http_client()?;
    let discovery = discover_mcp_oauth(&client, server).await?;
    if !discovery
        .metadata
        .code_challenge_methods_supported
        .iter()
        .any(|method| method == "S256")
    {
        return Err(
            "MCP authorization server does not advertise the required PKCE S256 method".to_string(),
        );
    }

    let listener = TcpListener::bind(("127.0.0.1", 0))
        .await
        .map_err(|error| format!("failed to bind MCP OAuth callback: {error}"))?;
    let callback_port = listener
        .local_addr()
        .map_err(|error| format!("failed to inspect MCP OAuth callback: {error}"))?
        .port();
    let redirect_uri = format!("http://127.0.0.1:{callback_port}{MCP_OAUTH_CALLBACK_PATH}");
    let client_id = if let Some(client_id) = server.oauth_client_id.as_deref() {
        client_id.to_string()
    } else if let Some(registration_endpoint) = discovery.metadata.registration_endpoint.as_deref()
    {
        dynamically_register_mcp_client(&client, registration_endpoint, &redirect_uri).await?
    } else {
        let supported_hint = if discovery.metadata.client_id_metadata_document_supported {
            " This server supports hosted Client ID Metadata Documents, but Halv needs a pre-registered client ID because a desktop app cannot securely host one on localhost."
        } else {
            ""
        };
        return Err(format!(
            "MCP authorization server does not offer dynamic client registration. Enter its pre-registered OAuth client ID in the server settings.{supported_hint}"
        ));
    };

    let flow = McpOAuthFlow::generate();
    let scopes = select_mcp_oauth_scopes(server, &discovery);
    let authorize_url = build_mcp_oauth_authorize_url(
        &discovery.metadata.authorization_endpoint,
        &client_id,
        &redirect_uri,
        &flow,
        &discovery.resource,
        &scopes,
    )?;
    tauri_plugin_opener::open_url(&authorize_url, None::<&str>)
        .map_err(|error| format!("failed to open MCP OAuth login in the browser: {error}"))?;
    let code = wait_for_mcp_oauth_callback(listener, &flow.state).await?;
    let response = exchange_mcp_oauth_code(
        &client,
        &discovery.metadata.token_endpoint,
        &client_id,
        &redirect_uri,
        &flow.verifier,
        &code,
        &discovery.resource,
    )
    .await?;
    mcp_oauth_credentials_from_response(
        response,
        None,
        client_id,
        discovery.metadata.token_endpoint,
        discovery.authorization_server,
        discovery.resource,
        scopes,
    )
}

impl McpOAuthFlow {
    fn generate() -> Self {
        let verifier = URL_SAFE_NO_PAD.encode(rand::random::<[u8; 64]>());
        let challenge = mcp_pkce_challenge(&verifier);
        let state = URL_SAFE_NO_PAD.encode(rand::random::<[u8; 32]>());
        Self {
            verifier,
            challenge,
            state,
        }
    }
}

fn mcp_pkce_challenge(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

fn mcp_oauth_http_client() -> ExtensionResult<reqwest::Client> {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(std::time::Duration::from_secs(20))
        .user_agent(concat!("Halv/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|error| format!("failed to create MCP OAuth client: {error}"))
}

async fn discover_mcp_oauth(
    client: &reqwest::Client,
    server: &McpServerConfig,
) -> ExtensionResult<McpOAuthDiscovery> {
    let server_url = server
        .url
        .as_deref()
        .ok_or_else(|| "HTTP MCP URL is missing".to_string())?;
    let resource = canonical_mcp_resource(server_url)?;
    let (challenged_metadata_url, challenged_scopes) =
        probe_mcp_oauth_challenge(client, server_url).await?;

    let protected = if let Some(metadata_url) = challenged_metadata_url {
        let metadata_url =
            validate_mcp_oauth_url(&metadata_url, "MCP protected resource metadata URL", true)?;
        fetch_mcp_oauth_json_required(client, metadata_url.as_str(), "protected resource metadata")
            .await?
    } else {
        let mut found = None;
        for candidate in protected_resource_metadata_urls(server_url)? {
            if let Some(metadata) = fetch_mcp_oauth_json_optional::<McpProtectedResourceMetadata>(
                client,
                candidate.as_str(),
            )
            .await?
            {
                found = Some(metadata);
                break;
            }
        }
        found.ok_or_else(|| {
            "MCP server did not expose OAuth protected resource metadata".to_string()
        })?
    };

    if protected.authorization_servers.is_empty() {
        return Err("MCP protected resource metadata omitted authorization_servers".to_string());
    }
    if let Some(declared_resource) = protected.resource.as_deref() {
        let declared = validate_mcp_oauth_url(declared_resource, "MCP resource identifier", true)?;
        let target = Url::parse(&resource)
            .map_err(|error| format!("invalid canonical MCP resource: {error}"))?;
        if !same_url_origin(&declared, &target) {
            return Err(
                "MCP protected resource metadata declared a different resource origin".to_string(),
            );
        }
    }

    let authorization_server = protected.authorization_servers[0].trim().to_string();
    let issuer = validate_mcp_oauth_url(
        &authorization_server,
        "MCP authorization server issuer",
        false,
    )?;
    let metadata = discover_mcp_authorization_server_metadata(client, &issuer).await?;
    validate_mcp_oauth_url(
        &metadata.authorization_endpoint,
        "MCP OAuth authorization endpoint",
        false,
    )?;
    validate_mcp_oauth_url(&metadata.token_endpoint, "MCP OAuth token endpoint", false)?;
    if let Some(endpoint) = metadata.registration_endpoint.as_deref() {
        validate_mcp_oauth_url(endpoint, "MCP OAuth registration endpoint", false)?;
    }

    Ok(McpOAuthDiscovery {
        resource,
        challenged_scopes,
        protected,
        authorization_server,
        metadata,
    })
}

async fn probe_mcp_oauth_challenge(
    client: &reqwest::Client,
    server_url: &str,
) -> ExtensionResult<(Option<String>, Vec<String>)> {
    let response = client
        .post(server_url)
        .header(
            reqwest::header::ACCEPT,
            "application/json, text/event-stream",
        )
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .header("MCP-Protocol-Version", MCP_PROTOCOL_VERSION)
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": MCP_PROTOCOL_VERSION,
                "capabilities": {},
                "clientInfo": { "name": "halv", "version": env!("CARGO_PKG_VERSION") }
            }
        }))
        .send()
        .await
        .map_err(|error| format!("failed to contact MCP server for OAuth discovery: {error}"))?;
    let mut metadata_url = None;
    let mut scopes = Vec::new();
    for header in response
        .headers()
        .get_all(reqwest::header::WWW_AUTHENTICATE)
    {
        let Ok(header) = header.to_str() else {
            continue;
        };
        if metadata_url.is_none() {
            metadata_url = mcp_bearer_challenge_parameter(header, "resource_metadata");
        }
        if scopes.is_empty() {
            scopes = mcp_bearer_challenge_parameter(header, "scope")
                .map(|value| split_mcp_oauth_scopes(&value))
                .unwrap_or_default();
        }
    }
    Ok((metadata_url, scopes))
}

fn mcp_bearer_challenge_parameter(header: &str, requested: &str) -> Option<String> {
    let lowercase = header.to_ascii_lowercase();
    let bearer = lowercase.find("bearer")?;
    let rest = header.get(bearer + "bearer".len()..)?;
    for part in split_quoted_header_values(rest) {
        let part = part.trim();
        let (key, value) = part.split_once('=')?;
        if !key.trim().eq_ignore_ascii_case(requested) {
            continue;
        }
        let value = value.trim();
        if let Some(quoted) = value
            .strip_prefix('"')
            .and_then(|value| value.strip_suffix('"'))
        {
            let mut decoded = String::with_capacity(quoted.len());
            let mut escaped = false;
            for character in quoted.chars() {
                if escaped {
                    decoded.push(character);
                    escaped = false;
                } else if character == '\\' {
                    escaped = true;
                } else {
                    decoded.push(character);
                }
            }
            return Some(decoded);
        }
        return Some(value.to_string());
    }
    None
}

fn split_quoted_header_values(value: &str) -> Vec<&str> {
    let mut values = Vec::new();
    let mut start = 0;
    let mut quoted = false;
    let mut escaped = false;
    for (index, character) in value.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match character {
            '\\' if quoted => escaped = true,
            '"' => quoted = !quoted,
            ',' if !quoted => {
                values.push(&value[start..index]);
                start = index + character.len_utf8();
            }
            _ => {}
        }
    }
    values.push(&value[start..]);
    values
}

fn protected_resource_metadata_urls(server_url: &str) -> ExtensionResult<Vec<Url>> {
    let parsed = validate_mcp_oauth_url(server_url, "MCP server URL", true)?;
    let endpoint_path = parsed.path().trim_matches('/');
    let mut urls = Vec::new();
    if !endpoint_path.is_empty() {
        let mut path_specific = parsed.clone();
        path_specific.set_query(None);
        path_specific.set_fragment(None);
        path_specific.set_path(&format!(
            "/.well-known/oauth-protected-resource/{endpoint_path}"
        ));
        urls.push(path_specific);
    }
    let mut root = parsed;
    root.set_query(None);
    root.set_fragment(None);
    root.set_path("/.well-known/oauth-protected-resource");
    if !urls.iter().any(|candidate| candidate == &root) {
        urls.push(root);
    }
    Ok(urls)
}

async fn discover_mcp_authorization_server_metadata(
    client: &reqwest::Client,
    issuer: &Url,
) -> ExtensionResult<McpAuthorizationServerMetadata> {
    let mut last_error = None;
    for candidate in authorization_server_metadata_urls(issuer) {
        match fetch_mcp_oauth_json_optional::<McpAuthorizationServerMetadata>(
            client,
            candidate.as_str(),
        )
        .await
        {
            Ok(Some(metadata)) => {
                if normalize_issuer(&metadata.issuer) != normalize_issuer(issuer.as_str()) {
                    return Err(
                        "OAuth authorization server metadata issuer did not match discovery"
                            .to_string(),
                    );
                }
                return Ok(metadata);
            }
            Ok(None) => {}
            Err(error) => last_error = Some(error),
        }
    }
    Err(last_error
        .unwrap_or_else(|| "OAuth authorization server metadata was not available".to_string()))
}

fn authorization_server_metadata_urls(issuer: &Url) -> Vec<Url> {
    let path = issuer.path().trim_matches('/');
    let mut urls = Vec::new();
    let mut oauth = issuer.clone();
    oauth.set_query(None);
    oauth.set_fragment(None);
    let oauth_path = if path.is_empty() {
        "/.well-known/oauth-authorization-server".to_string()
    } else {
        // RFC 8414 path insertion.
        format!("/.well-known/oauth-authorization-server/{path}")
    };
    oauth.set_path(&oauth_path);
    urls.push(oauth);

    let mut oidc_inserted = issuer.clone();
    oidc_inserted.set_query(None);
    oidc_inserted.set_fragment(None);
    let oidc_inserted_path = if path.is_empty() {
        "/.well-known/openid-configuration".to_string()
    } else {
        format!("/.well-known/openid-configuration/{path}")
    };
    oidc_inserted.set_path(&oidc_inserted_path);
    urls.push(oidc_inserted);
    if !path.is_empty() {
        let mut oidc_appended = issuer.clone();
        oidc_appended.set_query(None);
        oidc_appended.set_fragment(None);
        oidc_appended.set_path(&format!("/{path}/.well-known/openid-configuration"));
        urls.push(oidc_appended);
    }
    urls
}

async fn fetch_mcp_oauth_json_optional<T: DeserializeOwned>(
    client: &reqwest::Client,
    url: &str,
) -> ExtensionResult<Option<T>> {
    let response = client
        .get(url)
        .header("MCP-Protocol-Version", MCP_PROTOCOL_VERSION)
        .send()
        .await
        .map_err(|error| format!("failed to fetch MCP OAuth metadata: {error}"))?;
    if !response.status().is_success() {
        return Ok(None);
    }
    parse_mcp_oauth_json_response(response, "MCP OAuth metadata")
        .await
        .map(Some)
}

async fn fetch_mcp_oauth_json_required<T: DeserializeOwned>(
    client: &reqwest::Client,
    url: &str,
    label: &str,
) -> ExtensionResult<T> {
    let response = client
        .get(url)
        .header("MCP-Protocol-Version", MCP_PROTOCOL_VERSION)
        .send()
        .await
        .map_err(|error| format!("failed to fetch {label}: {error}"))?;
    let status = response.status();
    if !status.is_success() {
        return Err(format!("{label} returned HTTP {status}"));
    }
    parse_mcp_oauth_json_response(response, label).await
}

async fn parse_mcp_oauth_json_response<T: DeserializeOwned>(
    response: reqwest::Response,
    label: &str,
) -> ExtensionResult<T> {
    if response
        .content_length()
        .is_some_and(|length| length > MAX_MCP_OAUTH_METADATA_BYTES as u64)
    {
        return Err(format!("{label} exceeded the safety limit"));
    }
    let bytes = response
        .bytes()
        .await
        .map_err(|error| format!("failed to read {label}: {error}"))?;
    if bytes.len() > MAX_MCP_OAUTH_METADATA_BYTES {
        return Err(format!("{label} exceeded the safety limit"));
    }
    serde_json::from_slice(&bytes).map_err(|error| format!("invalid {label} JSON: {error}"))
}

fn validate_mcp_oauth_url(
    raw: &str,
    label: &str,
    allow_loopback_http: bool,
) -> ExtensionResult<Url> {
    let url = Url::parse(raw).map_err(|error| format!("invalid {label}: {error}"))?;
    if url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        return Err(format!("invalid {label}"));
    }
    let loopback = is_loopback_url(&url);
    if url.scheme() != "https" && !(allow_loopback_http && url.scheme() == "http" && loopback) {
        return Err(format!("{label} must use HTTPS"));
    }
    Ok(url)
}

fn is_loopback_url(url: &Url) -> bool {
    matches!(
        url.host_str()
            .map(|host| host.to_ascii_lowercase())
            .as_deref(),
        Some("localhost" | "127.0.0.1" | "::1")
    )
}

fn same_url_origin(left: &Url, right: &Url) -> bool {
    left.scheme().eq_ignore_ascii_case(right.scheme())
        && left
            .host_str()
            .zip(right.host_str())
            .is_some_and(|(left, right)| left.eq_ignore_ascii_case(right))
        && left.port_or_known_default() == right.port_or_known_default()
}

fn canonical_mcp_resource(raw: &str) -> ExtensionResult<String> {
    let mut url = validate_mcp_oauth_url(raw, "MCP server URL", true)?;
    url.set_fragment(None);
    if url.path() == "/" && url.query().is_none() {
        return Ok(url.origin().ascii_serialization());
    }
    Ok(url.to_string())
}

fn normalize_issuer(value: &str) -> String {
    Url::parse(value.trim())
        .map(|url| url.to_string())
        .unwrap_or_else(|_| value.trim().to_string())
}

fn select_mcp_oauth_scopes(server: &McpServerConfig, discovery: &McpOAuthDiscovery) -> Vec<String> {
    let scopes = if !discovery.challenged_scopes.is_empty() {
        &discovery.challenged_scopes
    } else if !server.oauth_scopes.is_empty() {
        &server.oauth_scopes
    } else {
        &discovery.protected.scopes_supported
    };
    let mut selected = Vec::new();
    for scope in scopes {
        let scope = scope.trim();
        if !scope.is_empty() && !selected.iter().any(|item| item == scope) {
            selected.push(scope.to_string());
        }
    }
    selected
}

fn split_mcp_oauth_scopes(value: &str) -> Vec<String> {
    value
        .split_whitespace()
        .map(str::trim)
        .filter(|scope| !scope.is_empty())
        .map(ToString::to_string)
        .collect()
}

async fn dynamically_register_mcp_client(
    client: &reqwest::Client,
    endpoint: &str,
    redirect_uri: &str,
) -> ExtensionResult<String> {
    validate_mcp_oauth_url(endpoint, "MCP OAuth registration endpoint", false)?;
    let response = client
        .post(endpoint)
        .json(&json!({
            "client_name": "Halv",
            "client_uri": "https://halv.ai",
            "redirect_uris": [redirect_uri],
            "grant_types": ["authorization_code", "refresh_token"],
            "response_types": ["code"],
            "token_endpoint_auth_method": "none"
        }))
        .send()
        .await
        .map_err(|error| format!("MCP OAuth client registration failed: {error}"))?;
    let status = response.status();
    if !status.is_success() {
        return Err(mcp_oauth_http_error(response, "MCP OAuth client registration").await);
    }
    let registration: McpDynamicRegistrationResponse =
        parse_mcp_oauth_json_response(response, "MCP OAuth registration response").await?;
    if registration.client_id.trim().is_empty() || registration.client_id.len() > 2_048 {
        return Err("MCP OAuth registration returned an invalid client id".to_string());
    }
    if registration
        .token_endpoint_auth_method
        .as_deref()
        .is_some_and(|method| method != "none")
    {
        return Err(
            "MCP OAuth registration requires confidential client authentication, which is unsafe for a desktop app"
                .to_string(),
        );
    }
    Ok(registration.client_id)
}

fn build_mcp_oauth_authorize_url(
    endpoint: &str,
    client_id: &str,
    redirect_uri: &str,
    flow: &McpOAuthFlow,
    resource: &str,
    scopes: &[String],
) -> ExtensionResult<String> {
    let mut url = validate_mcp_oauth_url(endpoint, "MCP OAuth authorization endpoint", false)?;
    {
        let mut query = url.query_pairs_mut();
        query
            .append_pair("response_type", "code")
            .append_pair("client_id", client_id)
            .append_pair("redirect_uri", redirect_uri)
            .append_pair("code_challenge", &flow.challenge)
            .append_pair("code_challenge_method", "S256")
            .append_pair("state", &flow.state)
            .append_pair("resource", resource);
        if !scopes.is_empty() {
            query.append_pair("scope", &scopes.join(" "));
        }
    }
    Ok(url.to_string())
}

async fn wait_for_mcp_oauth_callback(
    listener: TcpListener,
    expected_state: &str,
) -> ExtensionResult<String> {
    tokio::time::timeout(
        std::time::Duration::from_secs(MCP_OAUTH_LOGIN_TIMEOUT_SECS),
        async {
            loop {
                let (mut stream, _) = listener
                    .accept()
                    .await
                    .map_err(|error| format!("MCP OAuth callback listener failed: {error}"))?;
                let mut buffer = [0_u8; 16 * 1024];
                let bytes_read = tokio::time::timeout(
                    std::time::Duration::from_secs(5),
                    stream.read(&mut buffer),
                )
                .await
                .map_err(|_| "MCP OAuth callback request timed out".to_string())?
                .map_err(|error| format!("failed to read MCP OAuth callback: {error}"))?;
                let result = parse_mcp_oauth_callback_request(
                    &String::from_utf8_lossy(&buffer[..bytes_read]),
                    expected_state,
                );
                match result {
                    Some(Ok(code)) => {
                        write_mcp_oauth_callback_response(
                            &mut stream,
                            "200 OK",
                            "Authorization complete. You can close this tab and return to Halv.",
                        )
                        .await;
                        return Ok(code);
                    }
                    Some(Err(error)) => {
                        write_mcp_oauth_callback_response(
                            &mut stream,
                            "400 Bad Request",
                            "Authorization was not completed. Return to Halv for details.",
                        )
                        .await;
                        return Err(error);
                    }
                    None => {
                        write_mcp_oauth_callback_response(
                            &mut stream,
                            "404 Not Found",
                            "Not found.",
                        )
                        .await;
                    }
                }
            }
        },
    )
    .await
    .map_err(|_| "timed out waiting for MCP OAuth authorization".to_string())?
}

fn parse_mcp_oauth_callback_request(
    request: &str,
    expected_state: &str,
) -> Option<ExtensionResult<String>> {
    let request_line = request.lines().next().unwrap_or_default();
    let mut parts = request_line.split_whitespace();
    if parts.next() != Some("GET") {
        return None;
    }
    let target = parts.next()?;
    let url = Url::parse(&format!("http://127.0.0.1{target}")).ok()?;
    if url.path() != MCP_OAUTH_CALLBACK_PATH {
        return None;
    }
    let query = url
        .query_pairs()
        .into_owned()
        .collect::<HashMap<String, String>>();
    if query.get("state").map(String::as_str) != Some(expected_state) {
        return Some(Err("MCP OAuth callback state did not match".to_string()));
    }
    if let Some(error) = query.get("error") {
        let description = query
            .get("error_description")
            .map(|description| format!(": {}", description.chars().take(300).collect::<String>()))
            .unwrap_or_default();
        return Some(Err(format!(
            "MCP OAuth authorization failed: {error}{description}"
        )));
    }
    Some(
        query
            .get("code")
            .filter(|code| !code.is_empty() && code.len() <= 16 * 1024)
            .cloned()
            .ok_or_else(|| "MCP OAuth callback omitted the authorization code".to_string()),
    )
}

async fn write_mcp_oauth_callback_response(
    stream: &mut tokio::net::TcpStream,
    status: &str,
    message: &str,
) {
    let body = format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><title>Halv</title></head><body><main><h1>Halv</h1><p>{message}</p></main></body></html>"
    );
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(response.as_bytes()).await;
    let _ = stream.shutdown().await;
}

async fn exchange_mcp_oauth_code(
    client: &reqwest::Client,
    token_endpoint: &str,
    client_id: &str,
    redirect_uri: &str,
    verifier: &str,
    code: &str,
    resource: &str,
) -> ExtensionResult<McpOAuthTokenResponse> {
    let params = [
        ("grant_type", "authorization_code"),
        ("code", code),
        ("redirect_uri", redirect_uri),
        ("client_id", client_id),
        ("code_verifier", verifier),
        ("resource", resource),
    ];
    post_mcp_oauth_token(client, token_endpoint, &params).await
}

async fn post_mcp_oauth_token(
    client: &reqwest::Client,
    token_endpoint: &str,
    params: &[(&str, &str)],
) -> ExtensionResult<McpOAuthTokenResponse> {
    validate_mcp_oauth_url(token_endpoint, "MCP OAuth token endpoint", false)?;
    let response = client
        .post(token_endpoint)
        .form(params)
        .send()
        .await
        .map_err(|error| format!("MCP OAuth token request failed: {error}"))?;
    let status = response.status();
    if !status.is_success() {
        return Err(mcp_oauth_http_error(response, "MCP OAuth token request").await);
    }
    let response: McpOAuthTokenResponse =
        parse_mcp_oauth_json_response(response, "MCP OAuth token response").await?;
    if response.access_token.trim().is_empty() || response.access_token.len() > 32 * 1024 {
        return Err("MCP OAuth token response contained an invalid access token".to_string());
    }
    if response
        .token_type
        .as_deref()
        .is_some_and(|token_type| !token_type.eq_ignore_ascii_case("bearer"))
    {
        return Err("MCP OAuth token response used an unsupported token type".to_string());
    }
    Ok(response)
}

async fn mcp_oauth_http_error(response: reqwest::Response, label: &str) -> String {
    let status = response.status();
    let text = response.text().await.unwrap_or_default();
    let detail = text
        .chars()
        .filter(|character| !character.is_control() || *character == ' ')
        .take(500)
        .collect::<String>();
    if detail.trim().is_empty() {
        format!("{label} returned HTTP {status}")
    } else {
        format!("{label} returned HTTP {status}: {detail}")
    }
}

fn mcp_oauth_credentials_from_response(
    response: McpOAuthTokenResponse,
    previous_refresh_token: Option<String>,
    client_id: String,
    token_endpoint: String,
    authorization_server: String,
    resource: String,
    requested_scopes: Vec<String>,
) -> ExtensionResult<McpOAuthCredentials> {
    let expires_at_unix = response.expires_in.map(|seconds| {
        Utc::now()
            .timestamp()
            .saturating_add(i64::try_from(seconds).unwrap_or(i64::MAX))
    });
    validate_mcp_oauth_credentials(McpOAuthCredentials {
        access_token: response.access_token,
        refresh_token: response.refresh_token.or(previous_refresh_token),
        token_type: response.token_type.or_else(|| Some("Bearer".to_string())),
        scope: response
            .scope
            .or_else(|| (!requested_scopes.is_empty()).then(|| requested_scopes.join(" "))),
        expires_at_unix,
        client_id,
        token_endpoint,
        authorization_server,
        resource,
    })
}

async fn refresh_mcp_oauth_credentials(
    server: &McpServerConfig,
    credentials: McpOAuthCredentials,
) -> ExtensionResult<McpOAuthCredentials> {
    let refresh_token = credentials.refresh_token.as_deref().ok_or_else(|| {
        "MCP OAuth token expired and cannot be refreshed; sign in again".to_string()
    })?;
    if credentials.client_id.is_empty()
        || credentials.token_endpoint.is_empty()
        || credentials.resource.is_empty()
    {
        return Err(
            "MCP OAuth token lacks refresh metadata; sign in again to upgrade it".to_string(),
        );
    }
    let client = mcp_oauth_http_client()?;
    let scope = credentials
        .scope
        .clone()
        .unwrap_or_else(|| server.oauth_scopes.join(" "));
    let mut params = vec![
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh_token),
        ("client_id", credentials.client_id.as_str()),
        ("resource", credentials.resource.as_str()),
    ];
    if !scope.trim().is_empty() {
        params.push(("scope", scope.as_str()));
    }
    let response = post_mcp_oauth_token(&client, &credentials.token_endpoint, &params).await?;
    mcp_oauth_credentials_from_response(
        response,
        credentials.refresh_token,
        credentials.client_id,
        credentials.token_endpoint,
        credentials.authorization_server,
        credentials.resource,
        split_mcp_oauth_scopes(&scope),
    )
}

pub fn selected_skill_instructions(
    manager: &ExtensionManager,
    root: Option<&Path>,
    trusted: bool,
    prompt: &str,
) -> ExtensionResult<Option<String>> {
    let project = root.map(|root| ProjectContext {
        root: root.to_path_buf(),
        trusted,
    });
    let selected = discover_skills(manager, project.as_ref())?
        .into_iter()
        .filter(|skill| skill.enabled && mentions_skill(prompt, &skill.name))
        .take(3)
        .collect::<Vec<_>>();
    if selected.is_empty() {
        return Ok(None);
    }
    let mut output = String::from("<selected_skills>\n");
    for skill in selected {
        output.push_str(&format!(
            "<skill name=\"{}\">\n{}\n</skill>\n",
            escape_xml(&skill.name),
            read_skill(Path::new(&skill.root))?
        ));
    }
    output.push_str("</selected_skills>");
    Ok(Some(output))
}

fn project_context(
    storage: &Storage,
    project_id: Option<i64>,
    workspace_id: Option<&str>,
) -> ExtensionResult<Option<ProjectContext>> {
    let Some(project_id) = project_id else {
        return Ok(None);
    };
    let project = storage
        .get_project(project_id)
        .map_err(|error| error.to_string())?;
    let root = if let Some(workspace_id) = workspace_id {
        let workspace = storage
            .get_workspace(workspace_id)
            .map_err(|error| error.to_string())?;
        if workspace.project_id != project_id {
            return Err("workspace belongs to another project".to_string());
        }
        projects::validate_registered_project_root(Path::new(&workspace.root_path))?
    } else {
        projects::validate_registered_project(&project)?
    };
    Ok(Some(ProjectContext {
        root,
        trusted: project.trust_level.as_deref() == Some("trusted"),
    }))
}

fn discover_skills(
    manager: &ExtensionManager,
    project: Option<&ProjectContext>,
) -> ExtensionResult<Vec<SkillDescriptor>> {
    let (preferences, plugins) =
        manager.read(|config| (config.skill_enabled.clone(), config.plugins.clone()));
    let mut roots = Vec::new();
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        roots.push(SkillRoot {
            root: home.join(".agents/skills"),
            scope: "global".to_string(),
            source: "agents".to_string(),
            plugin_id: None,
            plugin_relative_root: None,
        });
        roots.push(SkillRoot {
            root: home.join(".codex/skills"),
            scope: "global".to_string(),
            source: "codex".to_string(),
            plugin_id: None,
            plugin_relative_root: None,
        });
    }
    if let Some(project) = project.filter(|project| project.trusted) {
        roots.push(SkillRoot {
            root: project.root.join(".agents/skills"),
            scope: "project".to_string(),
            source: "agents".to_string(),
            plugin_id: None,
            plugin_relative_root: None,
        });
        roots.push(SkillRoot {
            root: project.root.join(".codex/skills"),
            scope: "project".to_string(),
            source: "codex".to_string(),
            plugin_id: None,
            plugin_relative_root: None,
        });
    }
    for plugin in plugins.into_iter().filter(|plugin| plugin.enabled) {
        roots.extend(plugin_skill_roots(
            Path::new(&plugin.root),
            &plugin.id,
            plugin_display_name(&plugin),
        )?);
    }
    let mut skills = Vec::new();
    for root in roots {
        scan_skill_root(&root, &preferences, &mut skills)?;
        if skills.len() >= MAX_SKILLS {
            break;
        }
    }
    skills.sort_by_key(|a| a.name.to_lowercase());
    skills.truncate(MAX_SKILLS);
    Ok(skills)
}

fn scan_skill_root(
    skill_source: &SkillRoot,
    preferences: &HashMap<String, bool>,
    skills: &mut Vec<SkillDescriptor>,
) -> ExtensionResult<()> {
    let root = &skill_source.root;
    if !root.is_dir() {
        return Ok(());
    }
    let root = root
        .canonicalize()
        .map_err(|error| format!("failed to inspect skill root: {error}"))?;
    let mut entries = fs::read_dir(&root)
        .map_err(|error| format!("failed to list skill root: {error}"))?
        .filter_map(Result::ok)
        .collect::<Vec<_>>();
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let metadata = fs::symlink_metadata(entry.path())
            .map_err(|error| format!("failed to inspect skill: {error}"))?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            continue;
        }
        let skill_root = entry
            .path()
            .canonicalize()
            .map_err(|error| format!("failed to canonicalize skill: {error}"))?;
        if !skill_root.starts_with(&root) {
            continue;
        }
        let skill_file = skill_root.join("SKILL.md");
        let metadata = match fs::symlink_metadata(&skill_file) {
            Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => metadata,
            _ => continue,
        };
        if metadata.len() > MAX_SKILL_BYTES {
            continue;
        }
        let contents = fs::read_to_string(skill_file)
            .map_err(|error| format!("failed to read skill: {error}"))?;
        let (name, description) = skill_metadata(&contents, &skill_root);
        let id = match skill_source.plugin_id.as_deref() {
            Some(plugin_id) => format!(
                "skill-plugin-{}",
                short_hash(&format!(
                    "{plugin_id}\0{}\0{}",
                    skill_source.plugin_relative_root.as_deref().unwrap_or(""),
                    entry.file_name().to_string_lossy()
                ))
            ),
            None => format!(
                "skill-{}-{}",
                skill_source.scope,
                short_hash(&skill_root.to_string_lossy())
            ),
        };
        let ui = read_skill_ui_metadata(&skill_root)?;
        let display_name = ui.display_name.unwrap_or_else(|| name.clone());
        let short_description = ui.short_description.unwrap_or_else(|| description.clone());
        skills.push(SkillDescriptor {
            enabled: preferences.get(&id).copied().unwrap_or(true),
            id,
            name,
            description,
            source: skill_source.source.clone(),
            scope: skill_source.scope.clone(),
            root: skill_root.to_string_lossy().into_owned(),
            fingerprint: short_hash(&contents),
            display_name,
            short_description,
            allow_implicit_invocation: ui.allow_implicit_invocation,
            default_prompt: ui.default_prompt,
            dependency_tools: ui.dependency_tools,
            plugin_id: skill_source.plugin_id.clone(),
        });
    }
    Ok(())
}

fn skill_metadata(contents: &str, root: &Path) -> (String, String) {
    let mut name = None;
    let mut description = None;
    let mut lines = contents.lines();
    if lines.next().map(str::trim) == Some("---") {
        for line in lines.by_ref() {
            let line = line.trim();
            if line == "---" {
                break;
            }
            if let Some(value) = line.strip_prefix("name:") {
                name = Some(metadata_value(value));
            } else if let Some(value) = line.strip_prefix("description:") {
                description = Some(metadata_value(value));
            }
        }
    }
    let fallback_name = root
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("skill")
        .to_string();
    let fallback_description = contents
        .lines()
        .find(|line| !line.trim().is_empty() && !line.trim().starts_with('#'))
        .map(|line| line.trim().chars().take(240).collect())
        .unwrap_or_else(|| "No description provided".to_string());
    (
        name.filter(|value| !value.is_empty())
            .unwrap_or(fallback_name),
        description
            .filter(|value| !value.is_empty())
            .unwrap_or(fallback_description),
    )
}

fn read_skill_ui_metadata(root: &Path) -> ExtensionResult<SkillUiMetadata> {
    let root = canonical_directory(root)?;
    let path = root.join("agents/openai.yaml");
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(SkillUiMetadata::default())
        }
        Err(error) => return Err(format!("failed to inspect skill UI metadata: {error}")),
    };
    if metadata.file_type().is_symlink() || !metadata.is_file() || metadata.len() > 128 * 1024 {
        return Err("skill UI metadata failed safety validation".to_string());
    }
    let path = path
        .canonicalize()
        .map_err(|error| format!("failed to resolve skill UI metadata: {error}"))?;
    if !path.starts_with(&root) {
        return Err("skill UI metadata escapes the skill root".to_string());
    }
    let contents = fs::read_to_string(path)
        .map_err(|error| format!("failed to read skill UI metadata: {error}"))?;
    let mut parsed = SkillUiMetadata::default();
    let mut section = "";
    let mut dependencies_subsection = "";
    let mut dependency_is_mcp = false;
    for raw in contents.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let indentation = raw.len().saturating_sub(raw.trim_start().len());
        if indentation == 0 && line.ends_with(':') {
            section = line.trim_end_matches(':');
            dependencies_subsection = "";
            dependency_is_mcp = false;
            continue;
        }
        match section {
            "interface" => {
                if let Some(value) = yaml_scalar(line, "display_name") {
                    parsed.display_name = Some(value);
                } else if let Some(value) = yaml_scalar(line, "short_description") {
                    parsed.short_description = Some(value);
                } else if let Some(value) = yaml_scalar(line, "default_prompt") {
                    parsed.default_prompt = Some(value);
                }
            }
            "policy" => {
                if let Some(value) = yaml_scalar(line, "allow_implicit_invocation") {
                    parsed.allow_implicit_invocation = !value.eq_ignore_ascii_case("false");
                }
            }
            "dependencies" => {
                if line == "tools:" {
                    dependencies_subsection = "tools";
                    continue;
                }
                if dependencies_subsection != "tools" {
                    continue;
                }
                if let Some(value) = line.strip_prefix("- type:") {
                    dependency_is_mcp = metadata_value(value) == "mcp";
                } else if dependency_is_mcp {
                    if let Some(value) = yaml_scalar(line, "value") {
                        if parsed.dependency_tools.len() < 32 {
                            parsed.dependency_tools.push(value);
                        }
                        dependency_is_mcp = false;
                    }
                }
            }
            _ => {}
        }
    }
    parsed.display_name = parsed
        .display_name
        .filter(|value| !value.is_empty())
        .map(|value| value.chars().take(200).collect());
    parsed.short_description = parsed
        .short_description
        .filter(|value| !value.is_empty())
        .map(|value| value.chars().take(500).collect());
    parsed.default_prompt = parsed
        .default_prompt
        .filter(|value| !value.is_empty())
        .map(|value| value.chars().take(1_000).collect());
    Ok(parsed)
}

fn yaml_scalar(line: &str, key: &str) -> Option<String> {
    line.strip_prefix(key)
        .and_then(|value| value.strip_prefix(':'))
        .map(metadata_value)
}

fn plugin_skill_roots(
    root: &Path,
    plugin_id: &str,
    plugin_name: &str,
) -> ExtensionResult<Vec<SkillRoot>> {
    let root = canonical_directory(root)?;
    let manifest = read_plugin_manifest(&root)?;
    let mut paths = vec![root.join("skills")];
    if let Some(value) = manifest.get("skills") {
        let relative = value
            .as_str()
            .ok_or_else(|| "plugin skills must be a relative path".to_string())?;
        paths.push(confined_plugin_path(&root, relative)?);
    }
    let mut roots = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for path in paths {
        if !path.exists() {
            continue;
        }
        let path = canonical_directory(&path)?;
        if !path.starts_with(&root) {
            return Err("plugin skill root escapes the plugin".to_string());
        }
        if !seen.insert(path.clone()) {
            continue;
        }
        let plugin_relative_root = path
            .strip_prefix(&root)
            .expect("validated plugin skill root should remain confined")
            .to_string_lossy()
            .into_owned();
        roots.push(SkillRoot {
            root: path,
            scope: "plugin".to_string(),
            source: format!("plugin:{plugin_name}"),
            plugin_id: Some(plugin_id.to_string()),
            plugin_relative_root: Some(plugin_relative_root),
        });
    }
    Ok(roots)
}

fn count_skill_directories(root: &Path) -> usize {
    fs::read_dir(root)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter(|entry| {
            fs::symlink_metadata(entry.path())
                .map(|metadata| {
                    metadata.is_dir()
                        && !metadata.file_type().is_symlink()
                        && entry.path().join("SKILL.md").is_file()
                })
                .unwrap_or(false)
        })
        .count()
}

fn metadata_value(value: &str) -> String {
    value
        .trim()
        .trim_matches(|character| character == '"' || character == '\'')
        .chars()
        .take(500)
        .collect()
}

fn read_skill(root: &Path) -> ExtensionResult<String> {
    let root = canonical_directory(root)?;
    let path = root.join("SKILL.md");
    let metadata =
        fs::symlink_metadata(&path).map_err(|error| format!("skill is unavailable: {error}"))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() || metadata.len() > MAX_SKILL_BYTES
    {
        return Err("skill failed safety validation".to_string());
    }
    fs::read_to_string(path).map_err(|error| format!("failed to read skill: {error}"))
}

fn validate_plugin(root: &Path) -> ExtensionResult<ValidatedPlugin> {
    let manifest = read_plugin_manifest(root)?;
    let name = manifest
        .get("name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "plugin manifest requires a name".to_string())?;
    let version = manifest
        .get("version")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("0.0.0");
    if name.len() > 100 {
        return Err("plugin name exceeds 100 characters".to_string());
    }
    if version.len() > 64
        || !version.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '.' | '-' | '_' | '+')
        })
    {
        return Err("plugin version is not safe for installation".to_string());
    }
    let id = slug(name);
    if id.is_empty() {
        return Err("plugin name must contain letters or numbers".to_string());
    }
    let interface = manifest.get("interface").and_then(Value::as_object);
    let description = manifest
        .get("description")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("No description provided")
        .chars()
        .take(1_000)
        .collect::<String>();
    let display_name = interface
        .and_then(|value| value.get("displayName"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(name)
        .chars()
        .take(200)
        .collect::<String>();
    let developer_name = interface
        .and_then(|value| value.get("developerName"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.chars().take(200).collect());
    let capabilities = interface
        .and_then(|value| value.get("capabilities"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .take(24)
        .map(|value| value.chars().take(100).collect())
        .collect();
    Ok(ValidatedPlugin {
        id,
        name: name.to_string(),
        version: version.to_string(),
        description,
        display_name,
        developer_name,
        capabilities,
    })
}

fn read_plugin_manifest(root: &Path) -> ExtensionResult<Value> {
    let root = canonical_directory(root)?;
    let path = root.join(".codex-plugin/plugin.json");
    let metadata = fs::symlink_metadata(&path)
        .map_err(|_| "plugin must contain .codex-plugin/plugin.json".to_string())?;
    if metadata.file_type().is_symlink() || !metadata.is_file() || metadata.len() > 128 * 1024 {
        return Err("plugin manifest failed safety validation".to_string());
    }
    let path = path
        .canonicalize()
        .map_err(|error| format!("failed to resolve plugin manifest: {error}"))?;
    if !path.starts_with(&root) {
        return Err("plugin manifest escapes the plugin root".to_string());
    }
    let manifest: Value = serde_json::from_slice(
        &fs::read(path).map_err(|error| format!("failed to read manifest: {error}"))?,
    )
    .map_err(|error| format!("invalid plugin manifest: {error}"))?;
    if !manifest.is_object() {
        return Err("plugin manifest must contain a JSON object".to_string());
    }
    Ok(manifest)
}

fn parse_plugin_mcp_servers(
    root: &Path,
    plugin_id: &str,
    plugin_name: &str,
    strict: bool,
) -> ExtensionResult<Vec<McpServerConfig>> {
    match parse_plugin_mcp_servers_inner(root, plugin_id, plugin_name) {
        Ok(servers) => Ok(servers),
        Err(_error) if !strict => Ok(Vec::new()),
        Err(error) => Err(error),
    }
}

fn parse_plugin_mcp_servers_inner(
    root: &Path,
    plugin_id: &str,
    plugin_name: &str,
) -> ExtensionResult<Vec<McpServerConfig>> {
    let root = canonical_directory(root)?;
    let manifest = read_plugin_manifest(&root)?;
    let entries = if let Some(config) = manifest.get("mcpServers") {
        match config {
            Value::Object(entries) => Some(entries.clone()),
            Value::String(path) => {
                let payload = read_plugin_json(&root, path, "plugin MCP config")?;
                Some(
                    payload
                        .get("mcpServers")
                        .and_then(Value::as_object)
                        .cloned()
                        .ok_or_else(|| {
                            "plugin MCP config requires an mcpServers object".to_string()
                        })?,
                )
            }
            _ => return Err("plugin mcpServers must be an object or relative path".to_string()),
        }
    } else if root.join(".mcp.json").is_file() {
        let payload = read_plugin_json(&root, ".mcp.json", "plugin MCP config")?;
        Some(
            payload
                .get("mcpServers")
                .and_then(Value::as_object)
                .cloned()
                .ok_or_else(|| "plugin MCP config requires an mcpServers object".to_string())?,
        )
    } else {
        None
    };
    let Some(entries) = entries else {
        return Ok(Vec::new());
    };
    let mut servers = Vec::new();
    for (name, raw) in entries {
        let raw = raw
            .as_object()
            .ok_or_else(|| format!("plugin MCP server `{name}` must be an object"))?;
        servers.push(parse_plugin_mcp_server(
            &root,
            plugin_id,
            plugin_name,
            &name,
            raw,
        )?);
    }
    servers.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(servers)
}

fn parse_plugin_mcp_server(
    root: &Path,
    plugin_id: &str,
    plugin_name: &str,
    server_name: &str,
    raw: &serde_json::Map<String, Value>,
) -> ExtensionResult<McpServerConfig> {
    let server_name = server_name.trim();
    if server_name.is_empty() || server_name.len() > 100 {
        return Err("plugin MCP server names must be 1 to 100 characters".to_string());
    }
    let command = json_string(raw, "command");
    let url = json_string(raw, "url");
    let transport = match json_string(raw, "type").as_deref() {
        Some("http" | "streamable_http") => McpTransport::StreamableHttp,
        Some("stdio") => McpTransport::Stdio,
        Some(other) => return Err(format!("unsupported plugin MCP transport `{other}`")),
        None if command.is_some() => McpTransport::Stdio,
        None if url.is_some() => McpTransport::StreamableHttp,
        None => {
            return Err(format!(
                "plugin MCP server `{server_name}` requires a command or URL"
            ))
        }
    };
    let command = command
        .map(|command| resolve_plugin_command(root, &command))
        .transpose()?;
    let cwd = if matches!(transport, McpTransport::Stdio) {
        let configured = json_string(raw, "cwd").unwrap_or_else(|| ".".to_string());
        Some(resolve_plugin_directory(root, &configured)?)
    } else {
        None
    };
    let env = json_string_map(raw.get("env"), "env")?;
    let env_vars = json_string_array(raw.get("env_vars").or_else(|| raw.get("envVars")))?;
    let headers = json_string_map(
        raw.get("http_headers").or_else(|| raw.get("headers")),
        "http_headers",
    )?;
    let env_http_headers = json_string_map(raw.get("env_http_headers"), "env_http_headers")?;
    let bearer_token_env_var = json_string(raw, "bearer_token_env_var");
    let auth_mode = match json_string(raw, "auth").as_deref() {
        Some("oauth") => McpAuthMode::Oauth,
        Some("bearer") => McpAuthMode::Bearer,
        Some("none") => McpAuthMode::None,
        None if bearer_token_env_var.is_some() => McpAuthMode::Bearer,
        None if matches!(transport, McpTransport::StreamableHttp) => McpAuthMode::Oauth,
        None => McpAuthMode::None,
        Some(other) => return Err(format!("unsupported plugin MCP auth mode `{other}`")),
    };
    let approval_mode = json_string(raw, "default_tools_approval_mode")
        .or_else(|| json_string(raw, "approval_mode"))
        .unwrap_or_else(|| "prompt".to_string());
    let mut tool_approval_modes = BTreeMap::new();
    if let Some(tools) = raw.get("tools").and_then(Value::as_object) {
        for (name, config) in tools {
            if let Some(mode) = config
                .get("approval_mode")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
            {
                tool_approval_modes.insert(name.clone(), mode.to_string());
            }
        }
    }
    let id = format!(
        "plugin-{}-{}-{}",
        slug(plugin_id).chars().take(24).collect::<String>(),
        slug(server_name).chars().take(24).collect::<String>(),
        &short_hash(&format!("{plugin_id}\0{server_name}"))[..8]
    );
    let mut server = McpServerConfig {
        id,
        name: format!("{plugin_name}: {server_name}"),
        transport,
        command,
        args: json_string_array(raw.get("args"))?,
        cwd,
        env,
        env_vars,
        url,
        headers,
        env_http_headers,
        bearer_token_env_var,
        auth_mode,
        oauth_scopes: json_string_array(raw.get("oauth_scopes").or_else(|| raw.get("scopes")))?,
        oauth_client_id: json_string(
            raw,
            if raw.contains_key("oauth_client_id") {
                "oauth_client_id"
            } else {
                "oauthClientId"
            },
        ),
        enabled: raw.get("enabled").and_then(Value::as_bool).unwrap_or(true),
        required: raw
            .get("required")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        startup_timeout_ms: json_timeout_ms(
            raw,
            "startup_timeout_ms",
            "startup_timeout_sec",
            10_000,
        ),
        tool_timeout_ms: json_timeout_ms(raw, "tool_timeout_ms", "tool_timeout_sec", 60_000),
        enabled_tools: json_string_array(raw.get("enabled_tools"))?,
        disabled_tools: json_string_array(raw.get("disabled_tools"))?,
        approval_mode,
        tool_approval_modes,
        trust_tool_annotations: raw
            .get("trust_tool_annotations")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        source: McpServerSource::Plugin,
        plugin_id: Some(plugin_id.to_string()),
        plugin_name: Some(plugin_name.to_string()),
    };
    validate_mcp(&mut server)?;
    Ok(server)
}

fn read_plugin_json(root: &Path, relative: &str, label: &str) -> ExtensionResult<Value> {
    let path = confined_plugin_path(root, relative)?;
    let metadata =
        fs::symlink_metadata(&path).map_err(|error| format!("{label} is unavailable: {error}"))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() || metadata.len() > 256 * 1024 {
        return Err(format!("{label} failed safety validation"));
    }
    let path = path
        .canonicalize()
        .map_err(|error| format!("failed to resolve {label}: {error}"))?;
    if !path.starts_with(root) {
        return Err(format!("{label} escapes the plugin root"));
    }
    let value: Value = serde_json::from_slice(
        &fs::read(path).map_err(|error| format!("failed to read {label}: {error}"))?,
    )
    .map_err(|error| format!("invalid {label}: {error}"))?;
    if !value.is_object() {
        return Err(format!("{label} must contain a JSON object"));
    }
    Ok(value)
}

fn confined_plugin_path(root: &Path, relative: &str) -> ExtensionResult<PathBuf> {
    let relative = Path::new(relative.trim());
    if relative.as_os_str().is_empty() || relative.is_absolute() {
        return Err("plugin component paths must be relative".to_string());
    }
    if relative
        .components()
        .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        return Err("plugin component paths cannot escape the plugin".to_string());
    }
    Ok(root.join(relative))
}

fn resolve_plugin_directory(root: &Path, relative: &str) -> ExtensionResult<String> {
    let path = confined_plugin_path(root, relative)?;
    let canonical = canonical_directory(&path)?;
    if !canonical.starts_with(root) {
        return Err("plugin MCP cwd escapes the plugin".to_string());
    }
    Ok(canonical.to_string_lossy().into_owned())
}

fn resolve_plugin_command(root: &Path, command: &str) -> ExtensionResult<String> {
    let path = Path::new(command);
    if path.is_absolute() {
        return Err("plugin MCP commands cannot use absolute paths".to_string());
    }
    if path.components().count() == 1 {
        return Ok(command.to_string());
    }
    let path = confined_plugin_path(root, command)?;
    let metadata = fs::symlink_metadata(&path)
        .map_err(|error| format!("plugin MCP command is unavailable: {error}"))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err("plugin MCP command failed safety validation".to_string());
    }
    let canonical = path
        .canonicalize()
        .map_err(|error| format!("failed to resolve plugin MCP command: {error}"))?;
    if !canonical.starts_with(root) {
        return Err("plugin MCP command escapes the plugin".to_string());
    }
    Ok(canonical.to_string_lossy().into_owned())
}

fn json_string(raw: &serde_json::Map<String, Value>, key: &str) -> Option<String> {
    raw.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn json_string_array(value: Option<&Value>) -> ExtensionResult<Vec<String>> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let values = value
        .as_array()
        .ok_or_else(|| "MCP list values must be arrays".to_string())?;
    values
        .iter()
        .map(|value| {
            value
                .as_str()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToString::to_string)
                .ok_or_else(|| "MCP list entries must be non-empty strings".to_string())
        })
        .collect()
}

fn json_string_map(
    value: Option<&Value>,
    label: &str,
) -> ExtensionResult<BTreeMap<String, String>> {
    let Some(value) = value else {
        return Ok(BTreeMap::new());
    };
    let values = value
        .as_object()
        .ok_or_else(|| format!("MCP {label} must be an object"))?;
    values
        .iter()
        .map(|(name, value)| {
            let value = value
                .as_str()
                .ok_or_else(|| format!("MCP {label} values must be strings"))?;
            Ok((name.clone(), value.to_string()))
        })
        .collect()
}

fn json_timeout_ms(
    raw: &serde_json::Map<String, Value>,
    milliseconds_key: &str,
    seconds_key: &str,
    fallback: u64,
) -> u64 {
    raw.get(milliseconds_key)
        .and_then(Value::as_u64)
        .or_else(|| {
            raw.get(seconds_key)
                .and_then(Value::as_u64)
                .and_then(|seconds| seconds.checked_mul(1_000))
        })
        .unwrap_or(fallback)
}

fn copy_plugin(source: &Path, destination: &Path) -> ExtensionResult<()> {
    let mut stack = vec![(source.to_path_buf(), destination.to_path_buf())];
    let mut files = 0_usize;
    let mut bytes = 0_u64;
    while let Some((from, to)) = stack.pop() {
        fs::create_dir_all(&to)
            .map_err(|error| format!("failed to create plugin directory: {error}"))?;
        for entry in
            fs::read_dir(&from).map_err(|error| format!("failed to list plugin: {error}"))?
        {
            let entry = entry.map_err(|error| format!("failed to inspect plugin: {error}"))?;
            if entry.file_name() == ".git" {
                continue;
            }
            let metadata = fs::symlink_metadata(entry.path())
                .map_err(|error| format!("failed to inspect plugin: {error}"))?;
            if metadata.file_type().is_symlink() {
                return Err("plugins cannot contain symlinks".to_string());
            }
            let target = to.join(entry.file_name());
            if metadata.is_dir() {
                stack.push((entry.path(), target));
            } else if metadata.is_file() {
                files += 1;
                bytes = bytes.saturating_add(metadata.len());
                if files > MAX_PLUGIN_FILES || bytes > MAX_PLUGIN_BYTES {
                    return Err("plugin exceeds file or byte safety limits".to_string());
                }
                fs::copy(entry.path(), target)
                    .map_err(|error| format!("failed to copy plugin: {error}"))?;
            }
        }
    }
    Ok(())
}

fn unsupported_components(root: &Path) -> Vec<String> {
    [
        ("hooks", root.join("hooks")),
        ("apps", root.join(".app.json")),
    ]
    .into_iter()
    .filter(|(_, path)| path.exists())
    .map(|(name, _)| name.to_string())
    .collect()
}

fn discover_codex_plugin_candidates(config: &ExtensionConfig) -> Vec<PluginCandidate> {
    let Some(home) = std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".codex")))
    else {
        return Vec::new();
    };
    let root = home.join("plugins").join("cache");
    let Ok(root) = root.canonicalize() else {
        return Vec::new();
    };
    let installed = config
        .plugins
        .iter()
        .map(|plugin| plugin.id.as_str())
        .collect::<std::collections::HashSet<_>>();
    let mut stack = vec![(root, 0_usize)];
    let mut inspected = 0_usize;
    let mut candidates = BTreeMap::<String, PluginCandidate>::new();
    while let Some((directory, depth)) = stack.pop() {
        inspected = inspected.saturating_add(1);
        if inspected > 10_000 {
            break;
        }
        if directory.join(".codex-plugin/plugin.json").is_file() {
            if let Ok(validated) = validate_plugin(&directory) {
                let candidate = PluginCandidate {
                    id: validated.id.clone(),
                    name: validated.name,
                    display_name: validated.display_name,
                    version: validated.version,
                    description: validated.description,
                    path: directory.to_string_lossy().into_owned(),
                    capabilities: validated.capabilities,
                    installed: installed.contains(validated.id.as_str()),
                };
                let replace = candidates
                    .get(&candidate.id)
                    .is_none_or(|current| candidate.version > current.version);
                if replace {
                    candidates.insert(candidate.id.clone(), candidate);
                }
            }
            continue;
        }
        if depth >= 4 {
            continue;
        }
        let Ok(entries) = fs::read_dir(&directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(metadata) = fs::symlink_metadata(entry.path()) else {
                continue;
            };
            if metadata.is_dir() && !metadata.file_type().is_symlink() {
                stack.push((entry.path(), depth.saturating_add(1)));
            }
        }
    }
    let mut candidates = candidates.into_values().collect::<Vec<_>>();
    candidates.sort_by(|left, right| {
        left.display_name
            .to_lowercase()
            .cmp(&right.display_name.to_lowercase())
            .then_with(|| left.id.cmp(&right.id))
    });
    candidates
}

fn validate_mcp(server: &mut McpServerConfig) -> ExtensionResult<()> {
    server.name = server.name.trim().to_string();
    if server.name.is_empty() || server.name.len() > 100 {
        return Err("MCP server name is required".to_string());
    }
    if server.id.trim().is_empty() {
        server.id = format!("mcp-{:032x}", rand::random::<u128>());
    }
    if server.id.len() > 200
        || !server
            .id
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
    {
        return Err("invalid MCP server id".to_string());
    }
    if server.args.len() > 128
        || server.enabled_tools.len() > 256
        || server.disabled_tools.len() > 256
        || server.oauth_scopes.len() > 64
        || server.tool_approval_modes.len() > 256
        || server.env.len() > 128
        || server.env_http_headers.len() > 128
    {
        return Err("MCP list limits exceeded".to_string());
    }
    if matches!(server.source, McpServerSource::User) && !server.env.is_empty() {
        return Err(
            "standalone MCP environment values cannot be stored; use envVars for secret names"
                .to_string(),
        );
    }
    server.startup_timeout_ms = server.startup_timeout_ms.clamp(1_000, 120_000);
    server.tool_timeout_ms = server.tool_timeout_ms.clamp(1_000, 600_000);
    if !matches!(
        server.approval_mode.as_str(),
        "auto" | "prompt" | "writes" | "approve"
    ) {
        return Err("invalid MCP approval mode".to_string());
    }
    for mode in server.tool_approval_modes.values() {
        if !matches!(mode.as_str(), "auto" | "prompt" | "writes" | "approve") {
            return Err("invalid per-tool MCP approval mode".to_string());
        }
    }
    if server.oauth_scopes.iter().any(|scope| {
        scope.trim().is_empty()
            || scope.len() > 256
            || scope.chars().any(char::is_whitespace)
            || scope.chars().any(char::is_control)
    }) {
        return Err("invalid MCP OAuth scope".to_string());
    }
    server.oauth_client_id = server
        .oauth_client_id
        .take()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    if server
        .oauth_client_id
        .as_ref()
        .is_some_and(|client_id| client_id.len() > 2_048 || client_id.chars().any(char::is_control))
    {
        return Err("invalid MCP OAuth client id".to_string());
    }
    for variable in server
        .env_vars
        .iter()
        .chain(server.bearer_token_env_var.iter())
        .chain(server.env_http_headers.values())
    {
        if variable.is_empty()
            || !variable
                .chars()
                .all(|character| character.is_ascii_alphanumeric() || character == '_')
        {
            return Err("MCP env_vars must contain names, never values".to_string());
        }
    }
    for (name, value) in &server.env {
        if name.is_empty()
            || !name
                .chars()
                .all(|character| character.is_ascii_alphanumeric() || character == '_')
            || value.len() > 16 * 1024
        {
            return Err("MCP env values failed safety validation".to_string());
        }
    }
    for (name, value) in &server.headers {
        let name = name.to_ascii_lowercase();
        if matches!(
            name.as_str(),
            "authorization" | "proxy-authorization" | "cookie" | "set-cookie" | "x-api-key"
        ) || value.len() > 4_096
        {
            return Err("MCP secrets cannot be stored in static headers".to_string());
        }
    }
    for name in server.env_http_headers.keys() {
        if name.trim().is_empty() || name.len() > 256 {
            return Err("MCP environment-backed HTTP header name is invalid".to_string());
        }
    }
    match server.transport {
        McpTransport::Stdio => {
            if server
                .command
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .is_none()
            {
                return Err("STDIO MCP servers require a command".to_string());
            }
            server.url = None;
            server.headers.clear();
            server.env_http_headers.clear();
            server.bearer_token_env_var = None;
            server.auth_mode = McpAuthMode::None;
            server.oauth_scopes.clear();
            server.oauth_client_id = None;
        }
        McpTransport::StreamableHttp => {
            if matches!(server.auth_mode, McpAuthMode::Oauth)
                && server.bearer_token_env_var.is_some()
            {
                return Err(
                    "OAuth MCP servers cannot also configure a bearer token environment variable"
                        .to_string(),
                );
            }
            if matches!(server.auth_mode, McpAuthMode::None)
                && server.bearer_token_env_var.is_some()
            {
                server.auth_mode = McpAuthMode::Bearer;
            }
            let url = Url::parse(
                server
                    .url
                    .as_deref()
                    .ok_or_else(|| "HTTP MCP servers require a URL".to_string())?,
            )
            .map_err(|error| format!("invalid MCP URL: {error}"))?;
            if !url.username().is_empty() || url.password().is_some() {
                return Err("MCP URLs cannot contain credentials".to_string());
            }
            let loopback = matches!(url.host_str(), Some("127.0.0.1" | "::1" | "[::1]"));
            if url.scheme() != "https" && !(url.scheme() == "http" && loopback) {
                return Err("remote MCP URLs must use HTTPS".to_string());
            }
            server.command = None;
            server.args.clear();
            server.cwd = None;
            server.env.clear();
            server.env_vars.clear();
        }
    }
    Ok(())
}

fn load_config(data_dir: &Path) -> ExtensionResult<ExtensionConfig> {
    let path = data_dir.join(CONFIG_FILE);
    if !path.exists() {
        return Ok(ExtensionConfig::default());
    }
    serde_json::from_slice(
        &fs::read(path).map_err(|error| format!("failed to read extensions: {error}"))?,
    )
    .map_err(|error| format!("invalid extension config: {error}"))
}

fn save_config(data_dir: &Path, config: &ExtensionConfig) -> ExtensionResult<()> {
    let temp = data_dir.join(format!(".{CONFIG_FILE}.tmp-{:016x}", rand::random::<u64>()));
    let mut file = app_data::private_file_options()
        .write(true)
        .create(true)
        .truncate(true)
        .open(&temp)
        .map_err(|error| format!("failed to create extension config: {error}"))?;
    file.write_all(
        &serde_json::to_vec_pretty(config)
            .map_err(|error| format!("failed to encode extensions: {error}"))?,
    )
    .and_then(|()| file.sync_all())
    .map_err(|error| format!("failed to write extensions: {error}"))?;
    fs::rename(temp, data_dir.join(CONFIG_FILE))
        .map_err(|error| format!("failed to replace extension config: {error}"))
}

fn canonical_directory(path: &Path) -> ExtensionResult<PathBuf> {
    let metadata =
        fs::symlink_metadata(path).map_err(|error| format!("directory is unavailable: {error}"))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err("path must be a real directory".to_string());
    }
    path.canonicalize()
        .map_err(|error| format!("failed to canonicalize directory: {error}"))
}

fn mentions_skill(prompt: &str, name: &str) -> bool {
    prompt
        .to_lowercase()
        .contains(&format!("${}", name.to_lowercase()))
}

fn short_hash(value: &str) -> String {
    Sha256::digest(value.as_bytes())
        .iter()
        .take(8)
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn slug(value: &str) -> String {
    value
        .to_ascii_lowercase()
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
                character
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .to_string()
}

fn escape_xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crux_savings_estimates_match_post_consolidation_table() {
        assert_eq!(
            CRUX_SAVINGS_ESTIMATES
                .iter()
                .map(|(tool, estimate)| (*tool, *estimate))
                .collect::<Vec<_>>(),
            vec![
                ("scip_index", 0),
                ("scip_find", 400),
                ("scip_map", 1_900),
                ("scip_outline", 250),
                ("scip_expand", 0),
            ]
        );
        assert_eq!(
            crux_savings_estimate("builtin.crux", "scip_map"),
            Some(1_900)
        );
        assert_eq!(crux_savings_estimate("crux", "scip_find"), Some(400));
        assert_eq!(crux_savings_estimate("other-server", "scip_map"), None);
        assert_eq!(crux_savings_estimate("builtin.crux", "unknown"), None);
    }

    #[test]
    fn crux_document_count_reads_stored_tool_stats() {
        let nested = r#"{"content":[{"type":"text","text":"documents 1,234 | symbols 99"}]}"#;
        assert_eq!(crux_document_count(nested), Some(1_234));
        assert_eq!(crux_document_count(r#"{"documentCount":42}"#), Some(42));
        assert_eq!(crux_document_count("symbols 99"), None);
    }

    fn test_server(transport: McpTransport) -> McpServerConfig {
        let (command, url) = match &transport {
            McpTransport::Stdio => (Some("example".to_string()), None),
            McpTransport::StreamableHttp => (None, Some("https://example.com/mcp".to_string())),
        };
        McpServerConfig {
            id: "server-1".to_string(),
            name: "Example".to_string(),
            transport,
            command,
            args: vec![],
            cwd: None,
            env: BTreeMap::new(),
            env_vars: vec![],
            url,
            headers: BTreeMap::new(),
            env_http_headers: BTreeMap::new(),
            bearer_token_env_var: None,
            auth_mode: McpAuthMode::None,
            oauth_scopes: vec![],
            oauth_client_id: None,
            enabled: true,
            required: false,
            startup_timeout_ms: 10_000,
            tool_timeout_ms: 60_000,
            enabled_tools: vec![],
            disabled_tools: vec![],
            approval_mode: "prompt".to_string(),
            tool_approval_modes: BTreeMap::new(),
            trust_tool_annotations: false,
            source: McpServerSource::User,
            plugin_id: None,
            plugin_name: None,
        }
    }

    fn test_census(files: u64, lines: u64) -> CruxProjectCensus {
        CruxProjectCensus {
            languages: BTreeMap::from([("rust".to_string(), CruxLanguageCensus { files, lines })]),
        }
    }

    fn test_language_census(languages: &[(&str, u64, u64)]) -> CruxProjectCensus {
        CruxProjectCensus {
            languages: languages
                .iter()
                .map(|(family, files, lines)| {
                    (
                        (*family).to_string(),
                        CruxLanguageCensus {
                            files: *files,
                            lines: *lines,
                        },
                    )
                })
                .collect(),
        }
    }

    #[test]
    fn crux_gate_applies_census_thresholds_and_activation_modes() {
        assert!(!crux_gate_passes(
            CruxActivation::Auto,
            Some(&test_census(199, 74_999)),
        ));
        assert!(crux_gate_passes(
            CruxActivation::Auto,
            Some(&test_census(200, 1)),
        ));
        assert!(crux_gate_passes(
            CruxActivation::Auto,
            Some(&test_census(1, 75_000)),
        ));
        assert!(!crux_gate_passes(CruxActivation::Auto, None));
        assert!(crux_gate_passes(CruxActivation::AlwaysOn, None));
        assert!(!crux_gate_passes(
            CruxActivation::AlwaysOff,
            Some(&test_census(1_000, 1_000_000)),
        ));
    }

    #[test]
    fn crux_gate_requires_a_single_language_family_to_pass() {
        let mut census = CruxProjectCensus {
            languages: BTreeMap::from([
                (
                    "typescript_javascript".to_string(),
                    CruxLanguageCensus {
                        files: 150,
                        lines: 60_000,
                    },
                ),
                (
                    "python".to_string(),
                    CruxLanguageCensus {
                        files: 150,
                        lines: 60_000,
                    },
                ),
            ]),
        };

        assert!(!crux_gate_passes(CruxActivation::Auto, Some(&census)));
        census
            .languages
            .get_mut("typescript_javascript")
            .expect("the TypeScript and JavaScript family should exist")
            .files = 200;
        assert!(crux_gate_passes(CruxActivation::Auto, Some(&census)));
    }

    #[test]
    fn crux_enabled_for_project_rejects_an_unavailable_manager() {
        assert!(!crux_enabled_for_project(None, Some(Path::new("/tmp"))));
    }

    #[test]
    fn crux_enabled_for_project_accepts_an_above_threshold_census() {
        let manager = ExtensionManager::default();
        let root = Path::new("/tmp");
        manager.set_crux_test_state(root, CruxActivation::Auto, true, test_census(200, 1));

        assert!(crux_enabled_for_project(Some(&manager), Some(root)));
    }

    #[test]
    fn crux_enabled_for_project_rejects_a_below_threshold_census() {
        let manager = ExtensionManager::default();
        let root = Path::new("/tmp");
        manager.set_crux_test_state(root, CruxActivation::Auto, true, test_census(199, 74_999));

        assert!(!crux_enabled_for_project(Some(&manager), Some(root)));
    }

    #[test]
    fn crux_enabled_for_project_rejects_an_unapproved_project() {
        let manager = ExtensionManager::default();
        let root = Path::new("/tmp");
        manager.set_crux_test_state(root, CruxActivation::AlwaysOn, true, test_census(200, 1));
        manager.set_crux_approved(root, false);

        assert!(!crux_enabled_for_project(Some(&manager), Some(root)));
    }

    #[test]
    fn crux_indexing_requires_project_approval() {
        let mut config = ExtensionConfig::default();
        let above_gate = test_census(200, 1);
        let below_gate = test_census(199, 74_999);

        assert!(!crux_should_index(&config, &above_gate, false));
        assert!(crux_should_index(&config, &above_gate, true));
        assert!(!crux_should_index(&config, &below_gate, true));
        config.crux_activation = CruxActivation::AlwaysOn;
        assert!(!crux_should_index(&config, &below_gate, false));
        assert!(crux_should_index(&config, &below_gate, true));
        config
            .builtin_mcp_enabled
            .insert(BUILTIN_CRUX_ID.to_string(), false);
        assert!(!crux_should_index(&config, &above_gate, true));
    }

    fn crux_approval_test_storage(name: &str) -> (PathBuf, Storage) {
        let base = std::env::temp_dir()
            .canonicalize()
            .unwrap()
            .join(format!("halv-crux-{name}-{}", rand::random::<u64>()));
        fs::create_dir_all(base.join("project")).unwrap();
        let storage = Storage::open(base.join("halv.sqlite3")).unwrap();
        (base, storage)
    }

    fn record_crux_index(storage: &Storage, root: &Path) {
        storage
            .upsert_crux_index_state(&CruxIndexState {
                project_path: root.to_string_lossy().into_owned(),
                last_indexed_git_head: None,
                last_index_stats: r#"{"documentCount":7}"#.to_string(),
                last_index_time: "2026-10-14T00:00:00Z".to_string(),
            })
            .unwrap();
    }

    #[test]
    fn crux_indexing_approval_needs_a_stored_approval_or_an_earlier_index() {
        let (base, storage) = crux_approval_test_storage("approval");
        let root = base.join("project");
        let worktree = base.join("worktree");
        fs::create_dir_all(root.join(".scip-nav")).unwrap();
        fs::create_dir_all(&worktree).unwrap();
        let project = storage
            .upsert_project("project", &root.to_string_lossy())
            .unwrap();
        let mut config = ExtensionConfig::default();

        // The repository controls this file, so it does not approve indexing.
        fs::write(root.join(CRUX_INDEX_RELATIVE_PATH), b"index").unwrap();
        assert!(!crux_indexing_approved(
            &config, &storage, project.id, &root
        ));
        assert!(!crux_indexing_approved(
            &config, &storage, project.id, &worktree
        ));

        config.crux_approved_projects.insert(project.id);
        assert!(crux_indexing_approved(&config, &storage, project.id, &root));
        assert!(crux_indexing_approved(
            &config, &storage, project.id, &worktree
        ));

        let config = ExtensionConfig::default();
        record_crux_index(&storage, &root);
        assert!(crux_indexing_approved(&config, &storage, project.id, &root));
        assert!(crux_indexing_approved(
            &config, &storage, project.id, &worktree
        ));
        let _ = fs::remove_dir_all(base);
    }

    fn crux_approval_test_manager(base: &Path) -> ExtensionManager {
        let manager = ExtensionManager::default();
        manager.set_app_data_dir(base.join("data")).unwrap();
        manager
    }

    fn approve_crux_project(manager: &ExtensionManager, project_id: i64) {
        manager
            .update(|config, _| {
                config.crux_approved_projects.insert(project_id);
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn crux_approval_ends_when_its_project_is_removed() {
        let (base, storage) = crux_approval_test_storage("remove");
        let manager = crux_approval_test_manager(&base);
        let root = base.join("project");
        let other_root = base.join("other");
        fs::create_dir_all(&other_root).unwrap();
        let project = storage
            .upsert_project("project", &root.to_string_lossy())
            .unwrap();
        approve_crux_project(&manager, project.id);
        record_crux_index(&storage, &root);
        manager.set_crux_approved(&root, true);
        assert!(crux_indexing_approved(
            &manager.read(Clone::clone),
            &storage,
            project.id,
            &root
        ));

        crate::projects::remove_registered_project(&storage, &manager, project.id).unwrap();

        // SQLite gives the id of the removed project to the next project.
        let other = storage
            .upsert_project("other", &other_root.to_string_lossy())
            .unwrap();
        assert_eq!(other.id, project.id);
        let config = manager.read(Clone::clone);
        assert!(!crux_indexing_approved(
            &config,
            &storage,
            other.id,
            &other_root
        ));
        assert!(load_config(&base.join("data"))
            .unwrap()
            .crux_approved_projects
            .is_empty());
        assert!(!manager.crux_approved(&root));
        assert!(storage
            .crux_index_state(&root.to_string_lossy())
            .unwrap()
            .is_none());

        // A project at the old root asks for approval again.
        let readded = storage
            .upsert_project("project", &root.to_string_lossy())
            .unwrap();
        assert!(!crux_indexing_approved(
            &config, &storage, readded.id, &root
        ));
        let _ = fs::remove_dir_all(base);
    }

    #[test]
    fn crux_task_sees_an_approval_that_comes_while_the_task_runs() {
        let (base, storage) = crux_approval_test_storage("task");
        let manager = crux_approval_test_manager(&base);
        // crux_approve_indexing stores the approval, then schedules a forced task.
        let approve = |project_id: i64, root: &Path| {
            approve_crux_project(&manager, project_id);
            assert!(!manager.begin_crux_task(root, true));
        };

        // The user approves during the census. The task sees the approval
        // and indexes, so it does not run again.
        let early_root = base.join("project");
        let early = storage
            .upsert_project("early", &early_root.to_string_lossy())
            .unwrap();
        assert!(manager.begin_crux_task(&early_root, false));
        approve(early.id, &early_root);
        assert!(crux_task_approval(&manager, &storage, early.id, &early_root).1);
        assert!(manager.crux_approved(&early_root));
        assert!(!manager.finish_crux_task(&early_root));

        // The user approves after the approval check. The task does not
        // index, so it runs again and then sees the approval.
        let late_root = base.join("late");
        let late = storage
            .upsert_project("late", &late_root.to_string_lossy())
            .unwrap();
        assert!(manager.begin_crux_task(&late_root, false));
        assert!(!crux_task_approval(&manager, &storage, late.id, &late_root).1);
        assert!(!manager.crux_approved(&late_root));
        approve(late.id, &late_root);
        assert!(manager.finish_crux_task(&late_root));
        assert!(manager.begin_crux_task(&late_root, true));
        assert!(crux_task_approval(&manager, &storage, late.id, &late_root).1);
        assert!(manager.crux_approved(&late_root));
        assert!(!manager.finish_crux_task(&late_root));
        let _ = fs::remove_dir_all(base);
    }

    #[test]
    fn crux_index_status_asks_for_approval_before_an_index() {
        let (base, storage) = crux_approval_test_storage("status");
        let root = base.join("project");
        let project = storage
            .upsert_project("project", &root.to_string_lossy())
            .unwrap();
        let manager = ExtensionManager::default();
        let census = test_census(200, 1);
        manager.set_crux_test_state(&root, CruxActivation::Auto, true, census.clone());
        let mut config = manager.read(Clone::clone);
        let status = |config: &ExtensionConfig| {
            crux_index_status(
                &manager,
                &storage,
                config,
                Some(project.id),
                Some(&root),
                Some(&census),
            )
            .unwrap()
            .state
        };

        assert_eq!(status(&config), CruxIndexStatusState::ApprovalRequired);
        config.crux_approved_projects.insert(project.id);
        assert_eq!(status(&config), CruxIndexStatusState::WaitingForIndex);

        let config = manager.read(Clone::clone);
        fs::create_dir_all(root.join(".scip-nav")).unwrap();
        fs::write(root.join(CRUX_INDEX_RELATIVE_PATH), b"index").unwrap();
        assert_eq!(status(&config), CruxIndexStatusState::ApprovalRequired);
        record_crux_index(&storage, &root);
        assert_eq!(status(&config), CruxIndexStatusState::Indexed);
        let _ = fs::remove_dir_all(base);
    }

    #[test]
    fn crux_census_groups_languages_and_honors_root_gitignore() {
        let root =
            std::env::temp_dir().join(format!("halv-crux-census-test-{}", rand::random::<u64>()));
        fs::create_dir_all(root.join("src")).unwrap();
        fs::create_dir_all(root.join("ignored")).unwrap();
        fs::write(root.join("src/app.ts"), "one\ntwo\n").unwrap();
        fs::write(root.join("src/main.rs"), "one\n").unwrap();
        fs::write(root.join("ignored/skip.ts"), "one\n").unwrap();
        fs::write(root.join(".gitignore"), "ignored/\n").unwrap();

        let census = build_crux_project_census(&root);
        assert_eq!(
            census.languages.get("typescript_javascript"),
            Some(&CruxLanguageCensus { files: 1, lines: 2 })
        );
        assert_eq!(
            census.languages.get("rust"),
            Some(&CruxLanguageCensus { files: 1, lines: 1 })
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn crux_builtin_injection_requires_enablement_gate_and_binary() {
        let root =
            std::env::temp_dir().join(format!("halv-crux-builtin-test-{}", rand::random::<u64>()));
        fs::create_dir_all(&root).unwrap();
        let binary = root.join(crux_binary_name());
        fs::write(&binary, b"crux").unwrap();
        let context = CruxEffectiveContext {
            binary: binary.clone(),
            approved: true,
            root: root.clone(),
            census: Some(test_census(200, 1)),
        };
        let mut config = ExtensionConfig {
            mcp_servers: vec![test_server(McpTransport::Stdio)],
            ..ExtensionConfig::default()
        };

        let servers = effective_mcp_servers(&config, Some(&context)).unwrap();
        assert_eq!(servers[0].id, BUILTIN_CRUX_ID);
        assert_eq!(servers[0].source, McpServerSource::BuiltIn);
        assert_eq!(servers[0].command.as_deref(), binary.to_str());
        assert_eq!(servers[0].cwd.as_deref(), root.to_str());
        assert_eq!(servers[0].args, ["--profile", "slim"]);
        assert_eq!(
            servers[0].enabled_tools,
            [
                "scip_index",
                "scip_find",
                "scip_map",
                "scip_outline",
                "scip_expand",
                "scip_architecture",
            ]
        );
        assert!(servers[0]
            .env
            .get("PATH")
            .is_some_and(|path| !path.is_empty()));
        assert_eq!(servers[1].id, "server-1");

        config
            .builtin_mcp_enabled
            .insert(BUILTIN_CRUX_ID.to_string(), false);
        assert!(effective_mcp_servers(&config, Some(&context))
            .unwrap()
            .iter()
            .all(|server| server.id != BUILTIN_CRUX_ID));
        config
            .builtin_mcp_enabled
            .insert(BUILTIN_CRUX_ID.to_string(), true);
        let below_gate = CruxEffectiveContext {
            census: Some(test_census(199, 74_999)),
            ..context.clone()
        };
        assert!(effective_mcp_servers(&config, Some(&below_gate))
            .unwrap()
            .iter()
            .all(|server| server.id != BUILTIN_CRUX_ID));
        let unapproved = CruxEffectiveContext {
            approved: false,
            ..context.clone()
        };
        assert!(effective_mcp_servers(&config, Some(&unapproved))
            .unwrap()
            .iter()
            .all(|server| server.id != BUILTIN_CRUX_ID));
        let missing = CruxEffectiveContext {
            binary: root.join("missing-crux"),
            ..context
        };
        assert!(effective_mcp_servers(&config, Some(&missing))
            .unwrap()
            .iter()
            .all(|server| server.id != BUILTIN_CRUX_ID));
        assert_eq!(
            ensure_mcp_server_removable(BUILTIN_CRUX_ID),
            Err("built-in MCP servers cannot be removed".to_string())
        );
        assert_eq!(
            ensure_mcp_server_editable(BUILTIN_CRUX_ID),
            Err("built-in MCP servers cannot be edited".to_string())
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn crux_settings_persist_in_extension_config() {
        let root =
            std::env::temp_dir().join(format!("halv-crux-settings-test-{}", rand::random::<u64>()));
        fs::create_dir_all(&root).unwrap();
        let mut config = ExtensionConfig {
            crux_activation: CruxActivation::AlwaysOn,
            ..ExtensionConfig::default()
        };
        config
            .builtin_mcp_enabled
            .insert(BUILTIN_CRUX_ID.to_string(), false);
        config.crux_approved_projects.insert(7);
        save_config(&root, &config).unwrap();
        let loaded = load_config(&root).unwrap();
        assert_eq!(loaded.crux_activation, CruxActivation::AlwaysOn);
        assert!(!builtin_mcp_enabled(&loaded, BUILTIN_CRUX_ID));
        assert_eq!(loaded.crux_approved_projects, BTreeSet::from([7]));
        let _ = fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[test]
    fn extension_config_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let root = std::env::temp_dir().join(format!(
            "halv-extension-modes-test-{}",
            rand::random::<u64>()
        ));
        fs::create_dir_all(&root).unwrap();
        let path = root.join(CONFIG_FILE);
        let mode = || fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        save_config(&root, &ExtensionConfig::default()).unwrap();
        assert_eq!(mode(), 0o600);

        // An earlier release left the file readable by others.
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        ExtensionManager::default()
            .set_app_data_dir(root.clone())
            .unwrap();
        assert_eq!(mode(), 0o600);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn crux_binary_resolution_uses_override_executable_and_staged_paths() {
        let root =
            std::env::temp_dir().join(format!("halv-crux-binary-test-{}", rand::random::<u64>()));
        fs::create_dir_all(&root).unwrap();
        let binary = root.join(crux_binary_name());
        fs::write(&binary, b"crux").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&binary, fs::Permissions::from_mode(0o755)).unwrap();
        }
        let expected = fs::canonicalize(&binary).unwrap();
        assert_eq!(
            locate_crux_binary_with_env(Some(binary.clone().into_os_string()), None, None),
            Some(expected.clone())
        );
        assert_eq!(
            locate_crux_binary_with_env(None, Some(root.join("Halv")), None),
            Some(expected.clone())
        );
        assert_eq!(
            locate_crux_binary_with_env(None, None, Some(binary)),
            Some(expected)
        );
        assert_eq!(locate_crux_binary_with_env(None, None, None), None);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn crux_tools_are_classified_as_read_only() {
        let server = builtin_crux_server(Path::new("/tmp/crux"), Path::new("/tmp/project"), true);
        for name in CRUX_TOOLS {
            let tool = parse_mcp_tool(
                &server,
                &json!({ "name": name, "inputSchema": { "type": "object" } }),
                None,
            )
            .unwrap()
            .expect("Crux tool should parse");
            assert!(tool.metadata.annotations.read_only_hint);
            assert!(!tool.metadata.annotations.destructive_hint);
            assert!(!tool.metadata.classified_as_write);
            assert_eq!(tool.descriptor.approval_mode, "approve");
        }
    }

    #[test]
    fn crux_qualifying_languages_returns_each_family_in_size_order() {
        let census = test_language_census(&[
            ("rust", 200, 80_000),
            ("typescript_javascript", 250, 100_000),
        ]);

        assert_eq!(crux_qualifying_languages(&census), ["typescript", "rust"]);
    }

    #[test]
    fn crux_qualifying_languages_filters_below_threshold_families() {
        let census = test_language_census(&[("python", 200, 1), ("rust", 199, 74_999)]);

        assert_eq!(crux_qualifying_languages(&census), ["python"]);
    }

    #[test]
    fn crux_qualifying_languages_returns_empty_when_no_family_qualifies() {
        let census = test_language_census(&[("rust", 199, 74_999)]);

        assert!(crux_qualifying_languages(&census).is_empty());
    }

    #[test]
    fn crux_qualifying_languages_skips_unknown_families() {
        let census = test_language_census(&[("unknown", 1_000, 100_000), ("rust", 200, 1)]);

        assert_eq!(crux_qualifying_languages(&census), ["rust"]);
    }

    #[test]
    fn crux_qualifying_languages_maps_family_names() {
        let census = test_language_census(&[
            ("typescript_javascript", 200, 1),
            ("java_kotlin", 201, 1),
            ("c_cpp", 202, 1),
        ]);

        assert_eq!(
            crux_qualifying_languages(&census),
            ["cpp", "java", "typescript"]
        );
    }

    #[test]
    fn crux_qualifying_languages_applies_exact_threshold_boundaries() {
        let census = test_language_census(&[
            ("rust", 1, 75_000),
            ("python", 1, 74_999),
            ("dart", 200, 1),
            ("java_kotlin", 199, 1),
        ]);

        assert_eq!(crux_qualifying_languages(&census), ["rust", "dart"]);
    }

    #[test]
    fn crux_qualifying_languages_preserves_tie_order() {
        let census = test_language_census(&[("rust", 200, 75_000), ("python", 200, 75_000)]);

        assert_eq!(crux_qualifying_languages(&census), ["python", "rust"]);
    }

    #[test]
    fn crux_index_arguments_use_languages_without_language() {
        let arguments = crux_index_arguments("/tmp/project", &["typescript", "rust"]);

        assert_eq!(
            arguments,
            json!({
                "project_root": "/tmp/project",
                "languages": ["typescript", "rust"],
            })
        );
        assert!(arguments.get("language").is_none());
    }

    #[test]
    fn crux_index_arguments_omit_language_keys_for_an_empty_list() {
        let arguments = crux_index_arguments("/tmp/project", &[]);

        assert_eq!(arguments, json!({ "project_root": "/tmp/project" }));
        assert!(arguments.get("language").is_none());
        assert!(arguments.get("languages").is_none());
    }

    #[test]
    fn crux_path_appends_existing_directories_once() {
        let root =
            std::env::temp_dir().join(format!("halv-crux-path-test-{}", rand::random::<u64>()));
        let original = root.join("original");
        let appended = root.join("appended");
        fs::create_dir_all(&original).unwrap();
        fs::create_dir_all(&appended).unwrap();
        let current = std::env::join_paths([&original]).unwrap();

        let augmented = augment_crux_path(
            Some(&current),
            [original.clone(), appended.clone(), root.join("missing")],
        );
        assert_eq!(
            std::env::split_paths(&augmented).collect::<Vec<_>>(),
            [original, appended]
        );
        let _ = fs::remove_dir_all(root);
    }

    // Agent CLIs outside the sandbox run the crux launcher first on PATH. A
    // sandboxed command can write to the RTK state directory.
    #[test]
    fn crux_launcher_stays_out_of_the_rtk_state_directory() {
        let Some(binary) = locate_crux_binary() else {
            eprintln!("the crux binary is missing; launcher assertions skipped");
            return;
        };

        let path = crux_path_environment_from(None);

        let crux_dir = std::env::split_paths(&path)
            .next()
            .expect("PATH should start with the crux directory");
        assert_eq!(
            fs::canonicalize(crux_dir.join(crux_binary_name())).ok(),
            fs::canonicalize(&binary).ok()
        );
        assert!(!crux_dir.starts_with(crate::exec::rtk_state_directory()));
        if binary.file_name() != Some(OsStr::new(crux_binary_name())) {
            let launcher_root = crate::exec::launcher_root().expect("tests have a launcher root");
            assert_eq!(crux_dir.parent(), Some(launcher_root.as_path()));
        }
    }

    #[test]
    fn crux_guidance_requires_a_connected_tool_and_index_file() {
        let root =
            std::env::temp_dir().join(format!("halv-crux-guidance-test-{}", rand::random::<u64>()));
        fs::create_dir_all(root.join(".scip-nav")).unwrap();
        let tools = vec![McpToolDescriptor {
            server_id: BUILTIN_CRUX_ID.to_string(),
            server_name: BUILTIN_CRUX_NAME.to_string(),
            name: "scip_map".to_string(),
            wire_name: "mcp__builtin-crux__scip_map__123".to_string(),
            description: "Map symbols".to_string(),
            input_schema: json!({ "type": "object" }),
            approval_mode: "approve".to_string(),
        }];
        assert!(!crux_index_guidance_available(&tools, Some(&root)));
        fs::write(root.join(CRUX_INDEX_RELATIVE_PATH), b"index").unwrap();
        assert!(crux_index_guidance_available(&tools, Some(&root)));
        assert!(!crux_index_guidance_available(&[], Some(&root)));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn metadata_and_explicit_skill_selection_are_stable() {
        let (name, description) = skill_metadata(
            "---\nname: review-helper\ndescription: Reviews safely.\n---\n# Body",
            Path::new("/tmp/review-helper"),
        );
        assert_eq!(name, "review-helper");
        assert_eq!(description, "Reviews safely.");
        assert!(mentions_skill("Use $review-helper", &name));
        assert!(!mentions_skill("Review normally", &name));
    }

    #[test]
    fn mcp_rejects_embedded_credentials_and_secret_headers() {
        let mut server = test_server(McpTransport::StreamableHttp);
        server.id.clear();
        server.name = "remote".to_string();
        server.url = Some("https://user:secret@example.com/mcp".to_string());
        assert!(validate_mcp(&mut server).is_err());
        server.url = Some("https://example.com/mcp".to_string());
        server
            .headers
            .insert("Authorization".to_string(), "Bearer x".to_string());
        assert!(validate_mcp(&mut server).is_err());
        server.headers.clear();
        server.env.insert("TOKEN".to_string(), "secret".to_string());
        assert!(validate_mcp(&mut server).is_err());
    }

    #[test]
    fn mcp_oauth_pkce_matches_rfc_vector() {
        assert_eq!(
            mcp_pkce_challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn mcp_oauth_discovers_path_and_root_metadata_in_spec_order() {
        let protected =
            protected_resource_metadata_urls("https://example.com/public/mcp?tenant=1").unwrap();
        assert_eq!(
            protected.iter().map(Url::as_str).collect::<Vec<_>>(),
            vec![
                "https://example.com/.well-known/oauth-protected-resource/public/mcp",
                "https://example.com/.well-known/oauth-protected-resource",
            ]
        );

        let issuer = Url::parse("https://auth.example.com/tenant1").unwrap();
        let authorization = authorization_server_metadata_urls(&issuer);
        assert_eq!(
            authorization.iter().map(Url::as_str).collect::<Vec<_>>(),
            vec![
                "https://auth.example.com/.well-known/oauth-authorization-server/tenant1",
                "https://auth.example.com/.well-known/openid-configuration/tenant1",
                "https://auth.example.com/tenant1/.well-known/openid-configuration",
            ]
        );
    }

    #[test]
    fn mcp_oauth_parses_resource_and_scope_challenge() {
        let header = concat!(
            "Bearer realm=\"mcp\", ",
            "resource_metadata=\"https://mcp.example.com/.well-known/oauth-protected-resource\", ",
            "scope=\"files:read files:write\""
        );
        assert_eq!(
            mcp_bearer_challenge_parameter(header, "resource_metadata").as_deref(),
            Some("https://mcp.example.com/.well-known/oauth-protected-resource")
        );
        assert_eq!(
            split_mcp_oauth_scopes(&mcp_bearer_challenge_parameter(header, "scope").unwrap()),
            vec!["files:read", "files:write"]
        );
    }

    #[test]
    fn mcp_oauth_authorization_url_includes_pkce_state_and_resource() {
        let flow = McpOAuthFlow {
            verifier: "verifier".to_string(),
            challenge: "challenge".to_string(),
            state: "state".to_string(),
        };
        let built = build_mcp_oauth_authorize_url(
            "https://auth.example.com/authorize?prompt=consent",
            "client-id",
            "http://127.0.0.1:1234/oauth/callback",
            &flow,
            "https://mcp.example.com/mcp",
            &["files:read".to_string()],
        )
        .unwrap();
        let parsed = Url::parse(&built).unwrap();
        let params = parsed.query_pairs().into_owned().collect::<HashMap<_, _>>();
        assert_eq!(params.get("prompt").map(String::as_str), Some("consent"));
        assert_eq!(
            params.get("code_challenge").map(String::as_str),
            Some("challenge")
        );
        assert_eq!(
            params.get("code_challenge_method").map(String::as_str),
            Some("S256")
        );
        assert_eq!(params.get("state").map(String::as_str), Some("state"));
        assert_eq!(
            params.get("resource").map(String::as_str),
            Some("https://mcp.example.com/mcp")
        );
        assert_eq!(params.get("scope").map(String::as_str), Some("files:read"));
    }

    #[test]
    fn mcp_oauth_callback_rejects_state_mismatch() {
        let request = "GET /oauth/callback?code=secret&state=wrong HTTP/1.1\r\n\r\n";
        let result = parse_mcp_oauth_callback_request(request, "expected").unwrap();
        assert!(result.unwrap_err().contains("state did not match"));
        let request = "GET /oauth/callback?code=secret&state=expected HTTP/1.1\r\n\r\n";
        assert_eq!(
            parse_mcp_oauth_callback_request(request, "expected")
                .unwrap()
                .unwrap(),
            "secret"
        );
    }

    #[test]
    fn mcp_oauth_legacy_keychain_values_remain_readable() {
        let credentials = decode_mcp_oauth_credentials("legacy-access-token").unwrap();
        assert_eq!(credentials.access_token, "legacy-access-token");
        assert!(credentials.refresh_token.is_none());
        assert!(credentials.client_id.is_empty());
    }

    #[test]
    fn mcp_tools_are_filtered_and_receive_collision_resistant_wire_names() {
        let mut server = test_server(McpTransport::Stdio);
        server.enabled_tools = vec!["lookup".to_string()];
        server.approval_mode = "writes".to_string();
        server.trust_tool_annotations = true;
        let tool = parse_mcp_tool(
            &server,
            &json!({
                "name": "lookup",
                "description": "Lookup",
                "inputSchema": { "type": "object" },
                "annotations": { "readOnlyHint": true, "destructiveHint": false }
            }),
            Some("Use the canonical repository identifiers."),
        )
        .expect("tool should parse")
        .expect("tool should be enabled");
        assert!(tool
            .descriptor
            .wire_name
            .starts_with("mcp__server-1__lookup__"));
        assert!(tool
            .descriptor
            .description
            .contains("canonical repository identifiers"));
        assert_eq!(tool.descriptor.approval_mode, "approve");
        assert!(!tool.metadata.classified_as_write);
        assert!(tool.metadata.annotations.read_only_hint);

        server.trust_tool_annotations = false;
        let untrusted = parse_mcp_tool(
            &server,
            &json!({ "name": "lookup", "annotations": { "readOnlyHint": true } }),
            None,
        )
        .unwrap()
        .unwrap();
        assert_eq!(untrusted.descriptor.approval_mode, "prompt");
        assert!(untrusted.metadata.classified_as_write);
        assert!(parse_mcp_tool(&server, &json!({ "name": "other" }), None)
            .expect("filtered tool should parse safely")
            .is_none());
    }

    #[test]
    fn mcp_tool_results_preserve_text_images_and_error_state() {
        let result = normalize_mcp_tool_result(&json!({
            "content": [
                { "type": "text", "text": "Visible app state" },
                {
                    "type": "image",
                    "mimeType": "image/png",
                    "data": "iVBORw0KGgo="
                }
            ],
            "structuredContent": { "window": "Halv" },
            "isError": true
        }))
        .unwrap();
        assert!(result.text.contains("Visible app state"));
        assert!(result.text.contains("\"window\": \"Halv\""));
        assert_eq!(result.images.len(), 1);
        assert_eq!(result.images[0].media_type, "image/png");
        assert!(result.is_error);
    }

    #[test]
    fn mcp_elicitation_validates_form_and_url_modes() {
        let form = parse_mcp_elicitation_request(Some(&json!({
            "message": "Choose a workspace",
            "requestedSchema": {
                "type": "object",
                "properties": {
                    "workspace": { "type": "string" },
                    "remember": { "type": "boolean" }
                },
                "required": ["workspace"]
            }
        })))
        .unwrap();
        assert_eq!(form.mode, "form");
        assert!(validate_mcp_elicitation_response(
            &form,
            McpElicitationResponse {
                action: "accept".to_string(),
                content: Some(json!({ "workspace": "repo", "remember": true })),
            }
        )
        .is_ok());
        assert!(validate_mcp_elicitation_response(
            &form,
            McpElicitationResponse {
                action: "accept".to_string(),
                content: Some(json!({ "remember": true })),
            }
        )
        .is_err());

        let url = parse_mcp_elicitation_request(Some(&json!({
            "mode": "url",
            "message": "Connect account",
            "elicitationId": "one",
            "url": "https://example.com/connect"
        })))
        .unwrap();
        assert_eq!(url.url.as_deref(), Some("https://example.com/connect"));
        assert!(parse_mcp_elicitation_request(Some(&json!({
            "mode": "url",
            "message": "Unsafe",
            "elicitationId": "two",
            "url": "javascript:alert(1)"
        })))
        .is_err());
    }

    #[test]
    fn plugin_mcp_file_is_loaded_with_stable_provenance() {
        let root =
            std::env::temp_dir().join(format!("halv-plugin-mcp-test-{}", rand::random::<u64>()));
        fs::create_dir_all(root.join(".codex-plugin")).unwrap();
        fs::write(
            root.join(".codex-plugin/plugin.json"),
            r#"{
  "name": "example-plugin",
  "version": "1.2.3",
  "description": "Example plugin",
  "mcpServers": "./.mcp.json",
  "interface": { "displayName": "Example", "developerName": "Halv", "capabilities": ["Read"] }
}"#,
        )
        .unwrap();
        fs::write(
            root.join(".mcp.json"),
            r#"{
  "mcpServers": {
    "catalog": {
      "command": "node",
      "args": ["./server.mjs"],
      "cwd": ".",
      "env": { "LOG_LEVEL": "warn" },
      "env_vars": ["CATALOG_TOKEN"],
      "default_tools_approval_mode": "writes"
    }
  }
}"#,
        )
        .unwrap();

        let validated = validate_plugin(&root).unwrap();
        assert_eq!(validated.display_name, "Example");
        assert_eq!(validated.developer_name.as_deref(), Some("Halv"));
        let servers =
            parse_plugin_mcp_servers(&root, &validated.id, &validated.display_name, true).unwrap();
        assert_eq!(servers.len(), 1);
        assert_eq!(servers[0].source, McpServerSource::Plugin);
        assert_eq!(servers[0].plugin_id.as_deref(), Some("example-plugin"));
        assert_eq!(servers[0].approval_mode, "writes");
        assert_eq!(
            servers[0].env.get("LOG_LEVEL").map(String::as_str),
            Some("warn")
        );
        assert_eq!(servers[0].env_vars, vec!["CATALOG_TOKEN"]);
        assert!(servers[0].id.starts_with("plugin-example-plugin-catalog-"));
        let hydrated = plugin_snapshot_descriptor(&PluginDescriptor {
            id: validated.id,
            name: "example-plugin".to_string(),
            version: "0.0.0".to_string(),
            root: root.to_string_lossy().into_owned(),
            enabled: true,
            installed_at: "now".to_string(),
            contributes_skills: false,
            contributes_mcp: false,
            unsupported_components: vec![],
            description: String::new(),
            display_name: String::new(),
            developer_name: None,
            capabilities: vec![],
            skill_count: 0,
            mcp_server_count: 0,
        })
        .unwrap();
        assert_eq!(hydrated.display_name, "Example");
        assert_eq!(hydrated.description, "Example plugin");
        assert_eq!(hydrated.mcp_server_count, 1);
        assert!(hydrated.contributes_mcp);
        let effective = effective_mcp_servers(
            &ExtensionConfig {
                plugins: vec![hydrated.clone()],
                ..ExtensionConfig::default()
            },
            None,
        )
        .unwrap();
        assert_eq!(effective.len(), 1);
        assert_eq!(effective[0].source, McpServerSource::Plugin);
        let mut disabled = hydrated;
        disabled.enabled = false;
        let effective = effective_mcp_servers(
            &ExtensionConfig {
                plugins: vec![disabled],
                ..ExtensionConfig::default()
            },
            None,
        )
        .unwrap();
        assert!(!effective[0].enabled);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn plugin_skill_metadata_and_enablement_id_survive_reinstall_paths() {
        fn create_skill_root(parent: &Path) -> SkillRoot {
            let skill = parent.join("skills/reviewer");
            fs::create_dir_all(skill.join("agents")).unwrap();
            fs::write(
                skill.join("SKILL.md"),
                "---\nname: reviewer\ndescription: Review carefully.\n---\n# Reviewer\n",
            )
            .unwrap();
            fs::write(
                skill.join("agents/openai.yaml"),
                concat!(
                    "interface:\n",
                    "  display_name: Careful Reviewer\n",
                    "  short_description: Reviews with repository context\n",
                    "  default_prompt: Review this change\n",
                    "policy:\n",
                    "  allow_implicit_invocation: false\n",
                    "dependencies:\n",
                    "  tools:\n",
                    "    - type: mcp\n",
                    "      value: github.search_code\n"
                ),
            )
            .unwrap();
            SkillRoot {
                root: parent.join("skills"),
                scope: "plugin".to_string(),
                source: "plugin:Example".to_string(),
                plugin_id: Some("example-plugin".to_string()),
                plugin_relative_root: Some("skills".to_string()),
            }
        }

        let root =
            std::env::temp_dir().join(format!("halv-plugin-skill-test-{}", rand::random::<u64>()));
        let first = create_skill_root(&root.join("install-a"));
        let second = create_skill_root(&root.join("install-b"));
        let mut first_skills = Vec::new();
        let mut second_skills = Vec::new();
        scan_skill_root(&first, &HashMap::new(), &mut first_skills).unwrap();
        scan_skill_root(&second, &HashMap::new(), &mut second_skills).unwrap();
        assert_eq!(first_skills.len(), 1);
        assert_eq!(first_skills[0].id, second_skills[0].id);
        assert_eq!(first_skills[0].display_name, "Careful Reviewer");
        assert_eq!(
            first_skills[0].short_description,
            "Reviews with repository context"
        );
        assert!(!first_skills[0].allow_implicit_invocation);
        assert_eq!(
            first_skills[0].default_prompt.as_deref(),
            Some("Review this change")
        );
        assert_eq!(first_skills[0].dependency_tools, vec!["github.search_code"]);
        assert_eq!(first_skills[0].plugin_id.as_deref(), Some("example-plugin"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn streamable_http_parser_accepts_json_and_sse_results() {
        let json_response = r#"{"jsonrpc":"2.0","id":2,"result":{"tools":[]}}"#;
        assert_eq!(parse_http_rpc_response(json_response, 2).unwrap()["id"], 2);
        let sse_response = concat!(
            "event: message\n",
            "data: {\"jsonrpc\":\"2.0\",\"id\":3,\"result\":{\"tools\":[]}}\n\n"
        );
        assert_eq!(parse_http_rpc_response(sse_response, 3).unwrap()["id"], 3);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn stdio_mcp_lifecycle_discovers_tools() {
        use std::os::unix::fs::PermissionsExt;

        let root = std::env::temp_dir().join(format!("halv-mcp-test-{}", rand::random::<u64>()));
        fs::create_dir_all(&root).expect("temp root should create");
        let script = root.join("server.sh");
        fs::write(
            &script,
            r#"#!/bin/sh
while IFS= read -r line; do
  case "$line" in
    *'"method":"initialize"'*) printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2025-11-25","capabilities":{"tools":{"listChanged":true}},"instructions":"Use stable repository IDs.","serverInfo":{"name":"test","version":"1"}}}' ;;
    *'"method":"tools/list"'*) printf '%s\n' '{"jsonrpc":"2.0","id":2,"result":{"tools":[{"name":"lookup","description":"Lookup","inputSchema":{"type":"object"},"annotations":{"readOnlyHint":true,"destructiveHint":false}}]}}' ;;
  esac
done
"#,
        )
        .expect("script should write");
        let mut permissions = fs::metadata(&script).unwrap().permissions();
        permissions.set_mode(0o700);
        fs::set_permissions(&script, permissions).unwrap();

        let mut server = test_server(McpTransport::Stdio);
        server.id = "stdio-test".to_string();
        server.name = "STDIO test".to_string();
        server.command = Some(script.to_string_lossy().into_owned());
        server.required = true;
        server.startup_timeout_ms = 5_000;
        server.tool_timeout_ms = 5_000;
        server.approval_mode = "writes".to_string();
        server.trust_tool_annotations = true;
        let result = connect_and_list_tools(&server)
            .await
            .expect("stdio MCP should connect");
        assert_eq!(result.protocol_version, "2025-11-25");
        assert_eq!(
            result.server_instructions.as_deref(),
            Some("Use stable repository IDs.")
        );
        assert_eq!(result.server_capabilities["tools"]["listChanged"], true);
        assert_eq!(result.server_info.as_ref().unwrap()["name"], "test");
        assert_eq!(result.tools.len(), 1);
        assert_eq!(result.tools[0].name, "lookup");
        assert_eq!(result.tools[0].approval_mode, "approve");
        assert!(result.tools[0]
            .description
            .contains("Use stable repository IDs."));
        assert_eq!(result.tool_metadata.len(), 1);
        assert!(!result.tool_metadata[0].classified_as_write);
        let _ = fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn stdio_mcp_tool_call_handles_nested_elicitation() {
        use std::os::unix::fs::PermissionsExt;

        let root =
            std::env::temp_dir().join(format!("halv-mcp-elicit-test-{}", rand::random::<u64>()));
        fs::create_dir_all(&root).expect("temp root should create");
        let script = root.join("server.sh");
        fs::write(
            &script,
            r#"#!/bin/sh
while IFS= read -r line; do
  case "$line" in
    *'"method":"initialize"'*) printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2025-11-25","capabilities":{"tools":{}}}}' ;;
    *'"method":"tools/call"'*) printf '%s\n' '{"jsonrpc":"2.0","id":"permission","method":"elicitation/create","params":{"message":"Allow access?","requestedSchema":{"type":"object","properties":{}}}}' ;;
    *'"id":"permission"'*) printf '%s\n' '{"jsonrpc":"2.0","id":2,"result":{"content":[{"type":"text","text":"ready"},{"type":"image","mimeType":"image/png","data":"iVBORw0KGgo="}]}}' ;;
  esac
done
"#,
        )
        .expect("script should write");
        let mut permissions = fs::metadata(&script).unwrap().permissions();
        permissions.set_mode(0o700);
        fs::set_permissions(&script, permissions).unwrap();

        let mut server = test_server(McpTransport::Stdio);
        server.command = Some(script.to_string_lossy().into_owned());
        server.startup_timeout_ms = 5_000;
        server.tool_timeout_ms = 5_000;
        let (sender, mut receiver) = mpsc::channel(1);
        let task = tokio::spawn(async move {
            connect_and_call_tool(&server, "get_app_state", json!({}), Some(sender)).await
        });
        let request = receiver.recv().await.expect("elicitation should arrive");
        assert_eq!(request.message, "Allow access?");
        request
            .responder
            .send(McpElicitationResponse {
                action: "accept".to_string(),
                content: Some(json!({})),
            })
            .unwrap();
        let raw = task.await.unwrap().unwrap();
        let result = normalize_mcp_tool_result(&raw).unwrap();
        assert_eq!(result.text, "ready");
        assert_eq!(result.images.len(), 1);
        let _ = fs::remove_dir_all(root);
    }
}
