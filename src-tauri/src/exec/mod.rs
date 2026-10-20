pub mod apply_patch;
mod bwrap;
#[cfg(target_os = "macos")]
mod process_tree;

use crate::host_env;
pub use apply_patch::{ApplyPatchSummary, FileSnapshot, ParsedPatch, PatchFileChange};
#[cfg(target_os = "macos")]
use process_tree::SandboxTag;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
#[cfg(not(test))]
use std::io::Write;
use std::{
    collections::{hash_map::DefaultHasher, BTreeSet, HashSet},
    ffi::OsString,
    fs::{self, File},
    hash::{Hash, Hasher},
    io::{self, Read},
    path::{Component, Path, PathBuf},
    process::{Child, Command, ExitStatus, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex, OnceLock,
    },
    thread,
    time::{Duration, Instant},
};

pub const DEFAULT_TIMEOUT_MS: u64 = 10_000;
pub const MAX_TIMEOUT_MS: u64 = 10 * 60 * 1000;
pub const CAPTURE_LIMIT_BYTES: usize = 1024 * 1024;
pub const MODEL_OUTPUT_LIMIT_BYTES: usize = 10_000;

const SANDBOX_EXEC_PATH: &str = "/usr/bin/sandbox-exec";
const BASH_PATH: &str = "/bin/bash";
const GIT_PATH: &str = "git";
const GH_PATH: &str = "gh";
const HALV_RTK_BINARY_ENV: &str = "HALV_RTK_BINARY";
const HEADROOM_WORKSPACE_DIR_ENV: &str = "HEADROOM_WORKSPACE_DIR";
pub(crate) const RTK_DB_PATH_ENV: &str = "RTK_DB_PATH";
const RTK_TELEMETRY_DISABLED_ENV: &str = "RTK_TELEMETRY_DISABLED";
const RTK_REWRITE_TIMEOUT: Duration = Duration::from_secs(10);
const RTK_REWRITE_OUTPUT_LIMIT_BYTES: usize = 64 * 1024;
const AUTO_BRANCH_PREFIX: &str = "halv/";
const READER_DRAIN_GRACE: Duration = Duration::from_millis(300);
const CHILD_KILL_GRACE: Duration = Duration::from_millis(500);
const PROTECTED_ROOT_NAMES: &[&str] = &[".git", ".codex", ".halv", ".agents"];
const RTK_STATE_DIRECTORY_PREFIX: &str = "halv-rtk-";
static RTK_STATE_DIRECTORY: OnceLock<PathBuf> = OnceLock::new();
const LAUNCHER_DIRECTORY_NAME: &str = "launchers";
static LAUNCHER_ROOT: OnceLock<PathBuf> = OnceLock::new();

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SandboxPermissions {
    #[default]
    UseDefault,
    WithAdditionalPermissions,
    RequireEscalated,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalPolicy {
    ReadOnly,
    Ask,
    OnRequest,
    #[serde(alias = "on-failure")]
    OnFailure,
    Never,
    SkipAll,
}

impl ApprovalPolicy {
    pub fn from_storage(value: Option<&str>, trust_level: Option<&str>) -> Self {
        match value {
            Some("read_only") => Self::ReadOnly,
            Some("ask") => Self::Ask,
            Some("on_request") => Self::OnRequest,
            Some("on-failure" | "on_failure") => Self::OnFailure,
            Some("never") => Self::Never,
            Some("skip_all") => Self::SkipAll,
            _ => match trust_level {
                Some("trusted") => Self::OnRequest,
                Some("untrusted") => Self::Ask,
                _ => Self::Ask,
            },
        }
    }

    pub fn workspace_write_enabled(self) -> bool {
        matches!(
            self,
            Self::Ask | Self::OnRequest | Self::OnFailure | Self::Never | Self::SkipAll
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SandboxMode {
    ReadOnly,
    WorkspaceWrite,
    DangerFullAccess,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceRoots {
    roots: Vec<PathBuf>,
    danger_full_access: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedWritePath {
    pub absolute_path: PathBuf,
    pub real_path: PathBuf,
    pub is_writable: bool,
    pub is_protected: bool,
}

impl WorkspaceRoots {
    pub fn for_cwd(cwd: impl AsRef<Path>) -> Self {
        let mut roots = vec![normalize_path_lexically(cwd.as_ref())];
        let system_tmp = PathBuf::from("/tmp");
        roots.push(system_tmp.canonicalize().unwrap_or(system_tmp));
        if let Some(tmpdir) = std::env::var_os("TMPDIR").map(PathBuf::from) {
            roots.push(tmpdir.canonicalize().unwrap_or(tmpdir));
        }
        Self::new(roots)
    }

    pub fn new(roots: Vec<PathBuf>) -> Self {
        Self::new_with_access(roots, false)
    }

    fn new_with_access(roots: Vec<PathBuf>, danger_full_access: bool) -> Self {
        let mut deduped = BTreeSet::new();
        let mut stable_roots = Vec::new();
        for root in roots {
            let mut normalized = normalize_path_lexically(&root);
            if !normalized.is_absolute() {
                if let Ok(current_dir) = std::env::current_dir() {
                    normalized = normalize_path_lexically(&current_dir.join(normalized));
                }
            }
            if std::fs::symlink_metadata(&normalized)
                .is_ok_and(|metadata| metadata.file_type().is_symlink())
            {
                continue;
            }
            stable_roots.push(
                normalized
                    .canonicalize()
                    .map(|path| normalize_path_lexically(&path))
                    .unwrap_or(normalized),
            );
        }
        let roots = stable_roots
            .into_iter()
            .filter(|path| deduped.insert(path.clone()))
            .collect();
        Self {
            roots,
            danger_full_access,
        }
    }

    pub fn roots(&self) -> &[PathBuf] {
        &self.roots
    }

    pub fn with_danger_full_access(self) -> Self {
        Self {
            danger_full_access: true,
            ..self
        }
    }

    pub fn danger_full_access(&self) -> bool {
        self.danger_full_access
    }

    pub fn contains_writable_path(&self, path: &Path) -> bool {
        self.resolve_write_path(path)
            .is_ok_and(|resolved| resolved.is_writable)
    }

    pub fn is_protected_path(&self, path: &Path) -> bool {
        self.resolve_write_path(path)
            .map(|resolved| resolved.is_protected)
            .unwrap_or_else(|_| self.is_protected_path_lexically(path))
    }

    pub fn resolve_write_path(&self, path: &Path) -> Result<ResolvedWritePath, String> {
        let absolute_path = normalize_path_lexically(path);
        if !absolute_path.is_absolute() {
            return Err(format!(
                "`{}` is not an absolute path",
                absolute_path.display()
            ));
        }
        if self.danger_full_access {
            return Ok(ResolvedWritePath {
                absolute_path: absolute_path.clone(),
                real_path: absolute_path,
                is_writable: true,
                is_protected: false,
            });
        }

        let real_path = resolve_real_path_without_symlinks(&absolute_path)?;
        let real_roots = self.real_roots()?;
        let is_protected = path_is_protected(&absolute_path, &real_roots)
            || path_is_protected(&real_path, &real_roots);
        let is_writable = real_roots
            .iter()
            .any(|root| path_is_within(&real_path, root))
            && !is_protected;

        Ok(ResolvedWritePath {
            absolute_path,
            real_path,
            is_writable,
            is_protected,
        })
    }

    pub fn approved_patch_roots(
        &self,
        cwd: &Path,
        approved_paths: &[String],
    ) -> Result<Self, String> {
        let mut roots = self.roots.clone();
        for path in approved_paths {
            let absolute = resolve_workspace_path(cwd, path)?;
            let resolved = self.resolve_write_path(&absolute)?;
            if !self.danger_full_access && resolved.is_protected {
                return Err(format!("`{path}` is under a protected top-level directory"));
            }
            roots.push(resolved.real_path);
        }
        Ok(Self::new_with_access(roots, self.danger_full_access))
    }

    fn real_roots(&self) -> Result<Vec<PathBuf>, String> {
        self.validate_roots()?;
        Ok(self.roots.clone())
    }

    fn validate_roots(&self) -> Result<(), String> {
        for root in &self.roots {
            match std::fs::symlink_metadata(root) {
                Ok(metadata) => {
                    if metadata.file_type().is_symlink() {
                        return Err(format!(
                            "workspace root became a symbolic link: {}",
                            root.display()
                        ));
                    }
                    let canonical = root.canonicalize().map_err(|error| {
                        format!(
                            "failed to revalidate workspace root `{}`: {error}",
                            root.display()
                        )
                    })?;
                    if normalize_path_lexically(&canonical) != *root {
                        return Err(format!(
                            "workspace root identity changed: {}",
                            root.display()
                        ));
                    }
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    let resolved = resolve_real_path_without_symlinks(root)?;
                    if resolved != *root {
                        return Err(format!(
                            "workspace root resolves through a symbolic link: {}",
                            root.display()
                        ));
                    }
                }
                Err(error) => {
                    return Err(format!(
                        "failed to inspect workspace root `{}`: {error}",
                        root.display()
                    ));
                }
            }
        }
        Ok(())
    }

    fn is_protected_path_lexically(&self, path: &Path) -> bool {
        let path = normalize_path_lexically(path);
        self.roots.iter().any(|root| {
            PROTECTED_ROOT_NAMES
                .iter()
                .map(|name| normalize_path_lexically(&root.join(name)))
                .any(|protected| path_is_within(&path, &protected))
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PatchSafety {
    AutoApproved,
    RequiresApproval(Vec<String>),
    Denied(String),
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BranchStatus {
    pub is_git_repo: bool,
    pub branch: Option<String>,
    pub default_branch: Option<String>,
    pub clean: Option<bool>,
    pub auto_created_branch: Option<String>,
    pub warning: Option<String>,
}

#[derive(Debug, Clone)]
pub enum FirstWriteAction {
    Proceed(BranchStatus),
    RequiresConfirmation(BranchStatus),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case")]
enum PatchHelperRequest {
    Apply {
        cwd: String,
        patch: String,
        roots: Vec<String>,
    },
    Restore {
        cwd: String,
        snapshots: Vec<FileSnapshot>,
        roots: Vec<String>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ApplyPatchHelperResponse {
    ok: bool,
    summary: Option<ApplyPatchSummary>,
    error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ReviewDecision {
    Approved,
    ApprovedForSession,
    ApprovedPaths {
        paths: Vec<String>,
        remember: bool,
    },
    McpElicitation {
        action: String,
        content: Option<Value>,
    },
    Denied,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ApprovalKey {
    pub conversation_id: String,
    pub project_root: String,
    pub command: String,
    pub cwd: String,
    pub sandbox_permissions: SandboxPermissions,
    pub login: bool,
    pub timeout_ms: u64,
    pub additional_permissions: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PatchApprovalKey {
    conversation_id: String,
    project_root: PathBuf,
    real_path: PathBuf,
}

impl ApprovalKey {
    pub fn new(
        command: impl Into<String>,
        cwd: impl Into<String>,
        sandbox_permissions: SandboxPermissions,
        login: bool,
        timeout_ms: u64,
        additional_permissions: Vec<String>,
    ) -> Self {
        Self::new_scoped(
            "",
            "",
            command,
            cwd,
            sandbox_permissions,
            login,
            timeout_ms,
            additional_permissions,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn new_scoped(
        conversation_id: impl Into<String>,
        project_root: impl Into<String>,
        command: impl Into<String>,
        cwd: impl Into<String>,
        sandbox_permissions: SandboxPermissions,
        login: bool,
        timeout_ms: u64,
        mut additional_permissions: Vec<String>,
    ) -> Self {
        additional_permissions.sort();
        additional_permissions.dedup();
        Self {
            conversation_id: conversation_id.into(),
            project_root: project_root.into(),
            command: command.into(),
            cwd: cwd.into(),
            sandbox_permissions,
            login,
            timeout_ms,
            additional_permissions,
        }
    }
}

#[derive(Debug, Default)]
pub struct ApprovalCache {
    keys: Mutex<HashSet<ApprovalKey>>,
    patch_paths: Mutex<HashSet<PatchApprovalKey>>,
}

impl ApprovalCache {
    pub fn contains(&self, key: &ApprovalKey) -> bool {
        self.keys
            .lock()
            .expect("approval cache lock poisoned")
            .contains(key)
    }

    pub fn approve_for_session(&self, key: ApprovalKey) {
        self.keys
            .lock()
            .expect("approval cache lock poisoned")
            .insert(key);
    }

    pub fn contains_patch_path(&self, path: &PatchApprovalKey) -> bool {
        self.patch_paths
            .lock()
            .expect("patch approval cache lock poisoned")
            .contains(path)
    }

    pub fn approve_patch_paths<I>(&self, paths: I)
    where
        I: IntoIterator<Item = PatchApprovalKey>,
    {
        self.patch_paths
            .lock()
            .expect("patch approval cache lock poisoned")
            .extend(paths);
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct ShellCommandRequest {
    pub command: String,
    #[serde(default)]
    pub workdir: Option<String>,
    #[serde(default)]
    pub timeout_ms: Option<u64>,
    #[serde(default)]
    pub login: Option<bool>,
    #[serde(default)]
    pub sandbox_permissions: SandboxPermissions,
    #[serde(default)]
    pub justification: Option<String>,
    #[serde(default)]
    #[allow(dead_code)]
    pub prefix_rule: Option<Vec<String>>,
    #[serde(default)]
    pub additional_permissions: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentCapabilities {
    pub platform: String,
    pub sandbox_available: bool,
    pub message: String,
}

#[derive(Debug, Clone)]
pub struct ExecResult {
    pub exit_code: i32,
    pub timed_out: bool,
    pub wall_time: Duration,
    #[allow(dead_code)]
    pub captured_output: String,
    pub model_output: String,
    pub truncated_output: bool,
    pub rtk_route: Option<RtkRouteKind>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RtkRouteKind {
    Rewritten,
    Tracked,
}

impl RtkRouteKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Rewritten => "rewritten",
            Self::Tracked => "tracked",
        }
    }
}

#[derive(Debug)]
pub enum ExecError {
    Cancelled,
    Spawn(String),
    Join(String),
}

impl std::fmt::Display for ExecError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExecError::Cancelled => formatter.write_str("Chat request cancelled"),
            ExecError::Spawn(message) => write!(formatter, "{message}"),
            ExecError::Join(message) => write!(formatter, "{message}"),
        }
    }
}

impl std::error::Error for ExecError {}

type OutputHandler = dyn Fn(&[u8]) + Send + Sync + 'static;

#[derive(Clone)]
pub struct OutputCallback(Arc<OutputHandler>);

impl OutputCallback {
    pub fn new(callback: impl Fn(&[u8]) + Send + Sync + 'static) -> Self {
        Self(Arc::new(callback))
    }

    fn emit(&self, chunk: &[u8]) {
        (self.0)(chunk);
    }
}

#[derive(Debug)]
struct ExecPlan {
    command: String,
    cwd: PathBuf,
    timeout_ms: u64,
    login: bool,
    rtk_binary: Option<PathBuf>,
    rtk_database_path: PathBuf,
}

#[derive(Debug)]
struct ActiveRtk {
    binary: PathBuf,
    database_path: PathBuf,
    path: OsString,
}

#[derive(Debug)]
enum RtkRoute {
    /// RTK recognized the command and supplied a token-filtering replacement.
    Rewritten { command: String, runtime: ActiveRtk },
    /// RTK did not have a safe filter for this command. Execute it unchanged
    /// through `rtk proxy` so first-turn adoption and accounting still work.
    Tracked { runtime: ActiveRtk },
}

#[derive(Debug)]
struct RtkRewriteAttempt {
    route: Option<RtkRoute>,
    warning: Option<String>,
}

impl RtkRewriteAttempt {
    fn unavailable() -> Self {
        Self {
            route: None,
            warning: None,
        }
    }

    fn routed(route: RtkRoute) -> Self {
        Self {
            route: Some(route),
            warning: None,
        }
    }

    fn fallback(warning: String) -> Self {
        Self {
            route: None,
            warning: Some(warning),
        }
    }

    fn log_warning(self) -> Option<RtkRoute> {
        if let Some(warning) = self.warning {
            eprintln!("{warning}");
        }
        self.route
    }
}

#[derive(Debug, Clone, Copy)]
struct RtkRewriteProbeOptions {
    sandbox_mode: SandboxMode,
    timeout: Duration,
}

impl RtkRoute {
    fn runtime(&self) -> &ActiveRtk {
        match self {
            Self::Rewritten { runtime, .. } | Self::Tracked { runtime } => runtime,
        }
    }

    fn kind(&self) -> RtkRouteKind {
        match self {
            Self::Rewritten { .. } => RtkRouteKind::Rewritten,
            Self::Tracked { .. } => RtkRouteKind::Tracked,
        }
    }
}

#[derive(Debug)]
struct Capture {
    bytes: Vec<u8>,
    total_bytes: usize,
    truncated: bool,
    limit_bytes: usize,
}

impl Default for Capture {
    fn default() -> Self {
        Self::with_limit(CAPTURE_LIMIT_BYTES)
    }
}

impl Capture {
    fn with_limit(limit_bytes: usize) -> Self {
        Self {
            bytes: Vec::new(),
            total_bytes: 0,
            truncated: false,
            limit_bytes,
        }
    }

    fn push(&mut self, chunk: &[u8]) -> usize {
        self.total_bytes = self.total_bytes.saturating_add(chunk.len());
        if self.bytes.len() >= self.limit_bytes {
            self.truncated = true;
            return 0;
        }
        let remaining = self.limit_bytes - self.bytes.len();
        let keep = remaining.min(chunk.len());
        self.bytes.extend_from_slice(&chunk[..keep]);
        if keep < chunk.len() {
            self.truncated = true;
        }
        keep
    }
}

struct ReaderHandle {
    stop: Arc<AtomicBool>,
    handle: thread::JoinHandle<()>,
}

pub fn shell_command_tool_schema() -> Value {
    json!({
        "type": "function",
        "name": "shell_command",
        "description": "Runs a shell command and returns its output.",
        "strict": false,
        "parameters": {
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "The shell command to execute as a shell script."
                },
                "workdir": {
                    "type": "string",
                    "description": "The working directory for the command. Defaults to the conversation cwd or project root."
                },
                "timeout_ms": {
                    "type": "integer",
                    "description": "The command timeout in milliseconds.",
                    "default": DEFAULT_TIMEOUT_MS,
                    "minimum": 1,
                    "maximum": MAX_TIMEOUT_MS
                },
                "login": {
                    "type": "boolean",
                    "description": "Run the shell as a login shell.",
                    "default": false
                },
                "sandbox_permissions": {
                    "type": "string",
                    "description": "Requested sandbox permissions for this command.",
                    "enum": ["use_default", "with_additional_permissions", "require_escalated"]
                },
                "justification": {
                    "type": "string",
                    "description": "User-facing justification shown when approval is required."
                },
                "prefix_rule": {
                    "type": "array",
                    "description": "Reusable approval prefix for this command.",
                    "items": { "type": "string" }
                },
                "additional_permissions": {
                    "type": "array",
                    "description": "Additional sandbox permissions requested by the model.",
                    "items": { "type": "string" }
                }
            },
            "required": ["command"]
        }
    })
}

pub fn read_only_seatbelt_policy() -> String {
    read_only_seatbelt_policy_with_rtk(None)
}

fn read_only_seatbelt_policy_with_rtk(shared_rtk_state_dir: Option<&Path>) -> String {
    let mut lines = [
        "(version 1)",
        "(deny default)",
        "(deny file-write*)",
        "(deny network*)",
        "(allow file-read*)",
        "(allow file-write-data",
        "  (require-any",
        "    (path \"/dev/null\")",
        "    (path \"/dev/zero\")",
        "    (path \"/dev/dtracehelper\")",
        "    (path \"/dev/tty\")",
        "    (path \"/dev/random\")",
        "    (path \"/dev/urandom\")",
        "    (path \"/dev/stdin\")",
        "    (path \"/dev/stdout\")",
        "    (path \"/dev/stderr\")",
        "    (subpath \"/dev/fd\")))",
        "(allow process-fork)",
        "(allow process-exec)",
        "(allow sysctl-read)",
        "(allow signal (target same-sandbox))",
    ]
    .into_iter()
    .map(str::to_string)
    .collect::<Vec<_>>();

    if let Some(state_dir) = shared_rtk_state_dir {
        let state_dir = sbpl_escape_path(state_dir);
        lines.push(format!("(allow file-write* (subpath \"{state_dir}\"))"));
        lines.push(format!("(allow file-write* (path \"{state_dir}\"))"));
    }

    lines.join("\n")
}

pub fn workspace_write_seatbelt_policy(roots: &WorkspaceRoots) -> String {
    workspace_write_seatbelt_policy_with_rtk(roots, None)
}

fn workspace_write_seatbelt_policy_with_rtk(
    roots: &WorkspaceRoots,
    shared_rtk_state_dir: Option<&Path>,
) -> String {
    let mut lines = vec![
        "(version 1)".to_string(),
        "(deny default)".to_string(),
        "(deny network*)".to_string(),
        "(allow file-read*)".to_string(),
        "(allow file-write-data".to_string(),
        "  (require-any".to_string(),
        "    (path \"/dev/null\")".to_string(),
        "    (path \"/dev/zero\")".to_string(),
        "    (path \"/dev/dtracehelper\")".to_string(),
        "    (path \"/dev/tty\")".to_string(),
        "    (path \"/dev/random\")".to_string(),
        "    (path \"/dev/urandom\")".to_string(),
        "    (path \"/dev/stdin\")".to_string(),
        "    (path \"/dev/stdout\")".to_string(),
        "    (path \"/dev/stderr\")".to_string(),
        "    (subpath \"/dev/fd\")))".to_string(),
        "(allow process-fork)".to_string(),
        "(allow process-exec)".to_string(),
        "(allow sysctl-read)".to_string(),
        "(allow signal (target same-sandbox))".to_string(),
    ];

    for root in roots.roots() {
        let root = sbpl_escape_path(root);
        lines.push(format!("(allow file-write* (subpath \"{root}\"))"));
        lines.push(format!("(allow file-write* (path \"{root}\"))"));
    }

    for root in roots.roots() {
        for name in PROTECTED_ROOT_NAMES {
            let protected = sbpl_escape_path(root.join(name));
            lines.push(format!("(deny file-write* (subpath \"{protected}\"))"));
            lines.push(format!("(deny file-write* (path \"{protected}\"))"));
        }
    }

    if let Some(state_dir) = shared_rtk_state_dir {
        let state_dir = sbpl_escape_path(state_dir);
        lines.push(format!("(allow file-write* (subpath \"{state_dir}\"))"));
        lines.push(format!("(allow file-write* (path \"{state_dir}\"))"));
    }

    lines.join("\n")
}

/// Lists the paths that a sandboxed command can write on the host. The list
/// covers the read-only and workspace-write Seatbelt policies and the
/// bubblewrap arguments of both modes. Each policy also shares
/// `rtk_state_dir`. Bubblewrap binds that directory only when it exists.
#[cfg(test)]
pub(crate) fn sandbox_write_paths(roots: &WorkspaceRoots, rtk_state_dir: &Path) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    for policy in [
        read_only_seatbelt_policy_with_rtk(Some(rtk_state_dir)),
        workspace_write_seatbelt_policy_with_rtk(roots, Some(rtk_state_dir)),
    ] {
        paths.extend(policy.lines().filter_map(|line| {
            line.strip_prefix("(allow file-write* (subpath \"")
                .or_else(|| line.strip_prefix("(allow file-write* (path \""))?
                .strip_suffix("\"))")
                .map(PathBuf::from)
        }));
    }
    for mode in [SandboxMode::ReadOnly, SandboxMode::WorkspaceWrite] {
        let args = bwrap::bubblewrap_args(mode, roots, &[], Some(rtk_state_dir));
        paths.extend(
            args.windows(3)
                .filter(|window| window[0] == "--bind")
                .map(|window| PathBuf::from(&window[2])),
        );
    }
    paths
}

pub fn agent_capabilities() -> AgentCapabilities {
    match sandbox_platform() {
        SandboxPlatform::MacosSeatbelt => AgentCapabilities {
            platform: "macos".to_string(),
            sandbox_available: true,
            message: "macOS Seatbelt sandbox available. Read-only and workspace-write modes deny network by default.".to_string(),
        },
        SandboxPlatform::LinuxBubblewrap => AgentCapabilities {
            platform: "linux".to_string(),
            sandbox_available: true,
            message: "Linux bubblewrap sandbox available. Read-only and workspace-write modes deny network by default.".to_string(),
        },
        SandboxPlatform::Unavailable(message) => AgentCapabilities {
            platform: std::env::consts::OS.to_string(),
            sandbox_available: false,
            message,
        },
    }
}

pub fn approval_key_for(
    request: &ShellCommandRequest,
    conversation_id: &str,
    project_root: &str,
    resolved_cwd: &str,
) -> ApprovalKey {
    ApprovalKey::new_scoped(
        conversation_id,
        project_root,
        &request.command,
        resolved_cwd,
        request.sandbox_permissions,
        request.login.unwrap_or(false),
        effective_timeout_ms(request.timeout_ms),
        request.additional_permissions.clone().unwrap_or_default(),
    )
}

pub fn resolve_shell_cwd(
    request: &ShellCommandRequest,
    default_cwd: &str,
) -> Result<String, String> {
    let requested = request
        .workdir
        .as_deref()
        .filter(|value| !value.trim().is_empty());
    let candidate = match requested {
        Some(path) if Path::new(path).is_absolute() => PathBuf::from(path),
        Some(path) => Path::new(default_cwd).join(path),
        None => PathBuf::from(default_cwd),
    };
    let canonical = candidate.canonicalize().map_err(|error| {
        format!(
            "failed to resolve shell workdir `{}`: {error}",
            candidate.display()
        )
    })?;
    if !canonical.is_dir() {
        return Err(format!(
            "shell workdir is not a directory: {}",
            canonical.display()
        ));
    }
    Ok(canonical.to_string_lossy().into_owned())
}

pub fn needs_approval(request: &ShellCommandRequest) -> bool {
    requests_additional_execution_authority(request)
        || !command_is_provably_read_only(&request.command)
}

pub fn requests_additional_execution_authority(request: &ShellCommandRequest) -> bool {
    request.sandbox_permissions != SandboxPermissions::UseDefault
        || request
            .additional_permissions
            .as_ref()
            .is_some_and(|permissions| !permissions.is_empty())
        || request.login.unwrap_or(false)
        || request
            .timeout_ms
            .is_some_and(|timeout_ms| timeout_ms > DEFAULT_TIMEOUT_MS)
}

pub fn validate_shell_command_request(request: &ShellCommandRequest) -> Result<(), String> {
    if request
        .timeout_ms
        .is_some_and(|timeout_ms| timeout_ms > MAX_TIMEOUT_MS)
    {
        return Err(format!(
            "timeout_ms cannot exceed {MAX_TIMEOUT_MS} milliseconds"
        ));
    }
    Ok(())
}

pub fn denied_tool_output(command: &str) -> String {
    format!("Command denied by user approval policy. Command was not run.\n\n{command}")
}

pub fn denied_patch_output(reason: &str) -> String {
    format!("Patch denied by approval policy. Patch was not applied.\n\n{reason}")
}

pub fn parse_apply_patch(input: &str) -> Result<ParsedPatch, String> {
    apply_patch::parse_patch(input)
}

pub fn apply_patch_file_changes(parsed: &ParsedPatch) -> Vec<PatchFileChange> {
    apply_patch::file_changes(parsed)
}

pub fn apply_patch_touched_paths(parsed: &ParsedPatch) -> Vec<String> {
    apply_patch::touched_paths(parsed)
}

pub fn snapshot_patch_paths(cwd: &Path, parsed: &ParsedPatch) -> Result<Vec<FileSnapshot>, String> {
    apply_patch::snapshot_paths(cwd, &apply_patch::touched_paths(parsed))
}

pub fn apply_parsed_patch_in_process(
    cwd: &Path,
    parsed: &ParsedPatch,
    roots: &WorkspaceRoots,
) -> Result<ApplyPatchSummary, String> {
    apply_patch::apply_parsed_patch(cwd, parsed, roots)
}

pub fn patch_diff(cwd: &Path, snapshots: &mut [FileSnapshot]) -> Result<String, String> {
    apply_patch::unified_diff(cwd, snapshots)
}

pub fn restore_patch_snapshots(cwd: &Path, snapshots: &[FileSnapshot]) -> Result<(), String> {
    let roots = WorkspaceRoots::for_cwd(cwd);
    apply_patch::restore_snapshots(cwd, snapshots, &roots)
}

pub fn assess_patch_safety(
    parsed: &ParsedPatch,
    cwd: &Path,
    roots: &WorkspaceRoots,
    policy: ApprovalPolicy,
) -> PatchSafety {
    if matches!(policy, ApprovalPolicy::SkipAll) {
        return PatchSafety::AutoApproved;
    }
    if matches!(policy, ApprovalPolicy::ReadOnly) {
        return PatchSafety::Denied("effective approval policy is read-only".to_string());
    }

    let mut approval_paths = Vec::new();
    for path in apply_patch::touched_paths(parsed) {
        let absolute = match resolve_workspace_path(cwd, &path) {
            Ok(path) => path,
            Err(error) => return PatchSafety::Denied(error),
        };
        let resolved = match roots.resolve_write_path(&absolute) {
            Ok(resolved) => resolved,
            Err(error) => return PatchSafety::Denied(error),
        };
        if resolved.is_protected {
            return PatchSafety::Denied(format!(
                "`{path}` is under a protected top-level directory"
            ));
        }
        if !resolved.is_writable {
            approval_paths.push(path);
        }
    }

    if approval_paths.is_empty() {
        if matches!(policy, ApprovalPolicy::Ask) {
            PatchSafety::RequiresApproval(Vec::new())
        } else {
            PatchSafety::AutoApproved
        }
    } else if matches!(policy, ApprovalPolicy::Never) {
        PatchSafety::Denied(
            "patch is not provably safe under the workspace sandbox and policy is never"
                .to_string(),
        )
    } else {
        PatchSafety::RequiresApproval(approval_paths)
    }
}

pub async fn apply_patch_sandboxed(
    cwd: PathBuf,
    patch: String,
    roots: WorkspaceRoots,
) -> Result<ApplyPatchSummary, String> {
    if roots.danger_full_access() {
        let parsed = apply_patch::parse_patch(&patch)?;
        return apply_patch::apply_parsed_patch(&cwd, &parsed, &roots);
    }

    if !matches!(sandbox_platform(), SandboxPlatform::MacosSeatbelt) {
        let parsed = apply_patch::parse_patch(&patch)?;
        return apply_patch::apply_parsed_patch(&cwd, &parsed, &roots);
    }

    #[cfg(test)]
    {
        let parsed = apply_patch::parse_patch(&patch)?;
        apply_patch::apply_parsed_patch(&cwd, &parsed, &roots)
    }

    #[cfg(not(test))]
    {
        tokio::task::spawn_blocking(move || apply_patch_helper_blocking(cwd, patch, roots))
            .await
            .map_err(|error| format!("failed to join apply_patch helper: {error}"))?
    }
}

pub async fn restore_patch_snapshots_sandboxed(
    cwd: PathBuf,
    snapshots: Vec<FileSnapshot>,
) -> Result<(), String> {
    let roots = WorkspaceRoots::for_cwd(&cwd);
    if !matches!(sandbox_platform(), SandboxPlatform::MacosSeatbelt) {
        return apply_patch::restore_snapshots(&cwd, &snapshots, &roots);
    }

    #[cfg(test)]
    {
        apply_patch::restore_snapshots(&cwd, &snapshots, &roots)
    }

    #[cfg(not(test))]
    {
        tokio::task::spawn_blocking(move || {
            restore_snapshots_helper_blocking(cwd, snapshots, roots)
        })
        .await
        .map_err(|error| format!("failed to join restore helper: {error}"))?
    }
}

pub fn run_apply_patch_helper_from_stdin() -> i32 {
    let mut input = String::new();
    if let Err(error) = io::stdin().read_to_string(&mut input) {
        let _ = write_helper_response(ApplyPatchHelperResponse {
            ok: false,
            summary: None,
            error: Some(format!("failed to read apply_patch helper stdin: {error}")),
        });
        return 1;
    }
    let request = match serde_json::from_str::<PatchHelperRequest>(&input) {
        Ok(request) => request,
        Err(error) => {
            let _ = write_helper_response(ApplyPatchHelperResponse {
                ok: false,
                summary: None,
                error: Some(format!("invalid apply_patch helper request: {error}")),
            });
            return 1;
        }
    };
    match run_patch_helper_request(request) {
        Ok(summary) => {
            let _ = write_helper_response(ApplyPatchHelperResponse {
                ok: true,
                summary,
                error: None,
            });
            0
        }
        Err(error) => {
            let _ = write_helper_response(ApplyPatchHelperResponse {
                ok: false,
                summary: None,
                error: Some(error),
            });
            1
        }
    }
}

pub fn extract_apply_patch_from_shell_command(command: &str) -> Option<Result<String, String>> {
    let trimmed = command.trim_start();
    let rest = trimmed.strip_prefix("apply_patch")?;
    if !rest.is_empty() && !rest.starts_with(char::is_whitespace) {
        return None;
    }
    let rest = rest.trim_start();
    if rest.contains(BEGIN_PATCH_LITERAL) {
        let begin = rest.find(BEGIN_PATCH_LITERAL).unwrap_or(0);
        let patch = &rest[begin..];
        let Some(end) = patch.find(END_PATCH_LITERAL) else {
            return Some(Err(
                "apply_patch shell command is missing `*** End Patch`".to_string()
            ));
        };
        let end = end + END_PATCH_LITERAL.len();
        return Some(Ok(patch[..end].trim_end_matches('\n').to_string()));
    }
    if let Some((delimiter, body_start)) = parse_heredoc(rest) {
        let body = &rest[body_start..];
        let mut patch = String::new();
        for line in body.split_inclusive('\n') {
            let without_newline = line.trim_end_matches('\n');
            if without_newline == delimiter {
                return Some(Ok(patch));
            }
            patch.push_str(line);
        }
        return Some(Err(format!(
            "apply_patch heredoc delimiter `{delimiter}` was not closed"
        )));
    }
    Some(Err(
        "apply_patch shell command did not include a patch envelope".to_string(),
    ))
}

const BEGIN_PATCH_LITERAL: &str = "*** Begin Patch";
const END_PATCH_LITERAL: &str = "*** End Patch";

pub async fn run_shell_command(
    request: ShellCommandRequest,
    default_cwd: String,
    cancel: Arc<AtomicBool>,
    output_callback: OutputCallback,
) -> Result<ExecResult, ExecError> {
    run_shell_command_sandboxed(
        request,
        default_cwd,
        SandboxMode::ReadOnly,
        None,
        cancel,
        output_callback,
    )
    .await
}

pub async fn run_shell_command_sandboxed(
    request: ShellCommandRequest,
    default_cwd: String,
    sandbox_mode: SandboxMode,
    workspace_roots: Option<WorkspaceRoots>,
    cancel: Arc<AtomicBool>,
    output_callback: OutputCallback,
) -> Result<ExecResult, ExecError> {
    run_shell_command_sandboxed_with_rtk_database(
        request,
        default_cwd,
        sandbox_mode,
        workspace_roots,
        rtk_database_path(),
        cancel,
        output_callback,
    )
    .await
}

pub(crate) async fn run_shell_command_sandboxed_with_rtk_database(
    request: ShellCommandRequest,
    default_cwd: String,
    sandbox_mode: SandboxMode,
    workspace_roots: Option<WorkspaceRoots>,
    rtk_database_path: PathBuf,
    cancel: Arc<AtomicBool>,
    output_callback: OutputCallback,
) -> Result<ExecResult, ExecError> {
    let plan = build_exec_plan(request, &default_cwd, rtk_database_path);
    let platform = sandbox_platform();
    if !matches!(sandbox_mode, SandboxMode::DangerFullAccess) {
        if let SandboxPlatform::Unavailable(message) = platform {
            return Ok(platform_unavailable_result(&plan.command, message));
        }
    }
    let roots = workspace_roots.unwrap_or_else(|| WorkspaceRoots::for_cwd(&plan.cwd));

    tokio::task::spawn_blocking(move || {
        run_shell_command_blocking(plan, sandbox_mode, roots, cancel, output_callback)
    })
    .await
    .map_err(|error| ExecError::Join(format!("failed to join shell executor: {error}")))?
}

pub fn format_model_output(exit_code: i32, wall_time: Duration, output: &[u8]) -> (String, bool) {
    let output_text = String::from_utf8_lossy(output);
    let total_output_lines = count_lines(output_text.as_ref());
    let (body, truncated) = middle_truncate_utf8(output_text.as_ref(), MODEL_OUTPUT_LIMIT_BYTES);
    (
        format!(
            "Exit code: {exit_code}\nWall time: {:.3} seconds\n[Total output lines: {total_output_lines}]\nOutput:\n{body}",
            wall_time.as_secs_f64()
        ),
        truncated,
    )
}

fn build_exec_plan(
    request: ShellCommandRequest,
    default_cwd: &str,
    rtk_database_path: PathBuf,
) -> ExecPlan {
    let cwd = effective_cwd_string(&request, default_cwd);
    ExecPlan {
        command: request.command,
        cwd: PathBuf::from(cwd),
        timeout_ms: effective_timeout_ms(request.timeout_ms),
        login: request.login.unwrap_or(false),
        rtk_binary: locate_rtk_binary(),
        rtk_database_path,
    }
}

/// Shared, sandbox-writable RTK accounting database for Halv and its managed
/// Headroom child.
pub(crate) fn rtk_database_path() -> PathBuf {
    rtk_state_directory().join("history.db")
}

pub(crate) fn rtk_state_directory() -> PathBuf {
    RTK_STATE_DIRECTORY.get().cloned().unwrap_or_else(|| {
        canonical_temp_directory().join(format!(
            "{RTK_STATE_DIRECTORY_PREFIX}{}",
            std::process::id()
        ))
    })
}

/// Records the RTK state directory and the launcher root of this app data
/// directory. Call it during app setup, before a command runs.
pub(crate) fn initialize_rtk_state_directory(app_data_dir: &Path) -> PathBuf {
    LAUNCHER_ROOT.get_or_init(|| launcher_root_for(app_data_dir));
    initialize_rtk_state_directory_in(
        &RTK_STATE_DIRECTORY,
        app_data_dir,
        &canonical_temp_directory(),
        current_user_id(),
    )
}

fn initialize_rtk_state_directory_in(
    state_directory: &OnceLock<PathBuf>,
    app_data_dir: &Path,
    temp_dir: &Path,
    user_id: u32,
) -> PathBuf {
    state_directory
        .get_or_init(|| rtk_state_directory_for(app_data_dir, temp_dir, user_id))
        .clone()
}

fn canonical_temp_directory() -> PathBuf {
    let temp_dir = std::env::temp_dir();
    temp_dir.canonicalize().unwrap_or(temp_dir)
}

fn rtk_state_directory_for(app_data_dir: &Path, temp_dir: &Path, user_id: u32) -> PathBuf {
    let mut hash = 0x811c9dc5u32;
    for byte in app_data_dir.as_os_str().to_string_lossy().as_bytes() {
        hash ^= u32::from(*byte);
        hash = hash.wrapping_mul(0x01000193);
    }
    temp_dir.join(format!("{RTK_STATE_DIRECTORY_PREFIX}{user_id}-{hash:08x}"))
}

/// The RTK and Crux launchers go under the app data directory. Processes
/// outside the sandbox run these launchers first on PATH. A sandboxed
/// command can write to the RTK state directory and the temp directory, so
/// the launchers must not go there. No sandbox write root contains the app
/// data directory.
fn launcher_root_for(app_data_dir: &Path) -> PathBuf {
    app_data_dir.join(LAUNCHER_DIRECTORY_NAME)
}

pub(crate) fn launcher_root() -> Option<PathBuf> {
    if let Some(root) = LAUNCHER_ROOT.get() {
        return Some(root.clone());
    }
    // Unit tests run without the app setup. Other builds never use the temp
    // directory, because a sandboxed command can write there.
    #[cfg(test)]
    return Some(
        canonical_temp_directory().join(format!("halv-launchers-test-{}", std::process::id())),
    );
    #[cfg(not(test))]
    None
}

#[cfg(unix)]
fn current_user_id() -> u32 {
    // SAFETY: geteuid reads the effective user ID and has no preconditions.
    unsafe { libc::geteuid() }
}

#[cfg(not(unix))]
fn current_user_id() -> u32 {
    let mut hash = 0x811c9dc5u32;
    for byte in std::env::var_os("USERNAME")
        .unwrap_or_default()
        .to_string_lossy()
        .as_bytes()
    {
        hash ^= u32::from(*byte);
        hash = hash.wrapping_mul(0x01000193);
    }
    hash
}

/// Session-scoped crux savings ledger (JSON lines appended by the crux
/// MCP server when CRUX_SAVINGS_LEDGER is set). The sidebar meter polls it.
pub(crate) fn crux_session_ledger_path(session_id: &str) -> PathBuf {
    rtk_state_directory().join(format!("crux-session-{session_id}.jsonl"))
}

pub(crate) fn savings_source_exists(path: &Path) -> bool {
    path.is_file()
}

/// Sum of saved_tokens across the crux ledger lines. Unreadable lines are
/// skipped: the ledger is append-only and a torn final line heals on the
/// next poll.
pub(crate) fn crux_tokens_saved_at(ledger_path: &Path) -> Option<u64> {
    if !ledger_path.is_file() {
        return Some(0);
    }
    let contents = std::fs::read_to_string(ledger_path).ok()?;
    Some(
        contents
            .lines()
            .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
            .filter_map(|value| value.get("saved_tokens")?.as_u64())
            .sum(),
    )
}

/// Session-scoped RTK ledger for one embedded terminal. The sidebar meter
/// polls this file, so shell savings attribute to exactly one session.
pub(crate) fn rtk_session_database_path(session_id: &str) -> PathBuf {
    rtk_state_directory().join(format!("session-{session_id}.db"))
}

pub(crate) fn find_sibling_rtk_state_file(file_name: &str) -> Option<PathBuf> {
    find_sibling_rtk_state_file_in(&canonical_temp_directory(), file_name)
}

fn find_sibling_rtk_state_file_in(temp_dir: &Path, file_name: &str) -> Option<PathBuf> {
    if Path::new(file_name)
        .file_name()
        .and_then(|name| name.to_str())
        != Some(file_name)
    {
        return None;
    }
    let mut state_dirs = fs::read_dir(temp_dir)
        .ok()?
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|file_type| file_type.is_dir()))
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with(RTK_STATE_DIRECTORY_PREFIX)
        })
        .map(|entry| entry.path())
        .collect::<Vec<_>>();
    state_dirs.sort();
    state_dirs
        .into_iter()
        .map(|state_dir| state_dir.join(file_name))
        .find(|candidate| candidate.is_file())
}

#[derive(Debug, Clone)]
pub(crate) struct RunningRtkSessionReference {
    pub id: String,
    pub rtk_db_path: Option<PathBuf>,
    pub crux_ledger_path: Option<PathBuf>,
}

pub(crate) fn spawn_stale_rtk_state_cleanup(references: Vec<RunningRtkSessionReference>) {
    if let Err(error) = spawn_stale_rtk_state_cleanup_in(
        canonical_temp_directory(),
        rtk_state_directory(),
        references,
    ) {
        eprintln!("rtk state: failed to start cleanup: {error}");
    }
}

/// Removes the RTK state directories of stopped app processes. The cleanup
/// never removes a path in the current state directory. A sandboxed command
/// can replace that directory with a symbolic link to any directory. A
/// removal through that link would delete files outside the sandbox. Builds
/// before #51 left launchers in the current state directory. No process of
/// this build runs them, so they stay there.
fn spawn_stale_rtk_state_cleanup_in(
    temp_dir: PathBuf,
    current_state_dir: PathBuf,
    references: Vec<RunningRtkSessionReference>,
) -> io::Result<thread::JoinHandle<()>> {
    thread::Builder::new()
        .name("rtk-state-cleanup".to_string())
        .spawn(move || {
            lower_cleanup_thread_priority();
            let removed = cleanup_stale_rtk_directories_in(
                &temp_dir,
                &current_state_dir,
                &references,
                process_is_alive,
            );
            eprintln!("rtk state: removed {removed} stale directories");
        })
}

#[cfg(target_os = "macos")]
fn lower_cleanup_thread_priority() {
    // SAFETY: This call changes only the current thread's quality of service.
    unsafe {
        libc::pthread_set_qos_class_self_np(libc::qos_class_t::QOS_CLASS_BACKGROUND, 0);
    }
}

#[cfg(not(target_os = "macos"))]
fn lower_cleanup_thread_priority() {}

fn cleanup_stale_rtk_directories_in<F>(
    temp_dir: &Path,
    current_state_dir: &Path,
    references: &[RunningRtkSessionReference],
    process_is_alive: F,
) -> usize
where
    F: Fn(u32) -> bool,
{
    let Ok(entries) = fs::read_dir(temp_dir) else {
        return 0;
    };
    let mut removed = 0;
    for entry in entries.filter_map(Result::ok) {
        if !entry.file_type().is_ok_and(|file_type| file_type.is_dir()) {
            continue;
        }
        let path = entry.path();
        if path == current_state_dir {
            continue;
        }
        let name = entry.file_name();
        let Some(pid) = name
            .to_string_lossy()
            .strip_prefix(RTK_STATE_DIRECTORY_PREFIX)
            .and_then(|value| value.parse::<u32>().ok())
        else {
            continue;
        };
        if process_is_alive(pid) || rtk_directory_is_referenced(&path, references) {
            continue;
        }
        if fs::remove_dir_all(&path).is_ok() {
            removed += 1;
        }
    }
    removed
}

fn rtk_directory_is_referenced(
    directory: &Path,
    references: &[RunningRtkSessionReference],
) -> bool {
    references.iter().any(|reference| {
        let explicit_path_is_in_directory = [&reference.rtk_db_path, &reference.crux_ledger_path]
            .into_iter()
            .flatten()
            .any(|path| path.is_file() && path.parent() == Some(directory));
        explicit_path_is_in_directory
            || directory
                .join(format!("session-{}.db", reference.id))
                .is_file()
            || directory
                .join(format!("crux-session-{}.jsonl", reference.id))
                .is_file()
    })
}

#[cfg(unix)]
fn process_is_alive(pid: u32) -> bool {
    let Ok(pid) = i32::try_from(pid) else {
        return false;
    };
    // SAFETY: kill with signal zero checks one process and sends no signal.
    let result = unsafe { libc::kill(pid, 0) };
    result == 0 || io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
}

#[cfg(not(unix))]
fn process_is_alive(_pid: u32) -> bool {
    true
}

/// GUI shell tools use one RTK database per message turn. This prevents
/// concurrent GUI turns from sharing one RTK counter baseline.
pub(crate) fn rtk_gui_turn_database_path(turn_id: &str) -> PathBuf {
    let safe_turn_id = turn_id
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
                character
            } else {
                '_'
            }
        })
        .take(64)
        .collect::<String>();
    rtk_state_directory().join(format!("gui-turn-{safe_turn_id}.db"))
}

/// Exact RTK tokens avoided by shell commands executed by this Halv process.
///
/// Headroom also polls this database, but its context-tool metrics are cached.
/// Reading the source database here lets the chat settle path attribute RTK
/// savings to the turn that produced them without waiting for that poll.
pub(crate) fn rtk_tokens_saved() -> Option<u64> {
    rtk_tokens_saved_at(&rtk_database_path())
}

pub(crate) fn rtk_tokens_saved_at(database_path: &Path) -> Option<u64> {
    if !database_path.is_file() {
        return Some(0);
    }

    let connection = rusqlite::Connection::open_with_flags(
        database_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .ok()?;
    let commands_table_exists = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'commands')",
            [],
            |row| row.get::<_, bool>(0),
        )
        .ok()?;
    if !commands_table_exists {
        return Some(0);
    }

    let saved_tokens = connection
        .query_row(
            "SELECT COALESCE(SUM(saved_tokens), 0) FROM commands",
            [],
            |row| row.get::<_, i64>(0),
        )
        .ok()?;
    u64::try_from(saved_tokens).ok()
}

/// PATH value that makes the exact RTK selected by Halv available to the
/// managed Headroom child for RTK gain polling on a clean installation.
pub(crate) fn rtk_path_environment() -> Option<OsString> {
    let binary = locate_rtk_binary()?;
    let database_path = rtk_database_path();
    prepare_rtk_state_directory(&database_path)
        .then(|| path_with_preferred_rtk(&binary))
        .flatten()
}

pub(crate) fn bundled_rtk_binary_path() -> Option<PathBuf> {
    let current_bundle = std::env::current_exe().ok().and_then(|path| {
        path.parent()
            .map(|directory| directory.join(rtk_binary_name()))
            .and_then(executable_rtk_file)
    });
    current_bundle
        .or_else(|| staged_rtk_binary_path().and_then(executable_rtk_file))
        .and_then(|path| path.canonicalize().ok())
}

fn locate_rtk_binary() -> Option<PathBuf> {
    locate_rtk_binary_with_env(
        std::env::var_os(HALV_RTK_BINARY_ENV),
        std::env::current_exe().ok(),
        staged_rtk_binary_path(),
        std::env::var_os(HEADROOM_WORKSPACE_DIR_ENV),
        std::env::var_os("HOME"),
        std::env::var_os("PATH"),
    )
}

fn locate_rtk_binary_with_env(
    explicit_override: Option<OsString>,
    current_exe: Option<PathBuf>,
    staged_binary: Option<PathBuf>,
    headroom_workspace: Option<OsString>,
    home: Option<OsString>,
    path: Option<OsString>,
) -> Option<PathBuf> {
    explicit_override
        .and_then(|path| executable_rtk_file(PathBuf::from(path)))
        .or_else(|| {
            current_exe.and_then(|path| {
                path.parent()
                    .map(|directory| directory.join(rtk_binary_name()))
                    .and_then(executable_rtk_file)
            })
        })
        .or_else(|| staged_binary.and_then(executable_rtk_file))
        .or_else(|| {
            headroom_workspace
                .map(PathBuf::from)
                .map(|root| root.join("bin").join(rtk_binary_name()))
                .and_then(executable_rtk_file)
        })
        .or_else(|| {
            home.map(PathBuf::from)
                .map(|root| root.join(".headroom/bin").join(rtk_binary_name()))
                .and_then(executable_rtk_file)
        })
        .or_else(|| find_rtk_on_path(path.as_deref()))
}

fn staged_rtk_binary_path() -> Option<PathBuf> {
    let target = staged_rtk_target_triple()?;
    let filename = if cfg!(windows) {
        format!("rtk-{target}.exe")
    } else {
        format!("rtk-{target}")
    };
    Some(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("binaries")
            .join(filename),
    )
}

fn staged_rtk_target_triple() -> Option<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => Some("aarch64-apple-darwin"),
        ("macos", "x86_64") => Some("x86_64-apple-darwin"),
        ("linux", "aarch64") => Some("aarch64-unknown-linux-gnu"),
        ("linux", "x86_64") => Some("x86_64-unknown-linux-gnu"),
        ("windows", "x86_64") => Some("x86_64-pc-windows-msvc"),
        _ => None,
    }
}

fn rtk_binary_name() -> &'static str {
    if cfg!(windows) {
        "rtk.exe"
    } else {
        "rtk"
    }
}

fn find_rtk_on_path(path: Option<&std::ffi::OsStr>) -> Option<PathBuf> {
    path.into_iter()
        .flat_map(std::env::split_paths)
        .filter(|directory| !directory.as_os_str().is_empty())
        .map(|directory| directory.join(rtk_binary_name()))
        .find_map(executable_rtk_file)
}

fn executable_rtk_file(path: PathBuf) -> Option<PathBuf> {
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

fn effective_timeout_ms(timeout_ms: Option<u64>) -> u64 {
    timeout_ms
        .unwrap_or(DEFAULT_TIMEOUT_MS)
        .clamp(1, MAX_TIMEOUT_MS)
}

fn effective_cwd_string(request: &ShellCommandRequest, default_cwd: &str) -> String {
    request
        .workdir
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(default_cwd)
        .to_string()
}

#[cfg(not(test))]
fn apply_patch_helper_blocking(
    cwd: PathBuf,
    patch: String,
    roots: WorkspaceRoots,
) -> Result<ApplyPatchSummary, String> {
    let helper_request = PatchHelperRequest::Apply {
        cwd: cwd.to_string_lossy().to_string(),
        patch,
        roots: roots
            .roots()
            .iter()
            .map(|path| path.to_string_lossy().to_string())
            .collect(),
    };
    run_patch_helper_blocking(helper_request, roots)?
        .ok_or_else(|| "apply_patch helper returned no summary".to_string())
}

#[cfg(not(test))]
fn restore_snapshots_helper_blocking(
    cwd: PathBuf,
    snapshots: Vec<FileSnapshot>,
    roots: WorkspaceRoots,
) -> Result<(), String> {
    let helper_request = PatchHelperRequest::Restore {
        cwd: cwd.to_string_lossy().to_string(),
        snapshots,
        roots: roots
            .roots()
            .iter()
            .map(|path| path.to_string_lossy().to_string())
            .collect(),
    };
    run_patch_helper_blocking(helper_request, roots).map(|_| ())
}

#[cfg(not(test))]
fn run_patch_helper_blocking(
    helper_request: PatchHelperRequest,
    roots: WorkspaceRoots,
) -> Result<Option<ApplyPatchSummary>, String> {
    let payload = serde_json::to_vec(&helper_request)
        .map_err(|error| format!("failed to serialize apply_patch helper request: {error}"))?;
    let current_exe = std::env::current_exe()
        .map_err(|error| format!("failed to locate Halv executable: {error}"))?;
    let mut command = Command::new(SANDBOX_EXEC_PATH);
    command
        .arg("-p")
        .arg(workspace_write_seatbelt_policy(&roots))
        .arg("--")
        .arg(current_exe)
        .env("HALV_APPLY_PATCH_HELPER", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    host_env::apply_to_command(&mut command);
    crate::process::hide_console(&mut command);
    let mut child = command
        .spawn()
        .map_err(|error| format!("failed to spawn sandboxed apply_patch helper: {error}"))?;

    {
        let stdin = child
            .stdin
            .as_mut()
            .ok_or_else(|| "failed to open apply_patch helper stdin".to_string())?;
        stdin
            .write_all(&payload)
            .map_err(|error| format!("failed to write apply_patch helper request: {error}"))?;
    }

    let output = child
        .wait_with_output()
        .map_err(|error| format!("failed to wait for apply_patch helper: {error}"))?;
    let response =
        serde_json::from_slice::<ApplyPatchHelperResponse>(&output.stdout).map_err(|error| {
            let stderr = String::from_utf8_lossy(&output.stderr);
            format!("invalid apply_patch helper response: {error}; stderr: {stderr}")
        })?;
    if output.status.success() && response.ok {
        Ok(response.summary)
    } else {
        Err(response.error.unwrap_or_else(|| {
            format!(
                "apply_patch helper failed with exit status {}",
                output.status
            )
        }))
    }
}

fn run_patch_helper_request(
    request: PatchHelperRequest,
) -> Result<Option<ApplyPatchSummary>, String> {
    match request {
        PatchHelperRequest::Apply { cwd, patch, roots } => {
            let roots = helper_workspace_roots(roots);
            apply_patch::parse_patch(&patch)
                .and_then(|parsed| {
                    apply_patch::apply_parsed_patch(Path::new(&cwd), &parsed, &roots)
                })
                .map(Some)
        }
        PatchHelperRequest::Restore {
            cwd,
            snapshots,
            roots,
        } => {
            let roots = helper_workspace_roots(roots);
            apply_patch::restore_snapshots(Path::new(&cwd), &snapshots, &roots)?;
            Ok(None)
        }
    }
}

fn helper_workspace_roots(roots: Vec<String>) -> WorkspaceRoots {
    WorkspaceRoots::new(roots.into_iter().map(PathBuf::from).collect())
}

fn write_helper_response(response: ApplyPatchHelperResponse) -> Result<(), String> {
    let serialized = serde_json::to_string(&response)
        .map_err(|error| format!("failed to serialize apply_patch helper response: {error}"))?;
    println!("{serialized}");
    Ok(())
}

fn parse_heredoc(rest: &str) -> Option<(String, usize)> {
    let rest = rest.trim_start();
    let after = rest.strip_prefix("<<")?.trim_start();
    let line_end = after.find('\n')?;
    let delimiter = after[..line_end].trim();
    let delimiter = delimiter
        .trim_matches('\'')
        .trim_matches('"')
        .trim()
        .to_string();
    if delimiter.is_empty() {
        return None;
    }
    let body_start = rest.find('\n')? + 1;
    Some((delimiter, body_start))
}

pub fn resolve_workspace_path(cwd: &Path, path: &str) -> Result<PathBuf, String> {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return Err("path cannot be empty".to_string());
    }
    let path = Path::new(trimmed);
    let combined = if path.is_absolute() {
        path.to_path_buf()
    } else {
        let cwd = cwd
            .canonicalize()
            .map_err(|error| format!("failed to canonicalize cwd `{}`: {error}", cwd.display()))?;
        cwd.join(path)
    };
    Ok(normalize_path_lexically(&combined))
}

/// Resolve an existing path for read access while confining its real location
/// to the project root. Relative paths are interpreted from the project root;
/// absolute paths must still remain inside it.
pub fn resolve_project_read_path(project_root: &Path, path: &str) -> Result<PathBuf, String> {
    let root = project_root.canonicalize().map_err(|error| {
        format!(
            "failed to canonicalize project root `{}`: {error}",
            project_root.display()
        )
    })?;
    if !root.is_dir() {
        return Err(format!(
            "project root `{}` is not a directory",
            root.display()
        ));
    }

    let requested = resolve_workspace_path(&root, path)?;
    if !path_is_within(&requested, &root) {
        return Err(format!(
            "`{}` resolves outside the project root `{}`",
            path.trim(),
            root.display()
        ));
    }

    let real = requested.canonicalize().map_err(|error| {
        format!(
            "failed to resolve project path `{}`: {error}",
            requested.display()
        )
    })?;
    if !path_is_within(&real, &root) {
        return Err(format!(
            "`{}` resolves outside the project root `{}` through a symbolic link",
            path.trim(),
            root.display()
        ));
    }
    Ok(real)
}

pub fn patch_approval_key_for(
    conversation_id: &str,
    cwd: &Path,
    roots: &WorkspaceRoots,
    path: &str,
) -> Result<PatchApprovalKey, String> {
    let project_root = cwd
        .canonicalize()
        .map_err(|error| format!("failed to canonicalize cwd `{}`: {error}", cwd.display()))?;
    let absolute_path = resolve_workspace_path(&project_root, path)?;
    let resolved = roots.resolve_write_path(&absolute_path)?;
    Ok(PatchApprovalKey {
        conversation_id: conversation_id.to_string(),
        project_root: normalize_path_lexically(&project_root),
        real_path: resolved.real_path,
    })
}

fn normalize_path_lexically(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(Path::new("/")),
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            Component::Normal(part) => normalized.push(part),
        }
    }
    normalized
}

fn resolve_real_path_without_symlinks(path: &Path) -> Result<PathBuf, String> {
    let normalized = normalize_path_lexically(path);
    if !normalized.is_absolute() {
        return Err(format!(
            "`{}` is not an absolute path",
            normalized.display()
        ));
    }

    let parts = normalized
        .components()
        .filter_map(|component| match component {
            Component::Normal(part) => Some(part.to_os_string()),
            _ => None,
        })
        .collect::<Vec<_>>();
    let mut current = PathBuf::from("/");
    let mut existing = current.clone();
    let mut remaining_start = parts.len();
    let mut last_existing_is_dir = true;

    for (index, part) in parts.iter().enumerate() {
        if !last_existing_is_dir {
            return Err(format!(
                "`{}` has a non-directory path component",
                normalized.display()
            ));
        }

        current.push(part);
        match std::fs::symlink_metadata(&current) {
            Ok(metadata) => {
                if metadata.file_type().is_symlink() {
                    return Err(format!(
                        "`{}` contains a symbolic link path component",
                        normalized.display()
                    ));
                }
                existing = current.clone();
                remaining_start = index + 1;
                last_existing_is_dir = metadata.is_dir();
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                remaining_start = index;
                break;
            }
            Err(error) => {
                return Err(format!(
                    "failed to inspect `{}`: {error}",
                    current.display()
                ));
            }
        }
    }

    if remaining_start < parts.len() && !last_existing_is_dir {
        return Err(format!(
            "`{}` has a non-directory path component",
            normalized.display()
        ));
    }

    let mut real = existing
        .canonicalize()
        .map_err(|error| format!("failed to canonicalize `{}`: {error}", existing.display()))?;
    for part in &parts[remaining_start..] {
        real.push(part);
    }
    Ok(normalize_path_lexically(&real))
}

fn path_is_within(path: &Path, root: &Path) -> bool {
    path == root || path.starts_with(root)
}

fn path_is_protected(path: &Path, roots: &[PathBuf]) -> bool {
    roots.iter().any(|root| {
        PROTECTED_ROOT_NAMES
            .iter()
            .map(|name| normalize_path_lexically(&root.join(name)))
            .any(|protected| path_is_within(path, &protected))
    })
}

fn sbpl_escape_path(path: impl AsRef<Path>) -> String {
    path.as_ref()
        .to_string_lossy()
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
}

#[cfg(target_os = "linux")]
fn protected_root_paths(roots: &WorkspaceRoots) -> Vec<PathBuf> {
    roots
        .roots()
        .iter()
        .flat_map(|root| {
            PROTECTED_ROOT_NAMES
                .iter()
                .map(move |name| normalize_path_lexically(&root.join(name)))
        })
        .filter(|path| path.exists())
        .collect()
}

fn try_rewrite_shell_command_with_rtk(
    original_command: &str,
    cwd: &Path,
    binary: PathBuf,
    database_path: PathBuf,
    workspace_roots: &WorkspaceRoots,
    cancel: &AtomicBool,
    options: RtkRewriteProbeOptions,
) -> Result<RtkRewriteAttempt, ExecError> {
    if cancel.load(Ordering::SeqCst) {
        return Err(ExecError::Cancelled);
    }

    let Some(runtime) = prepare_active_rtk(binary, database_path) else {
        return Ok(RtkRewriteAttempt::unavailable());
    };
    let Some(mut command) = rtk_rewrite_process(
        &runtime.binary,
        original_command,
        options.sandbox_mode,
        workspace_roots,
        &runtime.database_path,
    ) else {
        return Ok(RtkRewriteAttempt::unavailable());
    };
    command
        .current_dir(cwd)
        .env("PATH", &runtime.path)
        .env(RTK_DB_PATH_ENV, &runtime.database_path)
        .env(RTK_TELEMETRY_DISABLED_ENV, "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    host_env::apply_to_command(&mut command);
    crate::process::hide_console(&mut command);
    configure_process_group(&mut command);

    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(_) => return Ok(RtkRewriteAttempt::unavailable()),
    };
    let process_family = match ProcessFamily::attach(&child, None) {
        Ok(process_family) => process_family,
        Err(_) => {
            let _ = child.kill();
            let _ = child.wait();
            return Ok(RtkRewriteAttempt::unavailable());
        }
    };
    let captured = Arc::new(Mutex::new(Capture::with_limit(
        RTK_REWRITE_OUTPUT_LIMIT_BYTES,
    )));
    let mut readers = Vec::new();
    if let Some(stdout) = child.stdout.take() {
        readers.push(spawn_reader(
            stdout,
            captured.clone(),
            OutputCallback::new(|_| {}),
        ));
    }

    let started = Instant::now();
    let status = loop {
        if cancel.load(Ordering::SeqCst) {
            process_family.terminate();
            let _ = wait_after_kill(&mut child, CHILD_KILL_GRACE);
            drain_readers(readers, READER_DRAIN_GRACE);
            return Err(ExecError::Cancelled);
        }
        match child.try_wait() {
            Ok(Some(status)) => {
                process_family.terminate();
                break status;
            }
            Ok(None) if started.elapsed() < options.timeout => {
                thread::sleep(Duration::from_millis(25));
            }
            Ok(None) => {
                let elapsed = started.elapsed();
                process_family.terminate();
                let _ = wait_after_kill(&mut child, CHILD_KILL_GRACE);
                drain_readers(readers, READER_DRAIN_GRACE);
                return Ok(RtkRewriteAttempt::fallback(format!(
                    "RTK rewrite timed out after {elapsed:?}; the timeout was {:?}. The command ran without a rewrite.",
                    options.timeout
                )));
            }
            Err(error) => {
                let elapsed = started.elapsed();
                process_family.terminate();
                let _ = wait_after_kill(&mut child, CHILD_KILL_GRACE);
                drain_readers(readers, READER_DRAIN_GRACE);
                return Ok(RtkRewriteAttempt::fallback(format!(
                    "RTK rewrite status check failed after {elapsed:?}: {error}. The command ran without a rewrite."
                )));
            }
        }
    };

    drain_readers(readers, READER_DRAIN_GRACE);
    let captured = captured.lock().expect("capture lock poisoned");
    if captured.truncated {
        return Ok(RtkRewriteAttempt::fallback(format!(
            "RTK rewrite output exceeded the {}-byte capture limit. The command ran without a rewrite.",
            RTK_REWRITE_OUTPUT_LIMIT_BYTES
        )));
    }
    let exit_code = exit_code_from_status(status);
    // Exit 1 is RTK's normal "no safe rewrite" result. Keep RTK in the
    // execution path through its lossless tracked proxy instead of bypassing
    // it, including on the first shell command of a conversation.
    if exit_code == 1 {
        return Ok(RtkRewriteAttempt::routed(RtkRoute::Tracked { runtime }));
    }
    if !matches!(exit_code, 0 | 3) {
        return Ok(RtkRewriteAttempt::fallback(format!(
            "RTK rewrite exited with status {exit_code}. The command ran without a rewrite."
        )));
    }
    let Ok(rewritten) = std::str::from_utf8(&captured.bytes) else {
        return Ok(RtkRewriteAttempt::fallback(
            "RTK rewrite produced invalid UTF-8 output. The command ran without a rewrite."
                .to_string(),
        ));
    };
    let rewritten = rewritten.trim();
    if rewritten.is_empty() {
        return Ok(RtkRewriteAttempt::fallback(
            "RTK rewrite produced empty output. The command ran without a rewrite.".to_string(),
        ));
    }
    // RTK returns exit 3 for an already-wrapped command and echoes it
    // unchanged. Keep the active runtime in that case so a bundled `rtk`
    // remains resolvable and uses Halv's shared accounting database.
    if rewritten == original_command && exit_code != 3 {
        return Ok(RtkRewriteAttempt::routed(RtkRoute::Tracked { runtime }));
    }

    Ok(RtkRewriteAttempt::routed(RtkRoute::Rewritten {
        command: rewritten.to_string(),
        runtime,
    }))
}

fn prepare_active_rtk(binary: PathBuf, database_path: PathBuf) -> Option<ActiveRtk> {
    if !prepare_rtk_state_directory(&database_path) {
        return None;
    }
    let path = path_with_preferred_rtk(&binary)?;
    Some(ActiveRtk {
        binary,
        database_path,
        path,
    })
}

fn prepare_rtk_state_directory(database_path: &Path) -> bool {
    let Some(directory) = database_path.parent() else {
        return false;
    };
    if !prepare_private_directory(directory) {
        return false;
    }
    match fs::symlink_metadata(database_path) {
        Ok(metadata) => metadata.is_file() && !metadata.file_type().is_symlink(),
        Err(error) => error.kind() == io::ErrorKind::NotFound,
    }
}

/// Creates a directory that only the current user can use. The function
/// refuses a symbolic link and a directory of another user.
fn prepare_private_directory(directory: &Path) -> bool {
    if fs::create_dir_all(directory).is_err() {
        return false;
    }
    let Ok(metadata) = fs::symlink_metadata(directory) else {
        return false;
    };
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        if metadata.uid() != unsafe { libc::geteuid() } {
            return false;
        }
        if fs::set_permissions(directory, fs::Permissions::from_mode(0o700)).is_err() {
            return false;
        }
    }
    true
}

fn path_with_preferred_rtk(binary: &Path) -> Option<OsString> {
    path_with_preferred_binary(
        binary,
        std::ffi::OsStr::new(rtk_binary_name()),
        launcher_root().as_deref(),
        std::env::var_os("PATH").as_deref(),
    )
}

/// Puts the directory of `binary` first on PATH. When the file name of
/// `binary` is not `binary_name`, a launcher under `launcher_root` gives the
/// binary that name. When the function needs a launcher and has no launcher
/// root, it gives no PATH.
pub(crate) fn path_with_preferred_binary(
    binary: &Path,
    binary_name: &std::ffi::OsStr,
    launcher_root: Option<&Path>,
    current_path: Option<&std::ffi::OsStr>,
) -> Option<OsString> {
    let binary_parent = binary.parent()?.to_path_buf();
    let launcher_dir = if binary.file_name()? == binary_name {
        binary_parent
    } else {
        prepare_named_binary_launcher(binary, binary_name, launcher_root?)?
    };
    let mut paths = vec![launcher_dir.clone()];
    if let Some(path) = current_path {
        paths.extend(
            std::env::split_paths(path)
                .filter(|path| !path.as_os_str().is_empty() && *path != launcher_dir),
        );
    }
    std::env::join_paths(paths).ok()
}

/// Gives the directory of a launcher that runs `binary` as `binary_name`.
/// Each call checks that the launcher links directly to `binary`, and
/// replaces a launcher that does not.
fn prepare_named_binary_launcher(
    binary: &Path,
    binary_name: &std::ffi::OsStr,
    launcher_root: &Path,
) -> Option<PathBuf> {
    let binary = fs::canonicalize(binary).ok()?;
    let mut binary_path_hash = DefaultHasher::new();
    binary.hash(&mut binary_path_hash);
    let launcher_dir = launcher_root.join(format!("{:016x}", binary_path_hash.finish()));
    if !prepare_private_directory(launcher_root) || !prepare_private_directory(&launcher_dir) {
        return None;
    }
    let launcher = launcher_dir.join(binary_name);

    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;

        let links_to_binary =
            |launcher: &Path| fs::read_link(launcher).ok().as_deref() == Some(binary.as_path());
        match fs::symlink_metadata(&launcher) {
            Ok(_) if links_to_binary(&launcher) => return Some(launcher_dir),
            Ok(_) => fs::remove_file(&launcher).ok()?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(_) => return None,
        }
        if let Err(error) = symlink(&binary, &launcher) {
            // Another command using this exact pinned binary may have created
            // the launcher concurrently between the checks above.
            if error.kind() != io::ErrorKind::AlreadyExists || !links_to_binary(&launcher) {
                return None;
            }
        }
    }
    #[cfg(windows)]
    {
        fs::copy(&binary, &launcher).ok()?;
    }
    #[cfg(not(any(unix, windows)))]
    {
        return None;
    }

    Some(launcher_dir)
}

fn command_assigns_path(command: &str) -> bool {
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Quote {
        None,
        Single,
        Double,
    }

    let mut quote = Quote::None;
    let mut escaped = false;
    let mut word = String::new();
    let flush_word = |word: &mut String| {
        let assigns_path = word.starts_with("PATH=");
        word.clear();
        assigns_path
    };

    for character in command.chars() {
        if escaped {
            word.push(character);
            escaped = false;
            continue;
        }
        match quote {
            Quote::Single => {
                if character == '\'' {
                    quote = Quote::None;
                }
            }
            Quote::Double => match character {
                '"' => quote = Quote::None,
                '\\' => escaped = true,
                _ => {}
            },
            Quote::None => match character {
                '\'' => quote = Quote::Single,
                '"' => quote = Quote::Double,
                '\\' => escaped = true,
                value if value.is_whitespace() || matches!(value, ';' | '|' | '&' | '(' | ')') => {
                    if flush_word(&mut word) {
                        return true;
                    }
                }
                _ => word.push(character),
            },
        }
    }
    flush_word(&mut word)
}

fn rtk_rewrite_process(
    binary: &Path,
    original_command: &str,
    sandbox_mode: SandboxMode,
    workspace_roots: &WorkspaceRoots,
    database_path: &Path,
) -> Option<Command> {
    if matches!(sandbox_mode, SandboxMode::DangerFullAccess) {
        let mut command = Command::new(binary);
        command.arg("rewrite").arg(original_command);
        return Some(command);
    }
    sandboxed_rtk_rewrite_process(
        binary,
        original_command,
        sandbox_mode,
        workspace_roots,
        database_path,
    )
}

#[cfg(target_os = "macos")]
fn sandboxed_rtk_rewrite_process(
    binary: &Path,
    original_command: &str,
    sandbox_mode: SandboxMode,
    workspace_roots: &WorkspaceRoots,
    database_path: &Path,
) -> Option<Command> {
    let state_dir = database_path.parent();
    let policy = match sandbox_mode {
        SandboxMode::ReadOnly => read_only_seatbelt_policy_with_rtk(state_dir),
        SandboxMode::WorkspaceWrite => {
            workspace_write_seatbelt_policy_with_rtk(workspace_roots, state_dir)
        }
        SandboxMode::DangerFullAccess => unreachable!(),
    };
    let mut command = Command::new(SANDBOX_EXEC_PATH);
    command
        .arg("-p")
        .arg(policy)
        .arg("--")
        .arg(binary)
        .arg("rewrite")
        .arg(original_command);
    Some(command)
}

#[cfg(target_os = "linux")]
fn sandboxed_rtk_rewrite_process(
    binary: &Path,
    original_command: &str,
    sandbox_mode: SandboxMode,
    workspace_roots: &WorkspaceRoots,
    database_path: &Path,
) -> Option<Command> {
    let protected_paths = protected_root_paths(workspace_roots);
    let mut command = Command::new(bwrap::BWRAP_PATH);
    command
        .args(bwrap::bubblewrap_args(
            sandbox_mode,
            workspace_roots,
            &protected_paths,
            database_path.parent(),
        ))
        .arg(binary)
        .arg("rewrite")
        .arg(original_command);
    bwrap::harden_command_environment(&mut command);
    Some(command)
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn sandboxed_rtk_rewrite_process(
    _binary: &Path,
    _original_command: &str,
    _sandbox_mode: SandboxMode,
    _workspace_roots: &WorkspaceRoots,
    _database_path: &Path,
) -> Option<Command> {
    None
}

fn shell_single_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn rtk_tracked_shell_command(original_command: &str, login: bool) -> String {
    let shell_mode = if login { "-lc" } else { "-c" };
    format!(
        "rtk proxy -- {BASH_PATH} {shell_mode} {}",
        shell_single_quote(original_command)
    )
}

fn run_shell_command_blocking(
    mut plan: ExecPlan,
    sandbox_mode: SandboxMode,
    workspace_roots: WorkspaceRoots,
    cancel: Arc<AtomicBool>,
    output_callback: OutputCallback,
) -> Result<ExecResult, ExecError> {
    if !matches!(sandbox_mode, SandboxMode::DangerFullAccess) {
        workspace_roots.validate_roots().map_err(ExecError::Spawn)?;
    }
    let rtk_route = match plan.rtk_binary.take() {
        // Rewriting is unsafe when a login shell may rebuild PATH or the user
        // explicitly assigns it. RTK can still losslessly track the command by
        // proxying the original shell after RTK itself has started.
        Some(binary) if plan.login || command_assigns_path(&plan.command) => {
            prepare_active_rtk(binary, plan.rtk_database_path.clone())
                .map(|runtime| RtkRoute::Tracked { runtime })
        }
        Some(binary) => try_rewrite_shell_command_with_rtk(
            &plan.command,
            &plan.cwd,
            binary,
            plan.rtk_database_path.clone(),
            &workspace_roots,
            cancel.as_ref(),
            RtkRewriteProbeOptions {
                sandbox_mode,
                timeout: RTK_REWRITE_TIMEOUT,
            },
        )?
        .log_warning(),
        None => None,
    };
    let rtk_route_kind = rtk_route.as_ref().map(RtkRoute::kind);
    match rtk_route.as_ref() {
        Some(RtkRoute::Rewritten { command, .. }) => plan.command = command.clone(),
        Some(RtkRoute::Tracked { .. }) => {
            plan.command = rtk_tracked_shell_command(&plan.command, plan.login);
            // The wrapper must resolve Halv's pinned `rtk` before the original
            // login shell or PATH assignment can alter its environment.
            plan.login = false;
        }
        None => {}
    }

    // RTK's bounded rewrite probe is optimization overhead, not part of the
    // user's command runtime or timeout budget. The actual shell keeps the
    // exact timeout behavior it had before RTK routing was introduced.
    let start = Instant::now();
    let spawn_description;
    let mut sandbox_tag = None;
    let mut command = match sandbox_mode {
        SandboxMode::DangerFullAccess => {
            spawn_description = format!("shell `{BASH_PATH}`");
            let mut command = Command::new(BASH_PATH);
            command
                .arg(if plan.login { "-lc" } else { "-c" })
                .arg(&plan.command);
            command
        }
        SandboxMode::ReadOnly | SandboxMode::WorkspaceWrite => {
            let tag = SandboxTag::new();
            let sandboxed = sandboxed_shell_command(
                &plan,
                sandbox_mode,
                &workspace_roots,
                rtk_route
                    .as_ref()
                    .map(|route| route.runtime().database_path.as_path()),
                &tag,
            )?;
            sandbox_tag = Some(tag);
            spawn_description = sandboxed.0;
            sandboxed.1
        }
    };
    if let Some(runtime) = rtk_route.as_ref().map(RtkRoute::runtime) {
        command
            .env("PATH", &runtime.path)
            .env(RTK_DB_PATH_ENV, &runtime.database_path)
            .env(RTK_TELEMETRY_DISABLED_ENV, "1");
    }
    command
        .current_dir(&plan.cwd)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    host_env::apply_to_command(&mut command);
    crate::process::hide_console(&mut command);
    configure_process_group(&mut command);

    let mut child = command.spawn().map_err(|error| {
        ExecError::Spawn(format!("failed to spawn {spawn_description}: {error}"))
    })?;
    let process_family = match ProcessFamily::attach(&child, sandbox_tag) {
        Ok(process_family) => process_family,
        Err(error) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(ExecError::Spawn(format!(
                "failed to protect the shell process family: {error}"
            )));
        }
    };
    let captured = Arc::new(Mutex::new(Capture::default()));
    let mut readers = Vec::new();

    if let Some(stdout) = child.stdout.take() {
        readers.push(spawn_reader(
            stdout,
            captured.clone(),
            output_callback.clone(),
        ));
    }
    if let Some(stderr) = child.stderr.take() {
        readers.push(spawn_reader(stderr, captured.clone(), output_callback));
    }

    let timeout = Duration::from_millis(plan.timeout_ms);
    let mut timed_out = false;
    let exit_code = loop {
        if cancel.load(Ordering::SeqCst) {
            process_family.terminate();
            let _ = wait_after_kill(&mut child, CHILD_KILL_GRACE);
            drain_readers(readers, READER_DRAIN_GRACE);
            return Err(ExecError::Cancelled);
        }

        if start.elapsed() >= timeout {
            timed_out = true;
            process_family.terminate();
            let _ = wait_after_kill(&mut child, CHILD_KILL_GRACE);
            break 124;
        }

        match child.try_wait() {
            Ok(Some(status)) => {
                process_family.terminate();
                break exit_code_from_status(status);
            }
            Ok(None) => thread::sleep(Duration::from_millis(25)),
            Err(error) => {
                process_family.terminate();
                let _ = wait_after_kill(&mut child, CHILD_KILL_GRACE);
                drain_readers(readers, READER_DRAIN_GRACE);
                return Err(ExecError::Spawn(format!(
                    "failed to poll shell process: {error}"
                )));
            }
        }
    };

    drain_readers(readers, READER_DRAIN_GRACE);
    let wall_time = start.elapsed();
    let capture = captured.lock().expect("capture lock poisoned");
    let mut bytes = capture.bytes.clone();
    let capture_truncated = capture.truncated || timed_out;
    drop(capture);

    if timed_out {
        bytes.extend_from_slice(b"\nCommand timed out and was killed.\n");
    }

    let (model_output, model_truncated) = format_model_output(exit_code, wall_time, &bytes);
    Ok(ExecResult {
        exit_code,
        timed_out,
        wall_time,
        captured_output: String::from_utf8_lossy(&bytes).to_string(),
        model_output,
        truncated_output: capture_truncated || model_truncated,
        rtk_route: rtk_route_kind,
    })
}

#[cfg(target_os = "macos")]
fn sandboxed_shell_command(
    plan: &ExecPlan,
    sandbox_mode: SandboxMode,
    workspace_roots: &WorkspaceRoots,
    shared_rtk_database: Option<&Path>,
    sandbox_tag: &SandboxTag,
) -> Result<(String, Command), ExecError> {
    let shared_rtk_state_dir = shared_rtk_database.and_then(Path::parent);
    let policy = match sandbox_mode {
        SandboxMode::ReadOnly => read_only_seatbelt_policy_with_rtk(shared_rtk_state_dir),
        SandboxMode::WorkspaceWrite => {
            workspace_write_seatbelt_policy_with_rtk(workspace_roots, shared_rtk_state_dir)
        }
        SandboxMode::DangerFullAccess => unreachable!(),
    };
    let policy = format!("{policy}\n{}", sandbox_tag.seatbelt_rule());
    let mut command = Command::new(SANDBOX_EXEC_PATH);
    command
        .arg("-p")
        .arg(policy)
        .arg("--")
        .arg(BASH_PATH)
        .arg(if plan.login { "-lc" } else { "-c" })
        .arg(&plan.command);
    Ok((format!("sandboxed shell `{SANDBOX_EXEC_PATH}`"), command))
}

#[cfg(target_os = "linux")]
fn sandboxed_shell_command(
    plan: &ExecPlan,
    sandbox_mode: SandboxMode,
    workspace_roots: &WorkspaceRoots,
    shared_rtk_database: Option<&Path>,
    _sandbox_tag: &SandboxTag,
) -> Result<(String, Command), ExecError> {
    let protected_paths = protected_root_paths(workspace_roots);
    let shared_rtk_state_dir = shared_rtk_database.and_then(Path::parent);
    let mut command = Command::new(bwrap::BWRAP_PATH);
    command
        .args(bwrap::bubblewrap_args(
            sandbox_mode,
            workspace_roots,
            &protected_paths,
            shared_rtk_state_dir,
        ))
        .arg(BASH_PATH)
        .arg(if plan.login { "-lc" } else { "-c" })
        .arg(&plan.command);
    bwrap::harden_command_environment(&mut command);
    Ok((format!("sandboxed shell `{}`", bwrap::BWRAP_PATH), command))
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn sandboxed_shell_command(
    _plan: &ExecPlan,
    _sandbox_mode: SandboxMode,
    _workspace_roots: &WorkspaceRoots,
    _shared_rtk_database: Option<&Path>,
    _sandbox_tag: &SandboxTag,
) -> Result<(String, Command), ExecError> {
    Err(ExecError::Spawn(
        "sandbox unavailable on this platform; command not run".to_string(),
    ))
}

#[cfg(unix)]
fn spawn_reader<R: std::os::fd::IntoRawFd + Send + 'static>(
    reader: R,
    captured: Arc<Mutex<Capture>>,
    output_callback: OutputCallback,
) -> ReaderHandle {
    use std::os::fd::FromRawFd;

    let fd = reader.into_raw_fd();
    set_fd_nonblocking(fd);
    let stop = Arc::new(AtomicBool::new(false));
    let stop_for_thread = stop.clone();
    let handle = thread::spawn(move || {
        let mut reader = unsafe { File::from_raw_fd(fd) };
        let mut buffer = [0_u8; 8192];
        while !stop_for_thread.load(Ordering::SeqCst) {
            match reader.read(&mut buffer) {
                Ok(0) => break,
                Ok(count) => {
                    let chunk = &buffer[..count];
                    let keep = captured.lock().expect("capture lock poisoned").push(chunk);
                    if keep > 0 {
                        output_callback.emit(&chunk[..keep]);
                    }
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(10));
                }
                Err(_) => break,
            }
        }
    });
    ReaderHandle { stop, handle }
}

#[cfg(not(unix))]
fn spawn_reader<R: Read + Send + 'static>(
    mut reader: R,
    captured: Arc<Mutex<Capture>>,
    output_callback: OutputCallback,
) -> ReaderHandle {
    let stop = Arc::new(AtomicBool::new(false));
    let stop_for_thread = stop.clone();
    let handle = thread::spawn(move || {
        let mut buffer = [0_u8; 8192];
        while !stop_for_thread.load(Ordering::SeqCst) {
            match reader.read(&mut buffer) {
                Ok(0) => break,
                Ok(count) => {
                    let chunk = &buffer[..count];
                    let keep = captured.lock().expect("capture lock poisoned").push(chunk);
                    if keep > 0 {
                        output_callback.emit(&chunk[..keep]);
                    }
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(_) => break,
            }
        }
    });
    ReaderHandle { stop, handle }
}

fn drain_readers(readers: Vec<ReaderHandle>, grace: Duration) {
    let deadline = Instant::now() + grace;
    while Instant::now() < deadline && readers.iter().any(|reader| !reader.handle.is_finished()) {
        thread::sleep(Duration::from_millis(10));
    }

    for reader in &readers {
        reader.stop.store(true, Ordering::SeqCst);
    }

    let stop_deadline = Instant::now() + Duration::from_millis(50);
    while Instant::now() < stop_deadline
        && readers.iter().any(|reader| !reader.handle.is_finished())
    {
        thread::sleep(Duration::from_millis(5));
    }

    for reader in readers {
        if reader.handle.is_finished() {
            let _ = reader.handle.join();
        }
    }
}

#[cfg(unix)]
fn set_fd_nonblocking(fd: std::os::fd::RawFd) {
    unsafe {
        let flags = libc::fcntl(fd, libc::F_GETFL);
        if flags >= 0 {
            let _ = libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK);
        }
    }
}

#[cfg(unix)]
fn configure_process_group(command: &mut Command) {
    use std::os::unix::process::CommandExt;
    unsafe {
        command.pre_exec(|| {
            if libc::setpgid(0, 0) == 0 {
                Ok(())
            } else {
                Err(io::Error::last_os_error())
            }
        });
    }
}

#[cfg(not(unix))]
fn configure_process_group(_command: &mut Command) {}

/// Stands in for the macOS Seatbelt tag. On Linux, bubblewrap ends the PID
/// namespace of the command with its parent. Thus no other platform needs a
/// tag.
#[cfg(not(target_os = "macos"))]
struct SandboxTag;

#[cfg(not(target_os = "macos"))]
impl SandboxTag {
    fn new() -> Self {
        Self
    }
}

struct ProcessFamily {
    #[cfg(unix)]
    child_id: u32,
    #[cfg(target_os = "macos")]
    tree: Option<process_tree::ProcessTree>,
    #[cfg(windows)]
    job: std::os::windows::io::OwnedHandle,
}

impl ProcessFamily {
    /// Attaches cleanup to a spawned child. A `sandbox_tag` marks a
    /// sandboxed command. On macOS, cleanup then also follows the processes
    /// that leave the process tree of the child.
    fn attach(child: &Child, sandbox_tag: Option<SandboxTag>) -> io::Result<Self> {
        #[cfg(not(target_os = "macos"))]
        let _ = sandbox_tag;

        #[cfg(windows)]
        {
            use std::{
                mem,
                os::windows::io::{AsRawHandle, FromRawHandle},
            };
            use windows_sys::Win32::System::JobObjects::{
                AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
                SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
                JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
            };

            let raw_job = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
            if raw_job.is_null() {
                return Err(io::Error::last_os_error());
            }
            let job = unsafe { std::os::windows::io::OwnedHandle::from_raw_handle(raw_job) };
            let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let configured = unsafe {
                SetInformationJobObject(
                    job.as_raw_handle(),
                    JobObjectExtendedLimitInformation,
                    &limits as *const _ as *const std::ffi::c_void,
                    mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                )
            };
            if configured == 0 {
                return Err(io::Error::last_os_error());
            }
            let assigned =
                unsafe { AssignProcessToJobObject(job.as_raw_handle(), child.as_raw_handle()) };
            if assigned == 0 {
                return Err(io::Error::last_os_error());
            }
            return Ok(Self { job });
        }

        #[cfg(target_os = "macos")]
        return Ok(Self {
            child_id: child.id(),
            tree: sandbox_tag
                .map(|tag| process_tree::ProcessTree::track(child.id(), tag))
                .transpose()?,
        });

        #[cfg(all(unix, not(target_os = "macos")))]
        return Ok(Self {
            child_id: child.id(),
        });

        #[cfg(not(any(unix, windows)))]
        Ok(Self {})
    }

    fn terminate(&self) {
        #[cfg(target_os = "macos")]
        if let Some(tree) = &self.tree {
            tree.terminate();
        }

        #[cfg(unix)]
        terminate_unix_process_family(self.child_id);

        #[cfg(windows)]
        {
            use std::os::windows::io::AsRawHandle;
            use windows_sys::Win32::System::JobObjects::TerminateJobObject;

            unsafe {
                let _ = TerminateJobObject(self.job.as_raw_handle(), 1);
            }
        }
    }
}

pub(crate) struct BoundedProcessOutput {
    pub status: ExitStatus,
    pub output: Vec<u8>,
    pub truncated: bool,
}

pub(crate) fn run_process_with_limits(
    mut command: Command,
    timeout: Duration,
    output_limit_bytes: usize,
) -> Result<BoundedProcessOutput, String> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    host_env::apply_to_command(&mut command);
    crate::process::hide_console(&mut command);
    configure_process_group(&mut command);
    let mut child = command
        .spawn()
        .map_err(|error| format!("could not start: {error}"))?;
    let process_family = match ProcessFamily::attach(&child, None) {
        Ok(process_family) => process_family,
        Err(error) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!("could not establish subprocess cleanup: {error}"));
        }
    };
    let captured = Arc::new(Mutex::new(Capture::with_limit(output_limit_bytes)));
    let callback = OutputCallback::new(|_| {});
    let mut readers = Vec::new();
    if let Some(stdout) = child.stdout.take() {
        readers.push(spawn_reader(stdout, captured.clone(), callback.clone()));
    }
    if let Some(stderr) = child.stderr.take() {
        readers.push(spawn_reader(stderr, captured.clone(), callback));
    }

    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                process_family.terminate();
                break status;
            }
            Ok(None) if started.elapsed() < timeout => {
                thread::sleep(Duration::from_millis(25));
            }
            Ok(None) => {
                process_family.terminate();
                let _ = wait_after_kill(&mut child, CHILD_KILL_GRACE);
                drain_readers(readers, READER_DRAIN_GRACE);
                return Err(format!(
                    "timed out after {} seconds and was terminated",
                    timeout.as_secs_f64()
                ));
            }
            Err(error) => {
                process_family.terminate();
                let _ = wait_after_kill(&mut child, CHILD_KILL_GRACE);
                drain_readers(readers, READER_DRAIN_GRACE);
                return Err(format!("could not be polled: {error}"));
            }
        }
    };

    drain_readers(readers, READER_DRAIN_GRACE);
    let captured = captured.lock().expect("capture lock poisoned");
    Ok(BoundedProcessOutput {
        status,
        output: captured.bytes.clone(),
        truncated: captured.truncated,
    })
}

#[cfg(unix)]
fn terminate_unix_process_family(child_id: u32) {
    let descendants = collect_descendant_pids(child_id as i32);
    unsafe {
        for pid in descendants.iter().rev().copied() {
            let _ = libc::kill(-pid, libc::SIGKILL);
            let _ = libc::kill(pid, libc::SIGKILL);
        }
        let root = child_id as i32;
        let _ = libc::kill(-root, libc::SIGKILL);
        let _ = libc::kill(root, libc::SIGKILL);
    }
}

#[cfg(unix)]
fn collect_descendant_pids(root_pid: i32) -> Vec<i32> {
    let mut seen = HashSet::new();
    let mut stack = vec![root_pid];
    let mut descendants = Vec::new();

    while let Some(parent) = stack.pop() {
        for child in direct_child_pids(parent) {
            if child > 0 && seen.insert(child) {
                descendants.push(child);
                stack.push(child);
            }
        }
    }

    descendants
}

#[cfg(unix)]
fn direct_child_pids(parent_pid: i32) -> Vec<i32> {
    let mut command = Command::new("/usr/bin/pgrep");
    command.arg("-P").arg(parent_pid.to_string());
    host_env::apply_to_command(&mut command);
    let output = command.output();
    let Ok(output) = output else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| line.trim().parse::<i32>().ok())
        .collect()
}

fn wait_after_kill(child: &mut Child, grace: Duration) -> Option<std::process::ExitStatus> {
    let deadline = Instant::now() + grace;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Some(status),
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(10)),
            Ok(None) | Err(_) => return None,
        }
    }
}

#[cfg(unix)]
fn exit_code_from_status(status: std::process::ExitStatus) -> i32 {
    use std::os::unix::process::ExitStatusExt;
    status
        .code()
        .or_else(|| status.signal().map(|signal| 128 + signal))
        .unwrap_or(1)
}

#[cfg(not(unix))]
fn exit_code_from_status(status: std::process::ExitStatus) -> i32 {
    status.code().unwrap_or(1)
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum SandboxPlatform {
    MacosSeatbelt,
    LinuxBubblewrap,
    Unavailable(String),
}

fn sandbox_platform() -> SandboxPlatform {
    sandbox_platform_for(
        std::env::consts::OS,
        || std::path::Path::new(SANDBOX_EXEC_PATH).is_file(),
        || command_available_on_path(bwrap::BWRAP_PATH),
    )
}

fn sandbox_platform_for(
    os: &str,
    macos_seatbelt_available: impl FnOnce() -> bool,
    linux_bwrap_available: impl FnOnce() -> bool,
) -> SandboxPlatform {
    match os {
        "macos" => {
            if macos_seatbelt_available() {
                SandboxPlatform::MacosSeatbelt
            } else {
                SandboxPlatform::Unavailable(
                    "macOS Seatbelt sandbox is unavailable; command not run".to_string(),
                )
            }
        }
        "linux" => {
            if linux_bwrap_available() {
                SandboxPlatform::LinuxBubblewrap
            } else {
                SandboxPlatform::Unavailable(
                    "Linux bubblewrap sandbox is unavailable; install bubblewrap with your distro's package manager; command not run".to_string(),
                )
            }
        }
        _ => SandboxPlatform::Unavailable(
            "sandbox unavailable on this platform; command not run".to_string(),
        ),
    }
}

fn command_available_on_path(command_name: &str) -> bool {
    let command_path = Path::new(command_name);
    if command_path.components().count() > 1 {
        return command_path.is_file();
    }

    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&path).any(|dir| dir.join(command_name).is_file())
}

fn platform_unavailable_result(command: &str, message: String) -> ExecResult {
    let body = format!("{message}\n\nCommand was not run:\n{command}");
    let (model_output, truncated_output) = format_model_output(1, Duration::ZERO, body.as_bytes());
    ExecResult {
        exit_code: 1,
        timed_out: false,
        wall_time: Duration::ZERO,
        captured_output: body,
        model_output,
        truncated_output,
        rtk_route: None,
    }
}

fn count_lines(text: &str) -> usize {
    if text.is_empty() {
        0
    } else {
        text.lines().count()
    }
}

fn middle_truncate_utf8(text: &str, limit_bytes: usize) -> (String, bool) {
    if text.len() <= limit_bytes {
        return (text.to_string(), false);
    }
    const MARKER: &str = "\n[... middle truncated ...]\n";
    let marker_len = MARKER.len().min(limit_bytes);
    let remaining = limit_bytes.saturating_sub(marker_len);
    let head_target = remaining / 2;
    let tail_target = remaining - head_target;
    let head_end = floor_char_boundary(text, head_target);
    let tail_start = ceil_char_boundary(text, text.len().saturating_sub(tail_target));
    let mut output = String::with_capacity(limit_bytes);
    output.push_str(&text[..head_end]);
    output.push_str(MARKER);
    output.push_str(&text[tail_start..]);
    (output, true)
}

fn floor_char_boundary(text: &str, index: usize) -> usize {
    let mut index = index.min(text.len());
    while index > 0 && !text.is_char_boundary(index) {
        index -= 1;
    }
    index
}

fn ceil_char_boundary(text: &str, index: usize) -> usize {
    let mut index = index.min(text.len());
    while index < text.len() && !text.is_char_boundary(index) {
        index += 1;
    }
    index
}

fn command_is_provably_read_only(command: &str) -> bool {
    let Some(words) = simple_shell_words(command) else {
        return false;
    };
    let Some((program, arguments)) = words.split_first() else {
        return false;
    };

    match program.as_str() {
        "pwd" | "ls" | "cat" | "head" | "tail" | "wc" | "stat" | "du" | "df" | "uname"
        | "whoami" | "id" | "basename" | "dirname" | "readlink" | "realpath" | "md5sum"
        | "sha1sum" | "sha256sum" | "shasum" | "cmp" | "diff" | "grep" | "echo" | "printf"
        | "true" | "false" => true,
        "rg" => !arguments.iter().any(|argument| {
            matches!(argument.as_str(), "--pre" | "--hostname-bin")
                || argument.starts_with("--pre=")
                || argument.starts_with("--hostname-bin=")
        }),
        "git" => git_command_is_provably_read_only(arguments),
        _ => false,
    }
}

fn git_command_is_provably_read_only(arguments: &[String]) -> bool {
    let Some((subcommand, subcommand_arguments)) = arguments.split_first() else {
        return false;
    };
    if !matches!(
        subcommand.as_str(),
        "status"
            | "diff"
            | "log"
            | "show"
            | "rev-parse"
            | "ls-files"
            | "ls-tree"
            | "grep"
            | "blame"
            | "describe"
            | "name-rev"
            | "shortlog"
            | "whatchanged"
    ) {
        return false;
    }

    !subcommand_arguments.iter().any(|argument| {
        matches!(
            argument.as_str(),
            "--ext-diff" | "--textconv" | "--output" | "--open-files-in-pager" | "--show-signature"
        ) || argument.starts_with("--output=")
            || argument.starts_with("--open-files-in-pager=")
            || (subcommand == "grep" && argument.starts_with("-O"))
    })
}

fn simple_shell_words(command: &str) -> Option<Vec<String>> {
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Quote {
        None,
        Single,
        Double,
    }

    let mut quote = Quote::None;
    let mut escaped = false;
    let mut current = String::new();
    let mut words = Vec::new();
    let mut redirect_target = false;
    let mut characters = command.chars().peekable();

    while let Some(character) = characters.next() {
        if character == '\n' || character == '\r' {
            return None;
        }
        if escaped {
            current.push(character);
            escaped = false;
            continue;
        }

        match quote {
            Quote::Single => {
                if character == '\'' {
                    quote = Quote::None;
                } else {
                    current.push(character);
                }
            }
            Quote::Double => match character {
                '"' => quote = Quote::None,
                '\\' => escaped = true,
                '$' | '`' => return None,
                _ => current.push(character),
            },
            Quote::None => match character {
                '\'' => quote = Quote::Single,
                '"' => quote = Quote::Double,
                '\\' => escaped = true,
                '>' if !redirect_target => {
                    if !current.is_empty() {
                        if current.chars().all(|value| value.is_ascii_digit()) {
                            current.clear();
                        } else {
                            words.push(std::mem::take(&mut current));
                        }
                    }
                    if characters.peek() == Some(&'>') {
                        characters.next();
                    }
                    redirect_target = true;
                }
                '&' if redirect_target && current.is_empty() => current.push(character),
                '$' | '`' | ';' | '|' | '&' | '<' | '>' | '(' | ')' | '*' | '?' | '[' | ']'
                | '{' | '}' => return None,
                value if value.is_whitespace() => {
                    if !current.is_empty() {
                        if redirect_target {
                            if !target_is_allowed_device_or_fd(&current) {
                                return None;
                            }
                            current.clear();
                            redirect_target = false;
                        } else {
                            words.push(std::mem::take(&mut current));
                        }
                    }
                }
                _ => current.push(character),
            },
        }
    }

    if escaped || quote != Quote::None {
        return None;
    }
    if redirect_target {
        if !target_is_allowed_device_or_fd(&current) {
            return None;
        }
    } else if !current.is_empty() {
        words.push(current);
    }
    Some(words)
}

fn target_is_allowed_device_or_fd(target: &str) -> bool {
    let target = target.trim();
    if target.is_empty() {
        return false;
    }
    if let Some(fd) = target.strip_prefix('&') {
        return matches!(fd, "1" | "2");
    }
    if let Some(fd) = target.strip_prefix("/dev/fd/") {
        return matches!(fd, "1" | "2");
    }
    matches!(
        target,
        "/dev/null"
            | "/dev/zero"
            | "/dev/dtracehelper"
            | "/dev/tty"
            | "/dev/random"
            | "/dev/urandom"
            | "/dev/stdin"
            | "/dev/stdout"
            | "/dev/stderr"
    )
}

pub fn prepare_first_write(cwd: &Path, slug_basis: &str) -> FirstWriteAction {
    let mut status = git_status(cwd);
    if !status.is_git_repo {
        status.warning = Some(
            "This folder is not a git repository. Writes can proceed only after confirmation, without an automatic branch safety net.".to_string(),
        );
        return FirstWriteAction::RequiresConfirmation(status);
    }

    if status.branch.is_none() {
        status.warning = Some(
            "The repository is in detached HEAD state. Halv will not write without confirmation because no branch ref would protect the changes.".to_string(),
        );
        return FirstWriteAction::RequiresConfirmation(status);
    }

    if status.clean == Some(false) {
        status.warning = Some(
            "The working tree is dirty. Halv will not auto-branch over existing changes; confirm to proceed in the current worktree.".to_string(),
        );
        return FirstWriteAction::RequiresConfirmation(status);
    }

    let on_default = status
        .branch
        .as_ref()
        .zip(status.default_branch.as_ref())
        .is_some_and(|(branch, default)| branch == default);
    if status.clean == Some(true) && on_default {
        let slug = branch_slug(slug_basis);
        let branch = unique_branch_name(cwd, &format!("{AUTO_BRANCH_PREFIX}{slug}"));
        let output = managed_git_command(cwd).and_then(|mut command| {
            command
                .arg("switch")
                .arg("-c")
                .arg(&branch)
                .output()
                .map_err(|error| format!("failed to run git switch: {error}"))
        });
        match output {
            Ok(output) if output.status.success() => {
                status.branch = Some(branch.clone());
                status.auto_created_branch = Some(branch);
            }
            Ok(output) => {
                status.warning = Some(format!(
                    "Failed to create an automatic branch: {}",
                    String::from_utf8_lossy(&output.stderr).trim()
                ));
                return FirstWriteAction::RequiresConfirmation(status);
            }
            Err(error) => {
                status.warning = Some(error);
                return FirstWriteAction::RequiresConfirmation(status);
            }
        }
    }

    FirstWriteAction::Proceed(status)
}

pub fn git_status(cwd: &Path) -> BranchStatus {
    let is_repo = git_output(cwd, ["rev-parse", "--is-inside-work-tree"])
        .is_some_and(|output| output.trim() == "true");
    if !is_repo {
        return BranchStatus {
            is_git_repo: false,
            branch: None,
            default_branch: None,
            clean: None,
            auto_created_branch: None,
            warning: None,
        };
    }

    let branch = git_output(cwd, ["branch", "--show-current"])
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    let default_branch = default_branch(cwd);
    let clean = git_output(cwd, ["status", "--porcelain", "--ignore-submodules=dirty"])
        .map(|value| value.trim().is_empty())
        .or(Some(false));
    BranchStatus {
        is_git_repo: true,
        branch,
        default_branch,
        clean,
        auto_created_branch: None,
        warning: None,
    }
}

pub fn create_draft_pr(cwd: &Path) -> Result<String, String> {
    let mut command = Command::new(GH_PATH);
    command.current_dir(cwd);
    harden_git_environment(&mut command)?;
    crate::process::hide_console(&mut command);
    let output = command
        .args(["pr", "create", "--draft", "--fill"])
        .output()
        .map_err(|error| format!("failed to run gh pr create: {error}"))?;
    if output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        Ok(if stdout.is_empty() {
            "Draft PR created.".to_string()
        } else {
            stdout
        })
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        Err(if stderr.is_empty() {
            format!("gh pr create failed with status {}", output.status)
        } else {
            stderr
        })
    }
}

fn default_branch(cwd: &Path) -> Option<String> {
    if let Some(origin_head) = git_output(
        cwd,
        [
            "symbolic-ref",
            "--quiet",
            "--short",
            "refs/remotes/origin/HEAD",
        ],
    ) {
        if let Some((_, branch)) = origin_head.trim().split_once('/') {
            return Some(branch.to_string());
        }
    }
    for candidate in ["main", "master"] {
        if git_output(cwd, ["rev-parse", "--verify", candidate]).is_some() {
            return Some(candidate.to_string());
        }
    }
    None
}

fn git_output<const N: usize>(cwd: &Path, args: [&str; N]) -> Option<String> {
    let output = managed_git_command(cwd).ok()?.args(args).output().ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).to_string())
}

fn unique_branch_name(cwd: &Path, base: &str) -> String {
    if !branch_exists(cwd, base) {
        return base.to_string();
    }
    for suffix in 2..1000 {
        let candidate = format!("{base}-{suffix}");
        if !branch_exists(cwd, &candidate) {
            return candidate;
        }
    }
    format!("{base}-{}", storage_safe_timestamp())
}

fn branch_exists(cwd: &Path, branch: &str) -> bool {
    let Ok(mut command) = managed_git_command(cwd) else {
        return false;
    };
    command
        .args(["show-ref", "--verify", "--quiet"])
        .arg(format!("refs/heads/{branch}"))
        .status()
        .is_ok_and(|status| status.success())
}

pub(crate) fn managed_git_command(cwd: &Path) -> Result<Command, String> {
    let mut command = Command::new(GIT_PATH);
    command.current_dir(cwd);
    harden_git_environment(&mut command)?;
    crate::process::hide_console(&mut command);
    Ok(command)
}

/// Prepares a git or gh command to run in a repository that Halv does not
/// trust. Set the working directory first, because the repository config
/// there decides which program keys this function overrides.
///
/// The overrides come only from the config of that repository. Thus git must
/// not start git in a submodule, because the submodule config can name other
/// programs. Give `--ignore-submodules=dirty` to each automatic status or
/// diff, and do not give a submodule path to `git add`.
pub(crate) fn harden_git_environment(command: &mut Command) -> Result<(), String> {
    scrub_git_environment(command)?;
    distrust_repository_commands(command).map(|_| ())
}

/// Prepares a git command for a repository that a running agent can write,
/// and puts it in the read-only sandbox.
///
/// The agent can change the config between the config query and the git
/// run. For example, an include path can name a FIFO that gives a different
/// config to each reader. Then a program from the repository config can run
/// in spite of the overrides. In the sandbox, that program cannot write files
/// or use the network. The function returns an error when this platform has
/// no sandbox.
pub(crate) fn sandboxed_git_command<I, S>(cwd: &Path, args: I) -> Result<Command, String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<std::ffi::OsStr>,
{
    let mut command = Command::new(sandboxed_git_program());
    command.current_dir(cwd);
    scrub_git_environment(&mut command)?;
    let entries = distrust_repository_commands(&mut command)?;
    // A filter cannot write files in the sandbox, thus git-lfs fails there.
    // If a filter is optional, git reads the file without the filter. If a
    // filter is required, git stops.
    add_command_config(&mut command, optional_filter_entries(&entries));
    command.args(args);
    let mut sandboxed = read_only_sandboxed_command(&command)?;
    crate::process::hide_console(&mut sandboxed);
    Ok(sandboxed)
}

/// Returns the git program for the sandbox.
///
/// On macOS, /usr/bin/git can be a launcher that writes a cache file and then
/// starts the real git. In the sandbox, the launcher cannot write its cache.
/// Thus it is slow, and it writes warnings that look like status lines. The
/// real git is in the git exec path, so the sandbox starts that program.
fn sandboxed_git_program() -> PathBuf {
    static PROGRAM: OnceLock<PathBuf> = OnceLock::new();
    PROGRAM
        .get_or_init(|| {
            let mut query = Command::new(GIT_PATH);
            // Git writes the exec path and stops before it looks for a
            // repository, thus it reads no repository config.
            query
                .arg("--exec-path")
                .current_dir(std::env::temp_dir())
                .env_remove("GIT_EXEC_PATH")
                .stdin(Stdio::null())
                .stderr(Stdio::null());
            host_env::apply_to_command(&mut query);
            crate::process::hide_console(&mut query);
            query
                .output()
                .ok()
                .filter(|output| output.status.success())
                .and_then(|output| String::from_utf8(output.stdout).ok())
                .map(|directory| PathBuf::from(directory.trim()).join(GIT_PATH))
                .filter(|program| program.is_absolute() && program.is_file())
                .unwrap_or_else(|| PathBuf::from(GIT_PATH))
        })
        .clone()
}

/// Copies a prepared command into the read-only sandbox. The sandboxed
/// command can read files. It cannot write files or use the network.
fn read_only_sandboxed_command(command: &Command) -> Result<Command, String> {
    if let SandboxPlatform::Unavailable(message) = sandbox_platform() {
        return Err(message);
    }
    let mut sandboxed = read_only_sandbox_launcher(command.get_current_dir())?;
    sandboxed
        .arg(command.get_program())
        .args(command.get_args());
    for (key, value) in command.get_envs() {
        match value {
            Some(value) => sandboxed.env(key, value),
            None => sandboxed.env_remove(key),
        };
    }
    if let Some(directory) = command.get_current_dir() {
        sandboxed.current_dir(directory);
    }
    #[cfg(target_os = "linux")]
    bwrap::harden_command_environment(&mut sandboxed);
    Ok(sandboxed)
}

#[cfg(target_os = "macos")]
fn read_only_sandbox_launcher(_directory: Option<&Path>) -> Result<Command, String> {
    let mut launcher = Command::new(SANDBOX_EXEC_PATH);
    launcher
        .arg("-p")
        .arg(read_only_seatbelt_policy())
        .arg("--");
    Ok(launcher)
}

#[cfg(target_os = "linux")]
fn read_only_sandbox_launcher(directory: Option<&Path>) -> Result<Command, String> {
    let mut launcher = Command::new(bwrap::BWRAP_PATH);
    launcher.args(bwrap::bubblewrap_args(
        SandboxMode::ReadOnly,
        &WorkspaceRoots::new(Vec::new()),
        &[],
        None,
    ));
    // The read-only policy puts empty file systems on /tmp, /var/tmp, and
    // /run. Bind the working directory again, read-only, because it can be
    // below one of them.
    if let Some(directory) = directory.filter(|directory| directory.is_absolute()) {
        launcher.arg("--ro-bind").arg(directory).arg(directory);
    }
    Ok(launcher)
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn read_only_sandbox_launcher(_directory: Option<&Path>) -> Result<Command, String> {
    Err("sandbox unavailable on this platform; command not run".to_string())
}

fn scrub_git_environment(command: &mut Command) -> Result<(), String> {
    let disabled_hooks = std::env::current_exe()
        .map_err(|error| format!("failed to resolve executable for Git hardening: {error}"))?
        .join(".halv-disabled-hooks")
        .to_string_lossy()
        .to_string();

    for key in [
        "GIT_ALTERNATE_OBJECT_DIRECTORIES",
        "GIT_ASKPASS",
        "GIT_ATTR_NOSYSTEM",
        "GIT_COMMON_DIR",
        "GIT_CONFIG_COUNT",
        "GIT_CONFIG_GLOBAL",
        "GIT_CONFIG_NOSYSTEM",
        "GIT_CONFIG_PARAMETERS",
        "GIT_CONFIG_SYSTEM",
        "GIT_DIFF_OPTS",
        "GIT_DIR",
        "GIT_EDITOR",
        "GIT_EXEC_PATH",
        "GIT_EXTERNAL_DIFF",
        "GIT_INDEX_FILE",
        "GIT_OBJECT_DIRECTORY",
        "GIT_PROXY_COMMAND",
        "GIT_SEQUENCE_EDITOR",
        "GIT_SSH",
        "GIT_SSH_COMMAND",
        "GIT_TEMPLATE_DIR",
        "GIT_TRACE",
        "GIT_TRACE2",
        "GIT_TRACE_PACKET",
        "GIT_WORK_TREE",
        "SSH_ASKPASS",
    ] {
        command.env_remove(key);
    }
    for (key, _) in std::env::vars_os() {
        let key_text = key.to_string_lossy();
        if key_text.starts_with("GIT_CONFIG_KEY_") || key_text.starts_with("GIT_CONFIG_VALUE_") {
            command.env_remove(key);
        }
    }

    let config = [
        ("core.hooksPath", disabled_hooks.as_str()),
        ("core.fsmonitor", "false"),
        ("core.pager", "cat"),
        ("gc.auto", "0"),
        ("protocol.ext.allow", "never"),
        // A git process in a submodule reads the submodule config, which
        // the overrides do not cover.
        ("submodule.recurse", "false"),
        // With diff.submodule=diff, git diff and git show start git diff in
        // each changed submodule. With the short format, git writes only
        // the two commit IDs of the submodule.
        ("diff.submodule", "short"),
        // A long-format status starts git submodule summary in each
        // changed submodule.
        ("status.submoduleSummary", "false"),
    ];
    command.env("GIT_CONFIG_COUNT", config.len().to_string());
    for (index, (key, value)) in config.into_iter().enumerate() {
        command.env(format!("GIT_CONFIG_KEY_{index}"), key);
        command.env(format!("GIT_CONFIG_VALUE_{index}"), value);
    }
    command
        .env("GIT_EDITOR", "true")
        .env("GIT_SEQUENCE_EDITOR", "true")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_PAGER", "cat")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("PAGER", "cat");
    #[cfg(test)]
    TEST_GIT_ENVIRONMENT.with(|entries| {
        for (key, value) in entries.borrow().iter() {
            command.env(key, value);
        }
    });
    // Git starts filters and other programs in the repository. There, a
    // relative PATH entry can find a program that the repository supplies,
    // for example its own git-lfs. Thus keep only the absolute entries.
    let path = command
        .get_envs()
        .find(|(key, _)| *key == "PATH")
        .map(|(_, value)| value.map(std::ffi::OsStr::to_os_string))
        .unwrap_or_else(|| std::env::var_os("PATH"));
    if let Some(path) = path {
        command.env("PATH", host_env::absolute_path_entries(&path));
    }
    host_env::apply_to_command(command);
    Ok(())
}

#[cfg(test)]
thread_local! {
    /// Environment entries that a test adds to each git command that Halv
    /// prepares on this thread.
    static TEST_GIT_ENVIRONMENT: std::cell::RefCell<Vec<(OsString, OsString)>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

/// Adds environment entries to each git command that Halv prepares on this
/// thread until the guard drops.
#[cfg(test)]
pub(crate) struct TestGitEnvironment;

#[cfg(test)]
impl TestGitEnvironment {
    pub(crate) fn set(entries: Vec<(OsString, OsString)>) -> Self {
        TEST_GIT_ENVIRONMENT.with(|current| *current.borrow_mut() = entries);
        Self
    }
}

#[cfg(test)]
impl Drop for TestGitEnvironment {
    fn drop(&mut self) {
        TEST_GIT_ENVIRONMENT.with(|current| current.borrow_mut().clear());
    }
}

/// Stops the repository config from choosing the programs that git runs.
///
/// Git reads command-scope config last, so the entries that this function
/// adds win over `.git/config`, `config.worktree`, and the files that they
/// include. Each entry resets a program key and then repeats its system and
/// global values. Thus a global git-lfs filter or credential helper still
/// runs, but a filter or helper from an untrusted repository does not. The
/// one exception is a repository `lfs` filter with the standard git-lfs
/// values (see `repository_command_overrides`). The repository config also
/// cannot choose the directory where git-lfs writes (`lfs.storage`). When
/// the repository config can hide keys from Halv, no filter runs, also a
/// global filter.
///
/// Returns the config entries that the query read.
fn distrust_repository_commands(command: &mut Command) -> Result<Vec<GitConfigEntry>, String> {
    let mut query = Command::new(GIT_PATH);
    for (key, value) in command.get_envs() {
        match value {
            Some(value) => query.env(key, value),
            None => query.env_remove(key),
        };
    }
    if let Some(directory) = command.get_current_dir() {
        query.current_dir(directory);
    }
    query.args(["config", "--list", "--show-scope", "-z"]);
    let entries = parse_git_config_listing(&read_git_config_listing(query)?);

    if entries
        .iter()
        .any(|entry| !entry.is_trusted() && entry.key == "core.gitproxy")
    {
        // Git uses the first core.gitProxy match, so a command-scope entry
        // cannot replace it. An empty variable disables the proxy.
        command.env("GIT_PROXY_COMMAND", "");
    }
    add_command_config(command, repository_command_overrides(&entries));
    Ok(entries)
}

/// Adds command-scope config entries after the entries that the command
/// already has.
pub(crate) fn add_command_config(command: &mut Command, entries: Vec<(String, String)>) {
    if entries.is_empty() {
        return;
    }
    let first = command
        .get_envs()
        .find(|(key, _)| *key == "GIT_CONFIG_COUNT")
        .and_then(|(_, value)| value?.to_str()?.parse::<usize>().ok())
        .unwrap_or(0);
    command.env("GIT_CONFIG_COUNT", (first + entries.len()).to_string());
    for (offset, (key, value)) in entries.into_iter().enumerate() {
        command.env(format!("GIT_CONFIG_KEY_{}", first + offset), key);
        command.env(format!("GIT_CONFIG_VALUE_{}", first + offset), value);
    }
}

/// Returns entries that set `filter.<driver>.required` to false for each
/// filter driver that the config names.
fn optional_filter_entries(entries: &[GitConfigEntry]) -> Vec<(String, String)> {
    let mut drivers = Vec::<&str>::new();
    for entry in entries {
        let Some((section, rest)) = entry.key.split_once('.') else {
            continue;
        };
        let Some((driver, _)) = rest.rsplit_once('.') else {
            continue;
        };
        if section.eq_ignore_ascii_case("filter") && !drivers.contains(&driver) {
            drivers.push(driver);
        }
    }
    drivers
        .into_iter()
        .map(|driver| (format!("filter.{driver}.required"), "false".to_string()))
        .collect()
}

const GIT_CONFIG_READ_TIMEOUT: Duration = Duration::from_secs(5);
const GIT_CONFIG_READ_LIMIT_BYTES: usize = 4 * 1024 * 1024;

/// Runs `git config --list` with a time limit. An include path that names a
/// FIFO must not stop the caller.
fn read_git_config_listing(mut query: Command) -> Result<Vec<u8>, String> {
    query
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    crate::process::hide_console(&mut query);
    let mut child = query
        .spawn()
        .map_err(|error| format!("failed to read Git config: {error}"))?;
    let (Some(stdout), Some(stderr)) = (child.stdout.take(), child.stderr.take()) else {
        let _ = child.kill();
        let _ = child.wait();
        return Err("failed to read Git config: the output pipes are missing".to_string());
    };
    let stdout = spawn_git_config_reader(stdout);
    let stderr = spawn_git_config_reader(stderr);

    let started = Instant::now();
    let mut delay = Duration::from_millis(1);
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() < GIT_CONFIG_READ_TIMEOUT => {
                thread::sleep(delay);
                delay = (delay * 2).min(Duration::from_millis(25));
            }
            Ok(None) | Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("failed to read Git config: git config did not finish".to_string());
            }
        }
    };
    let output_error = |_| "failed to read Git config output".to_string();
    let stdout = stdout.join().map_err(output_error)?;
    let stderr = stderr.join().map_err(output_error)?;
    if !status.success() {
        return Err(format!(
            "failed to read Git config: {}",
            String::from_utf8_lossy(&stderr).trim()
        ));
    }
    if stdout.len() > GIT_CONFIG_READ_LIMIT_BYTES {
        return Err("failed to read Git config: the config is too large".to_string());
    }
    Ok(stdout)
}

/// Reads at most the config size limit from a pipe, then drains the rest so
/// that git does not block on a full pipe.
fn spawn_git_config_reader(mut pipe: impl Read + Send + 'static) -> thread::JoinHandle<Vec<u8>> {
    thread::spawn(move || {
        let mut bytes = Vec::new();
        let limit = GIT_CONFIG_READ_LIMIT_BYTES as u64 + 1;
        let _ = (&mut pipe).take(limit).read_to_end(&mut bytes);
        let _ = io::copy(&mut pipe, &mut io::sink());
        bytes
    })
}

#[derive(Debug)]
struct GitConfigEntry {
    scope: String,
    key: String,
    value: String,
}

impl GitConfigEntry {
    /// The user controls the system and global config. Halv sets the
    /// command scope itself. The repository supplies the other scopes.
    fn is_trusted(&self) -> bool {
        matches!(self.scope.as_str(), "system" | "global" | "command")
    }
}

/// Parses the `scope NUL key LF value NUL` records of
/// `git config --list --show-scope -z`.
fn parse_git_config_listing(listing: &[u8]) -> Vec<GitConfigEntry> {
    let mut fields = listing.split(|byte| *byte == 0);
    let mut entries = Vec::new();
    while let (Some(scope), Some(item)) = (fields.next(), fields.next()) {
        let item = String::from_utf8_lossy(item);
        let (key, value) = item.split_once('\n').unwrap_or((&item, ""));
        entries.push(GitConfigEntry {
            scope: String::from_utf8_lossy(scope).into_owned(),
            key: key.to_string(),
            value: value.to_string(),
        });
    }
    entries
}

/// Returns the command-scope entries that replace each program key that an
/// untrusted scope sets. The entries reset the key to the git default and
/// then repeat the system and global values in their original order.
///
/// The `lfs` filter driver is the only exception. Its entries stay when each
/// one has a value that `git lfs install --local` writes, with or without
/// `--skip-smudge`.
///
/// When the repository config can hide keys from Halv (see
/// `repository_config_hides_lfs_keys`), the `lfs` filter does not stay.
/// Then each filter program of each scope also resets to empty, and no
/// trusted value follows. Thus git starts no filter, and git-lfs cannot
/// read the hidden keys.
fn repository_command_overrides(entries: &[GitConfigEntry]) -> Vec<(String, String)> {
    let hidden_lfs_keys = repository_config_hides_lfs_keys(entries);
    let keep_lfs_filter = !hidden_lfs_keys && repository_lfs_filter_is_standard(entries);
    let mut resets = Vec::new();
    for entry in entries.iter().filter(|entry| !entry.is_trusted()) {
        if keep_lfs_filter && lfs_filter_variable(&entry.key).is_some() {
            continue;
        }
        if let Some(reset) = program_key_reset(&entry.key) {
            if !resets.contains(&reset) {
                resets.push(reset);
            }
        }
    }
    if hidden_lfs_keys {
        for entry in entries {
            let reset = (entry.key.clone(), "");
            if is_filter_program_key(&entry.key) && !resets.contains(&reset) {
                resets.push(reset);
            }
        }
    }
    let mut overrides = Vec::new();
    for (reset_key, default) in resets {
        let trusted = entries
            .iter()
            .filter(|entry| {
                matches!(entry.scope.as_str(), "system" | "global")
                    && !(hidden_lfs_keys && is_filter_program_key(&entry.key))
                    && program_key_reset(&entry.key).is_some_and(|(key, _)| key == reset_key)
            })
            .map(|entry| (entry.key.clone(), entry.value.clone()))
            .collect::<Vec<_>>();
        overrides.push((reset_key, default.to_string()));
        overrides.extend(trusted);
    }
    overrides
}

/// Reports whether git-lfs can read a key from an untrusted scope that Halv
/// does not see.
///
/// Halv reads each NUL record of `git config --list -z` as one entry.
/// git-lfs reads `git config -l` line by line, and it splits each line at
/// the first `=`. Thus each line of a value with a newline can become one
/// more git-lfs key. A key with a `=` (in a subsection name) can also become
/// a different git-lfs key. Halv cannot reset these keys, because it does
/// not know their names.
fn repository_config_hides_lfs_keys(entries: &[GitConfigEntry]) -> bool {
    entries
        .iter()
        .filter(|entry| !entry.is_trusted())
        .any(|entry| entry.value.contains('\n') || entry.key.contains('='))
}

/// Reports whether a key is `filter.<driver>.clean`, `smudge`, or `process`.
fn is_filter_program_key(key: &str) -> bool {
    let Some((section, rest)) = key.split_once('.') else {
        return false;
    };
    let Some((_, name)) = rest.rsplit_once('.') else {
        return false;
    };
    section.eq_ignore_ascii_case("filter")
        && ["clean", "smudge", "process"]
            .iter()
            .any(|program| name.eq_ignore_ascii_case(program))
}

/// The values that `git lfs install --local` writes for the `lfs` filter
/// driver, also with `--skip-smudge`. These values start only the git-lfs
/// program from PATH.
const STANDARD_LFS_FILTER_VALUES: [(&str, &str); 7] = [
    ("clean", "git-lfs clean -- %f"),
    ("smudge", "git-lfs smudge -- %f"),
    ("smudge", "git-lfs smudge --skip -- %f"),
    ("process", "git-lfs filter-process"),
    ("process", "git-lfs filter-process --skip"),
    ("required", "true"),
    ("required", "false"),
];

/// Returns the variable name of a `filter.lfs.<name>` key, or None for other
/// keys. Git uses the exact case of a driver name, thus `filter.LFS` is a
/// different driver.
fn lfs_filter_variable(key: &str) -> Option<&str> {
    let (section, rest) = key.split_once('.')?;
    let (driver, name) = rest.rsplit_once('.')?;
    (section.eq_ignore_ascii_case("filter") && driver == "lfs").then_some(name)
}

/// Reports whether each `filter.lfs.*` entry from an untrusted scope has a
/// standard value. The check compares the full strings, thus a value that
/// only starts with a standard command fails. A variable that git-lfs does
/// not write also fails.
fn repository_lfs_filter_is_standard(entries: &[GitConfigEntry]) -> bool {
    entries
        .iter()
        .filter(|entry| !entry.is_trusted())
        .filter_map(|entry| Some((lfs_filter_variable(&entry.key)?, entry.value.as_str())))
        .all(|(name, value)| {
            STANDARD_LFS_FILTER_VALUES
                .iter()
                .any(|(standard_name, standard_value)| {
                    name.eq_ignore_ascii_case(standard_name) && value == *standard_value
                })
        })
}

/// Returns the key and the value that reset a config key that names a
/// program, or None for other keys. Related keys share one reset key. An
/// empty `credential.helper` clears every helper, including URL-scoped
/// helpers. `gpg.program` also sets the OpenPGP program.
///
/// git-lfs reads its settings through `git config`, thus the resets also
/// apply to a git-lfs filter. git-lfs ignores the case of keys. It also
/// finds a transfer agent program in each key that contains
/// `lfs.customtransfer.<name>.path`, also when the key has another section.
/// Thus each key that contains `customtransfer` gets a reset.
///
/// `lfs.storage` does not name a program. But git-lfs writes its objects in
/// that directory, thus the key also gets a reset. git-lfs uses the last
/// value, and an empty value gives the default directory, `.git/lfs`.
fn program_key_reset(key: &str) -> Option<(String, &'static str)> {
    if key.to_ascii_lowercase().contains("customtransfer") {
        return Some((key.to_string(), ""));
    }
    let (section, rest) = key.split_once('.')?;
    let (subsection, name) = match rest.rsplit_once('.') {
        Some((subsection, name)) => (Some(subsection), name),
        None => (None, rest),
    };
    let section = section.to_ascii_lowercase();
    let name = name.to_ascii_lowercase();
    let default = match (section.as_str(), subsection, name.as_str()) {
        ("credential", _, "helper") => return Some(("credential.helper".to_string(), "")),
        ("gpg", None | Some("openpgp"), "program") => {
            return Some(("gpg.program".to_string(), "gpg"))
        }
        ("gpg", Some("x509"), "program") => "gpgsm",
        ("gpg", Some("ssh"), "program") => "ssh-keygen",
        ("core", None, "sshcommand") => "ssh",
        ("remote", Some(_), "uploadpack") => "git-upload-pack",
        ("remote", Some(_), "receivepack") => "git-receive-pack",
        ("filter", Some(_), "clean" | "smudge" | "process")
        | ("diff", Some(_), "textconv" | "command")
        | ("diff", None, "external")
        | ("merge", Some(_), "driver")
        | ("core", None, "askpass" | "alternaterefscommand")
        | ("gpg", Some("ssh"), "defaultkeycommand")
        | ("lfs", None, "storage") => "",
        // A git-lfs extension program runs on each clean and smudge.
        ("lfs", Some(subsection), "clean" | "smudge")
            if subsection.to_ascii_lowercase().starts_with("extension.") =>
        {
            ""
        }
        _ => return None,
    };
    Some((key.to_string(), default))
}

fn branch_slug(input: &str) -> String {
    let mut slug = String::new();
    let mut last_dash = false;
    for ch in input.chars().flat_map(char::to_lowercase) {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch);
            last_dash = false;
        } else if !last_dash && !slug.is_empty() {
            slug.push('-');
            last_dash = true;
        }
        if slug.len() >= 48 {
            break;
        }
    }
    let slug = slug.trim_matches('-');
    if slug.is_empty() {
        "agent-write".to_string()
    } else {
        slug.to_string()
    }
}

fn storage_safe_timestamp() -> String {
    chrono::Utc::now().format("%Y%m%d%H%M%S").to_string()
}

/// Writes an executable test script that creates `sentinel` and then runs
/// `body`.
#[cfg(all(test, unix))]
pub(crate) fn write_sentinel_program(script: &Path, sentinel: &Path, body: &str) {
    use std::os::unix::fs::PermissionsExt;

    fs::write(
        script,
        format!("#!/bin/sh\n: > '{}'\n{body}\n", sentinel.display()),
    )
    .expect("sentinel program should write");
    fs::set_permissions(script, fs::Permissions::from_mode(0o755))
        .expect("sentinel program should become executable");
}

/// Makes the local config of a test repository name a program that creates
/// `sentinel`. The config uses the program as a filter and as a textconv
/// driver, directly for `*.txt` files and through an included file for
/// `*.md` files. The function also changes the time stamps of the tracked
/// files, so that git must hash them again.
#[cfg(all(test, unix))]
pub(crate) fn plant_repository_programs(repo: &Path, sentinel: &Path) {
    let git_dir = repo.join(".git");
    let script = git_dir.join("halv-test-program");
    write_sentinel_program(
        &script,
        sentinel,
        r#"if [ -f "$1" ]; then cat "$1"; else cat; fi"#,
    );
    let program = format!("'{}'", script.display());
    let included = git_dir.join("halv-test-include");
    fs::write(
        &included,
        format!("[filter \"included\"]\n\tclean = {program}\n[diff \"included\"]\n\ttextconv = {program}\n"),
    )
    .expect("included config should write");
    let config_path = git_dir.join("config");
    let mut config = fs::read_to_string(&config_path).expect("local config should read");
    config.push_str(&format!(
        "[filter \"evil\"]\n\tclean = {program}\n\tsmudge = {program}\n[diff \"evil\"]\n\ttextconv = {program}\n[include]\n\tpath = {}\n",
        included.display()
    ));
    fs::write(&config_path, config).expect("local config should write");
    fs::write(
        repo.join(".gitattributes"),
        "*.txt filter=evil diff=evil\n*.md filter=included diff=included\n",
    )
    .expect("attributes should write");

    let tracked = Command::new(GIT_PATH)
        .args(["ls-files", "-z"])
        .current_dir(repo)
        .output()
        .expect("git ls-files should run");
    for path in tracked.stdout.split(|byte| *byte == 0) {
        if path.is_empty() {
            continue;
        }
        fs::File::options()
            .write(true)
            .open(repo.join(String::from_utf8_lossy(path).as_ref()))
            .and_then(|file| {
                file.set_modified(std::time::UNIX_EPOCH + Duration::from_secs(1_000_000_000))
            })
            .expect("tracked file time stamp should change");
    }
}

/// Runs plain git for a test fixture and returns its standard output.
#[cfg(all(test, unix))]
fn run_fixture_git(cwd: &Path, args: &[&str]) -> String {
    let output = Command::new(GIT_PATH)
        .args(args)
        .current_dir(cwd)
        .output()
        .expect("git should run");
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("git output should use UTF-8")
}

/// Adds a populated submodule `sub` to a test repository and commits it.
/// The submodule has a user name and email. Returns the submodule path and
/// the submodule git directory.
#[cfg(all(test, unix))]
fn add_test_submodule(repo: &Path) -> (PathBuf, PathBuf) {
    let source = repo.join(".git/halv-submodule-source");
    fs::create_dir_all(&source).expect("submodule source should create");
    run_fixture_git(&source, &["init", "-q", "-b", "main"]);
    run_fixture_git(&source, &["config", "user.email", "test@example.com"]);
    run_fixture_git(&source, &["config", "user.name", "Halv Test"]);
    fs::write(source.join("inner.txt"), "inner\n").expect("submodule file should write");
    run_fixture_git(&source, &["add", "inner.txt"]);
    run_fixture_git(&source, &["commit", "-q", "-m", "inner"]);

    let source = source.to_string_lossy().into_owned();
    run_fixture_git(
        repo,
        &[
            "-c",
            "protocol.file.allow=always",
            "submodule",
            "add",
            "-q",
            &source,
            "sub",
        ],
    );
    run_fixture_git(repo, &["commit", "-q", "-m", "add submodule"]);

    let submodule = repo.join("sub");
    run_fixture_git(&submodule, &["config", "user.email", "test@example.com"]);
    run_fixture_git(&submodule, &["config", "user.name", "Halv Test"]);
    let git_dir =
        PathBuf::from(run_fixture_git(&submodule, &["rev-parse", "--absolute-git-dir"]).trim());
    (submodule, git_dir)
}

/// Adds a populated submodule `sub` to a test repository and commits it. The
/// submodule config and its info/attributes name a filter that creates
/// `sentinel`. The function also changes the time stamp of the submodule
/// file, so that a git status in the submodule must run the filter. The
/// repository must have a user name and email. Returns the submodule path.
#[cfg(all(test, unix))]
pub(crate) fn plant_submodule_program(repo: &Path, sentinel: &Path) -> PathBuf {
    let (submodule, git_dir) = add_test_submodule(repo);
    let script = git_dir.join("halv-test-program");
    write_sentinel_program(&script, sentinel, "cat");
    let program = format!("'{}'", script.display());
    run_fixture_git(&submodule, &["config", "filter.halvsub.clean", &program]);
    fs::create_dir_all(git_dir.join("info")).expect("submodule info should create");
    fs::write(git_dir.join("info/attributes"), "* filter=halvsub\n")
        .expect("submodule attributes should write");
    fs::File::options()
        .write(true)
        .open(submodule.join("inner.txt"))
        .and_then(|file| {
            file.set_modified(std::time::UNIX_EPOCH + Duration::from_secs(1_000_000_000))
        })
        .expect("submodule file time stamp should change");
    submodule
}

/// Adds a populated submodule `sub` to a test repository and commits it. The
/// submodule HEAD then moves one commit past the gitlink. The submodule
/// config names a program that creates `sentinel`, as diff.external and as
/// a textconv driver for each file. The superproject config sets
/// diff.submodule to diff. Thus a diff of the gitlink starts git diff in the
/// submodule, and that process runs the program. The repository must have a
/// user name and email. Returns the submodule path.
#[cfg(all(test, unix))]
pub(crate) fn plant_submodule_diff_program(repo: &Path, sentinel: &Path) -> PathBuf {
    let (submodule, git_dir) = add_test_submodule(repo);
    fs::write(submodule.join("inner.txt"), "inner changed\n")
        .expect("submodule file should change");
    run_fixture_git(&submodule, &["commit", "-q", "-am", "inner changed"]);

    let script = git_dir.join("halv-test-diff-program");
    write_sentinel_program(&script, sentinel, r#"cat "$1" 2>/dev/null; exit 0"#);
    let program = format!("'{}'", script.display());
    run_fixture_git(&submodule, &["config", "diff.external", &program]);
    run_fixture_git(&submodule, &["config", "diff.halvsub.textconv", &program]);
    fs::create_dir_all(git_dir.join("info")).expect("submodule info should create");
    fs::write(git_dir.join("info/attributes"), "* diff=halvsub\n")
        .expect("submodule attributes should write");
    run_fixture_git(repo, &["config", "diff.submodule", "diff"]);
    submodule
}

/// The script of the fake git-lfs program. It adds each subcommand to the log
/// at `@LOG@`. Its clean filter reads a file and writes `FakeGitLfs::POINTER`
/// in place of the file. The `filter-process` subcommand uses the
/// long-running filter protocol of git and supports only the clean command.
/// When `lfs.storage` has a value, the program makes the `objects` directory
/// in that directory, as git-lfs does. Like git-lfs, the program reads the
/// value from the lines of `git config -l`, and the last line wins.
#[cfg(all(test, unix))]
const FAKE_GIT_LFS_SCRIPT: &str = r#"#!/bin/sh
printf '%s\n' "$1" >> @LOG@
storage=$(git config -l 2>/dev/null | sed -n 's/^lfs\.storage=//p' | tail -n 1)
[ -z "$storage" ] || mkdir -p "$storage/objects" 2>/dev/null
case $1 in
clean)
	cat > /dev/null
	printf 'fake-lfs pointer\n'
	exit 0
	;;
smudge)
	cat
	exit 0
	;;
filter-process) ;;
*) exit 1 ;;
esac

# Reads one packet into $packet. Returns 1 for a flush packet. Stops the
# program when git closes the pipe.
read_packet() {
	size=$(dd bs=1 count=4 2>/dev/null)
	[ -n "$size" ] || exit 0
	[ "$size" != 0000 ] || return 1
	packet=
	[ "$size" = 0004 ] || packet=$(dd bs=1 count=$((0x$size - 4)) 2>/dev/null)
	return 0
}

write_packet() {
	printf '%04x%s\n' $((${#1} + 5)) "$1"
}

while read_packet; do :; done
write_packet git-filter-server
write_packet version=2
printf 0000
while read_packet; do :; done
write_packet capability=clean
printf 0000
while :; do
	command=
	while read_packet; do
		case $packet in command=*) command=${packet#command=} ;; esac
	done
	while read_packet; do :; done
	if [ "$command" = clean ]; then
		write_packet status=success
		printf 0000
		write_packet 'fake-lfs pointer'
		printf 0000
		printf 0000
	else
		write_packet status=error
		printf 0000
	fi
done
"#;

/// A fake git-lfs program for tests, and a git environment that finds it on
/// PATH. The environment also uses an empty file as the global config and
/// turns off the system config, so that the git-lfs setup of the user does
/// not change a test.
#[cfg(all(test, unix))]
pub(crate) struct FakeGitLfs {
    log: PathBuf,
    environment: Vec<(OsString, OsString)>,
}

#[cfg(all(test, unix))]
impl FakeGitLfs {
    /// The text that the fake clean filter writes in place of each file.
    pub(crate) const POINTER: &'static str = "fake-lfs pointer\n";

    /// Writes the program, its log, and the empty global config in
    /// `directory`.
    pub(crate) fn new(directory: &Path) -> Self {
        use std::os::unix::fs::PermissionsExt;

        let bin = directory.join("fake-git-lfs-bin");
        fs::create_dir_all(&bin).expect("fake git-lfs directory should create");
        let log = directory.join("fake-git-lfs.log");
        let program = bin.join("git-lfs");
        fs::write(
            &program,
            FAKE_GIT_LFS_SCRIPT.replace("@LOG@", &format!("'{}'", log.display())),
        )
        .expect("fake git-lfs should write");
        fs::set_permissions(&program, fs::Permissions::from_mode(0o755))
            .expect("fake git-lfs should become executable");
        let global = directory.join("fake-git-lfs-global.gitconfig");
        fs::write(&global, "").expect("empty global config should write");

        let mut path = vec![bin];
        if let Some(current) = std::env::var_os("PATH") {
            path.extend(std::env::split_paths(&current));
        }
        let path = std::env::join_paths(path).expect("PATH should join");
        Self {
            log,
            environment: vec![
                ("PATH".into(), path),
                ("GIT_CONFIG_GLOBAL".into(), global.into_os_string()),
                ("GIT_CONFIG_NOSYSTEM".into(), "1".into()),
            ],
        }
    }

    /// Returns the environment entries.
    pub(crate) fn environment(&self) -> Vec<(OsString, OsString)> {
        self.environment.clone()
    }

    /// Uses the environment for each git command that Halv prepares on this
    /// thread until the guard drops.
    pub(crate) fn activate(&self) -> TestGitEnvironment {
        TestGitEnvironment::set(self.environment())
    }

    /// Runs plain git with the environment and returns its standard output.
    pub(crate) fn git(&self, cwd: &Path, args: &[&str]) -> String {
        let output = Command::new(GIT_PATH)
            .args(args)
            .current_dir(cwd)
            .envs(self.environment())
            .output()
            .expect("git should run");
        assert!(
            output.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).into_owned()
    }

    /// Returns the subcommands that the program ran.
    pub(crate) fn runs(&self) -> Vec<String> {
        fs::read_to_string(&self.log)
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }
}

/// Gives a test repository the `lfs` filter config that
/// `git lfs install --local` writes. Then info/attributes assigns the filter
/// to the files that match `pattern`.
#[cfg(all(test, unix))]
pub(crate) fn plant_standard_lfs_filter(repo: &Path, pattern: &str) {
    for (key, value) in [
        ("lfs.repositoryformatversion", "0"),
        ("filter.lfs.clean", "git-lfs clean -- %f"),
        ("filter.lfs.smudge", "git-lfs smudge -- %f"),
        ("filter.lfs.process", "git-lfs filter-process"),
        ("filter.lfs.required", "true"),
    ] {
        run_fixture_git(repo, &["config", key, value]);
    }
    let info = repo.join(".git/info");
    fs::create_dir_all(&info).expect("info directory should create");
    let attributes = info.join("attributes");
    let mut content = fs::read_to_string(&attributes).unwrap_or_default();
    content.push_str(&format!("{pattern} filter=lfs diff=lfs merge=lfs -text\n"));
    fs::write(&attributes, content).expect("attributes should write");
}

/// Sets an old time stamp on a file, so that git must read the file again
/// to compare it with the index.
#[cfg(all(test, unix))]
fn make_stat_dirty(path: &Path) {
    fs::File::options()
        .write(true)
        .open(path)
        .and_then(|file| {
            file.set_modified(std::time::UNIX_EPOCH + Duration::from_secs(1_000_000_000))
        })
        .expect("time stamp should change");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::new_uuid_v4;
    use serde_json::json;
    use std::sync::Arc;

    #[test]
    fn crux_ledger_sums_saved_tokens_and_skips_torn_lines() {
        let dir = std::env::temp_dir().join(format!("halv-crux-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("test dir");
        let path = dir.join("ledger.jsonl");
        std::fs::write(
            &path,
            "{\"ts\":1,\"tool\":\"scip_map\",\"response_tokens\":100,\"saved_tokens\":80}\n{broken\n{\"saved_tokens\":8}\n",
        )
        .expect("write ledger");
        assert_eq!(crux_tokens_saved_at(&path), Some(88));
        assert_eq!(crux_tokens_saved_at(&dir.join("missing.jsonl")), Some(0));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rtk_state_directory_is_stable_and_scoped_to_app_data() {
        let root = std::env::temp_dir().join(format!("halv-rtk-path-test-{}", new_uuid_v4()));
        let first_app_data = root.join("first-app-data");
        let second_app_data = root.join("second-app-data");
        std::fs::create_dir_all(&first_app_data).expect("first app data directory should create");
        std::fs::create_dir_all(&second_app_data).expect("second app data directory should create");

        let first_state = OnceLock::new();
        let second_state = OnceLock::new();
        let first = initialize_rtk_state_directory_in(&first_state, &first_app_data, &root, 501);
        let repeated = initialize_rtk_state_directory_in(&first_state, &first_app_data, &root, 501);
        let second = initialize_rtk_state_directory_in(&second_state, &second_app_data, &root, 501);

        assert_eq!(first, repeated);
        assert_ne!(first, second);
        let directory_name = first
            .file_name()
            .and_then(|name| name.to_str())
            .expect("RTK state directory should have a UTF-8 name");
        let hash = directory_name
            .strip_prefix("halv-rtk-501-")
            .expect("RTK state directory should include the user ID");
        assert_eq!(hash.len(), 8);
        assert!(hash.bytes().all(|byte| byte.is_ascii_hexdigit()));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn stale_rtk_cleanup_preserves_current_and_referenced_directories() {
        let root = std::env::temp_dir().join(format!("halv-rtk-cleanup-test-{}", new_uuid_v4()));
        let current = root.join("halv-rtk-101");
        let referenced = root.join("halv-rtk-102");
        let dead = root.join("halv-rtk-103");
        let legacy_referenced = root.join("halv-rtk-104");
        let stable = root.join("halv-rtk-501-deadbeef");
        for directory in [&current, &referenced, &dead, &legacy_referenced, &stable] {
            std::fs::create_dir_all(directory).expect("RTK state fixture should create");
        }
        let referenced_db = referenced.join("session-explicit.db");
        std::fs::write(&referenced_db, []).expect("referenced RTK database should write");
        std::fs::write(legacy_referenced.join("session-legacy.db"), [])
            .expect("legacy referenced RTK database should write");
        let references = [
            RunningRtkSessionReference {
                id: "stored-row".to_string(),
                rtk_db_path: Some(referenced_db),
                crux_ledger_path: None,
            },
            RunningRtkSessionReference {
                id: "legacy".to_string(),
                rtk_db_path: None,
                crux_ledger_path: None,
            },
        ];

        let removed = cleanup_stale_rtk_directories_in(&root, &current, &references, |_| false);

        assert_eq!(removed, 1);
        assert!(current.is_dir());
        assert!(referenced.is_dir());
        assert!(legacy_referenced.is_dir());
        assert!(!dead.exists());
        assert!(stable.is_dir());
        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[test]
    fn startup_rtk_cleanup_does_not_follow_a_linked_state_directory() {
        use std::os::unix::fs::symlink;

        let root = std::env::temp_dir().join(format!("halv-rtk-linked-state-{}", new_uuid_v4()));
        // The Seatbelt policies let a sandboxed command remove the state
        // directory and put a link to any directory at its path.
        let outside = root.join("outside");
        let outside_files = ["bin", "agent-tools"]
            .map(|name| outside.join(name).join("0123456789abcdef").join("tool"));
        for file in &outside_files {
            std::fs::create_dir_all(file.parent().expect("outside file should have a parent"))
                .expect("outside directory should create");
            std::fs::write(file, "").expect("outside file should write");
        }
        let temp_dir = root.join("tmp");
        std::fs::create_dir_all(&temp_dir).expect("temp directory should create");
        let state_dir = temp_dir.join("halv-rtk-501-deadbeef");
        symlink(&outside, &state_dir).expect("state directory link should create");
        // A link with the name of a stale directory also stays. The PID does
        // not fit in a pid_t, so the cleanup takes the process as stopped.
        let stale_link = temp_dir.join(format!("halv-rtk-{}", u32::MAX));
        symlink(&outside, &stale_link).expect("stale directory link should create");

        spawn_stale_rtk_state_cleanup_in(temp_dir, state_dir.clone(), Vec::new())
            .expect("cleanup thread should start")
            .join()
            .expect("cleanup thread should finish");

        for file in &outside_files {
            assert!(file.is_file(), "`{}` should stay", file.display());
        }
        for link in [&state_dir, &stale_link] {
            assert!(std::fs::symlink_metadata(link)
                .expect("link should stay")
                .file_type()
                .is_symlink());
        }
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn shell_tool_json_schema_serializes_to_codex_shape() {
        assert_eq!(
            shell_command_tool_schema(),
            json!({
                "type": "function",
                "name": "shell_command",
                "description": "Runs a shell command and returns its output.",
                "strict": false,
                "parameters": {
                    "type": "object",
                    "properties": {
                        "command": {
                            "type": "string",
                            "description": "The shell command to execute as a shell script."
                        },
                        "workdir": {
                            "type": "string",
                            "description": "The working directory for the command. Defaults to the conversation cwd or project root."
                        },
                        "timeout_ms": {
                            "type": "integer",
                            "description": "The command timeout in milliseconds.",
                            "default": 10000,
                            "minimum": 1,
                            "maximum": 600000
                        },
                        "login": {
                            "type": "boolean",
                            "description": "Run the shell as a login shell.",
                            "default": false
                        },
                        "sandbox_permissions": {
                            "type": "string",
                            "description": "Requested sandbox permissions for this command.",
                            "enum": ["use_default", "with_additional_permissions", "require_escalated"]
                        },
                        "justification": {
                            "type": "string",
                            "description": "User-facing justification shown when approval is required."
                        },
                        "prefix_rule": {
                            "type": "array",
                            "description": "Reusable approval prefix for this command.",
                            "items": { "type": "string" }
                        },
                        "additional_permissions": {
                            "type": "array",
                            "description": "Additional sandbox permissions requested by the model.",
                            "items": { "type": "string" }
                        }
                    },
                    "required": ["command"]
                }
            })
        );
    }

    #[test]
    fn rtk_tokens_saved_reads_the_exact_database_total() {
        let root = std::env::temp_dir().join(format!("halv-rtk-savings-{}", new_uuid_v4()));
        std::fs::create_dir_all(&root).expect("RTK fixture directory should create");
        let database_path = root.join("history.db");

        assert_eq!(rtk_tokens_saved_at(&database_path), Some(0));

        let connection =
            rusqlite::Connection::open(&database_path).expect("RTK fixture database should open");
        assert_eq!(rtk_tokens_saved_at(&database_path), Some(0));
        connection
            .execute_batch(
                "CREATE TABLE commands (saved_tokens INTEGER NOT NULL);\
                 INSERT INTO commands (saved_tokens) VALUES (17), (0), (25);",
            )
            .expect("RTK fixture rows should insert");

        assert_eq!(rtk_tokens_saved_at(&database_path), Some(42));
        drop(connection);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn gui_turn_rtk_paths_are_distinct_and_filename_safe() {
        let first = rtk_gui_turn_database_path("turn/a");
        let second = rtk_gui_turn_database_path("turn-b");

        assert_ne!(first, second);
        assert_eq!(
            first.file_name().and_then(|value| value.to_str()),
            Some("gui-turn-turn_a.db")
        );
        assert_eq!(
            second.file_name().and_then(|value| value.to_str()),
            Some("gui-turn-turn-b.db")
        );
    }

    #[test]
    fn read_only_seatbelt_policy_is_deny_default_read_only_no_network() {
        let policy = read_only_seatbelt_policy();
        assert!(policy.contains("(deny default)"));
        assert!(policy.contains("(allow file-read*)"));
        assert!(policy.contains("(deny file-write*)"));
        assert!(policy.contains("(deny network*)"));
        assert!(policy.contains("(allow file-write-data"));
        assert!(policy.contains("(path \"/dev/null\")"));
        assert!(policy.contains("(subpath \"/dev/fd\")"));
        assert!(!policy.contains("(allow file-write* (subpath"));
        assert!(!policy.contains("(allow file-write*"));
        assert!(!policy.contains("(allow file-write-data (subpath"));
        assert!(!policy.contains("(allow network"));
    }

    #[test]
    fn seatbelt_policies_allow_only_the_shared_rtk_state_directory() {
        let state_dir = Path::new("/private/tmp/halv-rtk-501-1234abcd");
        let expected_subpath =
            "(allow file-write* (subpath \"/private/tmp/halv-rtk-501-1234abcd\"))";
        let expected_path = "(allow file-write* (path \"/private/tmp/halv-rtk-501-1234abcd\"))";

        let read_only = read_only_seatbelt_policy_with_rtk(Some(state_dir));
        assert!(read_only.contains(expected_subpath));
        assert!(read_only.contains(expected_path));
        assert!(!read_only.contains("/private/tmp/halv-rtk-neighbor"));

        let roots = WorkspaceRoots::new(vec![PathBuf::from("/workspace")]);
        let workspace = workspace_write_seatbelt_policy_with_rtk(&roots, Some(state_dir));
        assert!(workspace.contains(expected_subpath));
        assert!(workspace.contains(expected_path));
        assert!(!workspace.contains("/private/tmp/halv-rtk-neighbor"));
    }

    // An unsandboxed process runs the launcher first on PATH. A sandboxed
    // command must not replace it.
    #[cfg(target_os = "macos")]
    #[test]
    fn read_only_sandbox_cannot_replace_the_rtk_launcher() {
        let root = std::env::temp_dir().join(format!("halv-rtk-launcher-{}", new_uuid_v4()));
        let rtk = write_fake_rtk(&root.join("bundle/rtk-pinned"), "#!/bin/sh\nexit 0\n");
        let rtk = rtk.canonicalize().expect("fake rtk should resolve");
        let state_dir = root.join("state");
        std::fs::create_dir_all(&state_dir).expect("rtk state dir should create");
        let state_dir = state_dir
            .canonicalize()
            .expect("rtk state dir should resolve");

        let runtime = prepare_active_rtk(rtk.clone(), state_dir.join("history.db"))
            .expect("RTK runtime should prepare");
        let launcher = std::env::split_paths(&runtime.path)
            .next()
            .expect("PATH should start with the launcher directory")
            .join(rtk_binary_name());
        assert_eq!(launcher.canonicalize().ok(), Some(rtk.clone()));

        let output = Command::new(SANDBOX_EXEC_PATH)
            .arg("-p")
            .arg(read_only_seatbelt_policy_with_rtk(Some(&state_dir)))
            .arg("--")
            .arg("/bin/ln")
            .arg("-sfn")
            .arg("/usr/bin/true")
            .arg(&launcher)
            .output()
            .expect("sandbox-exec should start");
        if String::from_utf8_lossy(&output.stderr)
            .contains("sandbox_apply: Operation not permitted")
        {
            eprintln!(
                "sandbox-exec is blocked by the outer test environment; launcher assertion skipped"
            );
            let _ = std::fs::remove_dir_all(root);
            return;
        }

        let replaced = output.status.success();
        let target = launcher.canonicalize().ok();
        let _ = std::fs::remove_dir_all(root);
        assert!(
            !replaced,
            "a sandboxed command replaced the launcher `{}`",
            launcher.display()
        );
        assert_eq!(target, Some(rtk));
    }

    #[cfg(unix)]
    fn platform_app_data_dir() -> PathBuf {
        let home = PathBuf::from(std::env::var_os("HOME").expect("HOME should be set"));
        if cfg!(target_os = "macos") {
            home.join("Library/Application Support/ai.halv.app")
        } else {
            home.join(".local/share/ai.halv.app")
        }
    }

    // Resolves the symbolic links of the deepest existing ancestor, then adds
    // the missing components. Seatbelt and resolve_write_path use this form.
    #[cfg(unix)]
    fn canonical_path(path: &Path) -> PathBuf {
        for ancestor in path.ancestors() {
            if let Ok(mut canonical) = ancestor.canonicalize() {
                let rest = path
                    .strip_prefix(ancestor)
                    .expect("an ancestor should be a prefix of the path");
                canonical.extend(rest.components());
                return canonical;
            }
        }
        panic!("`{}` should have an existing ancestor", path.display());
    }

    #[cfg(unix)]
    #[test]
    fn sandbox_write_policies_refuse_the_launcher_root() {
        let project = std::env::temp_dir().join(format!("halv-launcher-project-{}", new_uuid_v4()));
        std::fs::create_dir_all(&project).expect("project directory should create");
        let roots = WorkspaceRoots::for_cwd(&project);
        let app_data_dir = platform_app_data_dir();
        let state_dir = rtk_state_directory_for(
            &app_data_dir,
            &canonical_temp_directory(),
            current_user_id(),
        );
        let launcher = canonical_path(
            &launcher_root_for(&app_data_dir)
                .join("0123456789abcdef")
                .join(rtk_binary_name()),
        );

        assert!(!launcher.starts_with(&state_dir));
        // A resolve error is not a refusal, so the path must resolve.
        let resolved = roots
            .resolve_write_path(&launcher)
            .unwrap_or_else(|error| panic!("`{}` should resolve: {error}", launcher.display()));
        assert!(
            !resolved.is_writable,
            "the workspace roots must not write `{}`",
            launcher.display()
        );

        for policy in [
            read_only_seatbelt_policy_with_rtk(Some(&state_dir)),
            workspace_write_seatbelt_policy_with_rtk(&roots, Some(&state_dir)),
        ] {
            let writable = policy
                .lines()
                .filter_map(|line| {
                    line.strip_prefix("(allow file-write* (subpath \"")
                        .or_else(|| line.strip_prefix("(allow file-write* (path \""))?
                        .strip_suffix("\"))")
                        .map(PathBuf::from)
                })
                .collect::<Vec<_>>();
            assert!(writable.contains(&state_dir));
            assert!(
                writable.iter().all(|path| !launcher.starts_with(path)),
                "the Seatbelt policy must not write `{}`",
                launcher.display()
            );
        }

        // bubblewrap binds the RTK state directory only when it exists.
        let bound_state_dir = std::env::temp_dir().join(format!("halv-rtk-bind-{}", new_uuid_v4()));
        std::fs::create_dir_all(&bound_state_dir).expect("rtk state dir should create");
        for mode in [SandboxMode::ReadOnly, SandboxMode::WorkspaceWrite] {
            let args = bwrap::bubblewrap_args(mode, &roots, &[], Some(&bound_state_dir));
            let writable = args
                .windows(3)
                .filter(|window| window[0] == "--bind")
                .map(|window| PathBuf::from(&window[2]))
                .collect::<Vec<_>>();
            assert!(writable.contains(&bound_state_dir));
            assert!(
                writable.iter().all(|path| !launcher.starts_with(path)),
                "bubblewrap must not write `{}`",
                launcher.display()
            );
        }
        let _ = std::fs::remove_dir_all(bound_state_dir);
        let _ = std::fs::remove_dir_all(project);
    }

    #[cfg(unix)]
    #[test]
    fn launcher_directories_are_private_and_the_launcher_is_repaired() {
        use std::os::unix::fs::{symlink, PermissionsExt};

        let root = std::env::temp_dir().join(format!("halv-launcher-repair-{}", new_uuid_v4()));
        let rtk = write_fake_rtk(&root.join("bundle/rtk-pinned"), "#!/bin/sh\nexit 0\n")
            .canonicalize()
            .expect("fake rtk should resolve");
        let launcher_root = root.join("launchers");
        let name = std::ffi::OsStr::new(rtk_binary_name());

        let launcher_dir = prepare_named_binary_launcher(&rtk, name, &launcher_root)
            .expect("launcher should prepare");
        assert_eq!(launcher_dir.parent(), Some(launcher_root.as_path()));
        for directory in [&launcher_root, &launcher_dir] {
            let mode = std::fs::metadata(directory)
                .expect("launcher directory should have metadata")
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o700, "`{}`", directory.display());
        }
        let launcher = launcher_dir.join(name);
        assert_eq!(std::fs::read_link(&launcher).ok(), Some(rtk.clone()));

        // A launcher that links to another binary links to the binary again.
        let other = write_fake_rtk(&root.join("other/rtk"), "#!/bin/sh\nexit 1\n");
        std::fs::remove_file(&launcher).expect("launcher should remove");
        symlink(&other, &launcher).expect("replacement should link");
        assert_eq!(
            prepare_named_binary_launcher(&rtk, name, &launcher_root),
            Some(launcher_dir.clone())
        );
        assert_eq!(std::fs::read_link(&launcher).ok(), Some(rtk.clone()));

        // A launcher must link directly to the binary. A second link in the
        // chain can change later.
        let relay = root.join("relay");
        symlink(&rtk, &relay).expect("relay should link");
        std::fs::remove_file(&launcher).expect("launcher should remove");
        symlink(&relay, &launcher).expect("relay launcher should link");
        assert_eq!(
            prepare_named_binary_launcher(&rtk, name, &launcher_root),
            Some(launcher_dir.clone())
        );
        assert_eq!(std::fs::read_link(&launcher).ok(), Some(rtk.clone()));

        // A launcher directory that is a symbolic link is refused.
        let elsewhere = root.join("elsewhere");
        std::fs::create_dir_all(&elsewhere).expect("other directory should create");
        std::fs::remove_dir_all(&launcher_dir).expect("launcher directory should remove");
        symlink(&elsewhere, &launcher_dir).expect("launcher directory should link");
        assert_eq!(
            prepare_named_binary_launcher(&rtk, name, &launcher_root),
            None
        );
        assert!(std::fs::read_dir(&elsewhere)
            .expect("other directory should read")
            .next()
            .is_none());

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn bubblewrap_read_only_args_are_read_only_with_tmpfs_scratch() {
        let roots = WorkspaceRoots::new(vec![PathBuf::from("/workspace")]);
        let args = bwrap::bubblewrap_args(
            SandboxMode::ReadOnly,
            &roots,
            &[PathBuf::from("/workspace/.git")],
            None,
        );

        assert_eq!(
            args,
            strings(&[
                "--ro-bind",
                "/",
                "/",
                "--tmpfs",
                "/tmp",
                "--tmpfs",
                "/var/tmp",
                "--tmpfs",
                "/run",
                "--dev",
                "/dev",
                "--proc",
                "/proc",
                "--unshare-net",
                "--unshare-ipc",
                "--die-with-parent",
                "--new-session",
                "--unshare-pid",
            ])
        );
        assert!(!args.iter().any(|arg| arg == "--bind"));
        assert!(args.iter().any(|arg| arg == "--unshare-net"));
        assert!(args.iter().any(|arg| arg == "--unshare-ipc"));
        assert!(args.iter().any(|arg| arg == "--new-session"));
    }

    #[test]
    fn bubblewrap_workspace_write_keeps_host_scratch_roots_private() {
        let roots = WorkspaceRoots::new(vec![
            PathBuf::from("/workspace"),
            PathBuf::from("/tmp"),
            PathBuf::from("/var/tmp"),
        ]);
        let args = bwrap::bubblewrap_args(SandboxMode::WorkspaceWrite, &roots, &[], None);

        assert!(arg_triplet_index(&args, "--bind", "/workspace", "/workspace").is_some());
        for scratch in roots
            .roots()
            .iter()
            .filter(|root| bwrap::is_private_scratch_root(root))
        {
            let scratch = scratch.to_string_lossy();
            assert!(
                arg_triplet_index(&args, "--bind", &scratch, &scratch).is_none(),
                "host scratch root must not be rebound: {scratch}"
            );
        }
    }

    #[test]
    fn bubblewrap_shares_only_the_explicit_rtk_state_directory() {
        let state_dir = std::env::temp_dir().join(format!("halv-rtk-bind-{}", new_uuid_v4()));
        std::fs::create_dir_all(&state_dir).expect("rtk state dir should create");
        let roots = WorkspaceRoots::new(vec![PathBuf::from("/workspace")]);
        let args = bwrap::bubblewrap_args(SandboxMode::ReadOnly, &roots, &[], Some(&state_dir));
        let state = state_dir.to_string_lossy();
        assert!(arg_triplet_index(&args, "--bind", &state, &state).is_some());
        assert!(arg_triplet_index(&args, "--bind", "/tmp", "/tmp").is_none());
        let _ = std::fs::remove_dir_all(state_dir);
    }

    #[test]
    fn bubblewrap_environment_hides_host_socket_discovery() {
        let mut command = Command::new("true");
        for key in [
            "DBUS_SESSION_BUS_ADDRESS",
            "DOCKER_HOST",
            "SSH_AUTH_SOCK",
            "XDG_RUNTIME_DIR",
        ] {
            command.env(key, "host-secret-socket");
        }

        bwrap::harden_command_environment(&mut command);

        let environment = command
            .get_envs()
            .map(|(key, value)| {
                (
                    key.to_string_lossy().to_string(),
                    value.map(|value| value.to_string_lossy().to_string()),
                )
            })
            .collect::<std::collections::HashMap<_, _>>();
        assert_eq!(environment.get("TMPDIR"), Some(&Some("/tmp".to_string())));
        for key in [
            "DBUS_SESSION_BUS_ADDRESS",
            "DOCKER_HOST",
            "SSH_AUTH_SOCK",
            "XDG_RUNTIME_DIR",
        ] {
            assert_eq!(environment.get(key), Some(&None));
        }
    }

    #[cfg(unix)]
    #[test]
    fn rtk_resolution_prefers_override_then_bundle_then_headroom_then_path() {
        let root = std::env::temp_dir().join(format!("halv-rtk-resolve-{}", new_uuid_v4()));
        let explicit = write_fake_rtk(&root.join("explicit/rtk"), "#!/bin/sh\nexit 0\n");
        let bundled = write_fake_rtk(&root.join("app/rtk"), "#!/bin/sh\nexit 0\n");
        let managed = write_fake_rtk(&root.join("headroom/bin/rtk"), "#!/bin/sh\nexit 0\n");
        let on_path = write_fake_rtk(&root.join("path/rtk"), "#!/bin/sh\nexit 0\n");
        let current_exe = root.join("app/Halv");
        let path = std::env::join_paths([on_path.parent().unwrap()]).unwrap();

        let resolved = locate_rtk_binary_with_env(
            Some(explicit.clone().into_os_string()),
            Some(current_exe.clone()),
            None,
            Some(root.join("headroom").into_os_string()),
            None,
            Some(path.clone()),
        );
        assert_eq!(resolved, Some(explicit.canonicalize().unwrap()));

        let resolved = locate_rtk_binary_with_env(
            None,
            Some(current_exe),
            None,
            Some(root.join("headroom").into_os_string()),
            None,
            Some(path.clone()),
        );
        assert_eq!(resolved, Some(bundled.canonicalize().unwrap()));

        std::fs::remove_file(&bundled).unwrap();
        let resolved = locate_rtk_binary_with_env(
            None,
            Some(root.join("app/Halv")),
            None,
            Some(root.join("headroom").into_os_string()),
            None,
            Some(path.clone()),
        );
        assert_eq!(resolved, Some(managed.canonicalize().unwrap()));

        std::fs::remove_file(&managed).unwrap();
        let resolved = locate_rtk_binary_with_env(
            None,
            Some(root.join("app/Halv")),
            None,
            Some(root.join("headroom").into_os_string()),
            None,
            Some(path),
        );
        assert_eq!(resolved, Some(on_path.canonicalize().unwrap()));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn path_assignments_are_kept_off_the_rtk_rewrite_path() {
        for command in [
            "PATH=/usr/bin git status",
            "env PATH='/custom bin' git status",
            "git status && PATH=/bin cargo test",
        ] {
            assert!(command_assigns_path(command), "must detect: {command}");
        }
        for command in [
            "printf 'PATH=/usr/bin git status'",
            "echo \"PATH=/bin\"",
            "git status",
        ] {
            assert!(!command_assigns_path(command), "must ignore: {command}");
        }
    }

    #[test]
    fn bubblewrap_workspace_write_rebinds_protected_paths_after_rw_roots() {
        let roots = WorkspaceRoots::new(vec![
            PathBuf::from("/workspace"),
            PathBuf::from("/workspace-extra"),
        ]);
        let protected_paths = vec![
            PathBuf::from("/workspace/.git"),
            PathBuf::from("/workspace/.codex"),
            PathBuf::from("/workspace-extra/.agents"),
            PathBuf::from("/outside/.git"),
        ];
        let args =
            bwrap::bubblewrap_args(SandboxMode::WorkspaceWrite, &roots, &protected_paths, None);

        let workspace_bind = arg_triplet_index(&args, "--bind", "/workspace", "/workspace")
            .expect("workspace root should be bound read-write");
        let extra_bind = arg_triplet_index(&args, "--bind", "/workspace-extra", "/workspace-extra")
            .expect("second workspace root should be bound read-write");
        let git_ro = arg_triplet_index(&args, "--ro-bind", "/workspace/.git", "/workspace/.git")
            .expect(".git should be re-bound read-only");
        let codex_ro =
            arg_triplet_index(&args, "--ro-bind", "/workspace/.codex", "/workspace/.codex")
                .expect(".codex should be re-bound read-only");
        let agents_ro = arg_triplet_index(
            &args,
            "--ro-bind",
            "/workspace-extra/.agents",
            "/workspace-extra/.agents",
        )
        .expect(".agents should be re-bound read-only");

        assert!(workspace_bind < git_ro);
        assert!(workspace_bind < codex_ro);
        assert!(extra_bind < agents_ro);
        assert!(workspace_bind < extra_bind);
        assert!(extra_bind < git_ro);
        assert!(arg_triplet_index(&args, "--ro-bind", "/outside/.git", "/outside/.git").is_none());
    }

    #[test]
    fn bubblewrap_unshares_network_except_for_danger_full_access() {
        let roots = WorkspaceRoots::new(vec![PathBuf::from("/workspace")]);
        let protected_paths = [PathBuf::from("/workspace/.git")];

        for policy in [SandboxMode::ReadOnly, SandboxMode::WorkspaceWrite] {
            let args = bwrap::bubblewrap_args(policy, &roots, &protected_paths, None);
            assert!(
                args.iter().any(|arg| arg == "--unshare-net"),
                "{policy:?} should unshare network"
            );
        }

        let danger_args = bwrap::bubblewrap_args(
            SandboxMode::DangerFullAccess,
            &roots,
            &protected_paths,
            None,
        );
        assert!(!danger_args.iter().any(|arg| arg == "--unshare-net"));
        assert!(danger_args.is_empty());
    }

    #[test]
    fn linux_missing_bwrap_reports_structured_sandbox_unavailable_without_path_lookup() {
        let platform = sandbox_platform_for("linux", || true, || false);
        let message = match platform {
            SandboxPlatform::Unavailable(message) => message,
            other => panic!("missing bwrap should be unavailable, got {other:?}"),
        };
        assert!(message.contains("Linux bubblewrap sandbox is unavailable"));
        assert!(message.contains("install bubblewrap with your distro's package manager"));

        let result = platform_unavailable_result("echo hi", message);
        assert_eq!(result.exit_code, 1);
        assert_eq!(result.wall_time, Duration::ZERO);
        assert!(result
            .captured_output
            .contains("Command was not run:\necho hi"));
        assert!(result
            .model_output
            .starts_with("Exit code: 1\nWall time: 0.000 seconds"));
    }

    #[test]
    fn approval_cache_keys_command_cwd_sandbox_and_additional_permissions() {
        let cache = ApprovalCache::default();
        let approved = ApprovalKey::new(
            "git status",
            "/repo",
            SandboxPermissions::RequireEscalated,
            false,
            DEFAULT_TIMEOUT_MS,
            vec!["network".to_string(), "write:/repo".to_string()],
        );
        cache.approve_for_session(approved.clone());

        assert!(cache.contains(&ApprovalKey::new(
            "git status",
            "/repo",
            SandboxPermissions::RequireEscalated,
            false,
            DEFAULT_TIMEOUT_MS,
            vec!["write:/repo".to_string(), "network".to_string()],
        )));
        assert!(!cache.contains(&ApprovalKey::new(
            "git status",
            "/other",
            SandboxPermissions::RequireEscalated,
            false,
            DEFAULT_TIMEOUT_MS,
            vec!["network".to_string(), "write:/repo".to_string()],
        )));
        assert!(!cache.contains(&ApprovalKey::new(
            "git status",
            "/repo",
            SandboxPermissions::UseDefault,
            false,
            DEFAULT_TIMEOUT_MS,
            vec!["network".to_string(), "write:/repo".to_string()],
        )));
        assert!(!cache.contains(&ApprovalKey::new(
            "git status",
            "/repo",
            SandboxPermissions::RequireEscalated,
            false,
            DEFAULT_TIMEOUT_MS,
            vec!["write:/other".to_string()],
        )));
        assert!(!cache.contains(&ApprovalKey::new(
            "git status",
            "/repo",
            SandboxPermissions::RequireEscalated,
            true,
            DEFAULT_TIMEOUT_MS,
            vec!["network".to_string(), "write:/repo".to_string()],
        )));
        assert!(!cache.contains(&ApprovalKey::new(
            "git status",
            "/repo",
            SandboxPermissions::RequireEscalated,
            false,
            DEFAULT_TIMEOUT_MS + 1,
            vec!["network".to_string(), "write:/repo".to_string()],
        )));
        assert!(!cache.contains(&ApprovalKey::new(
            "git  status",
            "/repo",
            SandboxPermissions::RequireEscalated,
            false,
            DEFAULT_TIMEOUT_MS,
            vec!["network".to_string(), "write:/repo".to_string()],
        )));
    }

    #[test]
    fn approval_cache_preserves_shell_whitespace_exactly() {
        let cache = ApprovalCache::default();
        let approved = ApprovalKey::new(
            "rm \"safe file\"",
            "/repo",
            SandboxPermissions::UseDefault,
            false,
            DEFAULT_TIMEOUT_MS,
            Vec::new(),
        );
        cache.approve_for_session(approved.clone());

        assert!(cache.contains(&approved));
        assert!(!cache.contains(&ApprovalKey::new(
            "rm \"safe  file\"",
            "/repo",
            SandboxPermissions::UseDefault,
            false,
            DEFAULT_TIMEOUT_MS,
            Vec::new(),
        )));
    }

    #[test]
    fn shell_approval_cache_is_scoped_to_conversation_and_project() {
        let cache = ApprovalCache::default();
        let approved = ApprovalKey::new_scoped(
            "conversation-a",
            "/projects/a",
            "cargo test",
            "/projects/a",
            SandboxPermissions::UseDefault,
            false,
            DEFAULT_TIMEOUT_MS,
            Vec::new(),
        );
        let other_conversation = ApprovalKey::new_scoped(
            "conversation-b",
            "/projects/a",
            "cargo test",
            "/projects/a",
            SandboxPermissions::UseDefault,
            false,
            DEFAULT_TIMEOUT_MS,
            Vec::new(),
        );
        let other_project = ApprovalKey::new_scoped(
            "conversation-a",
            "/projects/b",
            "cargo test",
            "/projects/a",
            SandboxPermissions::UseDefault,
            false,
            DEFAULT_TIMEOUT_MS,
            Vec::new(),
        );

        cache.approve_for_session(approved.clone());
        assert!(cache.contains(&approved));
        assert!(!cache.contains(&other_conversation));
        assert!(!cache.contains(&other_project));
    }

    #[cfg(unix)]
    #[test]
    fn shell_workdir_is_project_relative_and_bound_to_resolved_identity() {
        let base = std::env::temp_dir().join(format!("halv-shell-cwd-{}", new_uuid_v4()));
        let project = base.join("project");
        let first_target = project.join("first");
        let second_target = project.join("second");
        let spaced_target = project.join(" spaced ");
        let link = project.join("linked");
        std::fs::create_dir_all(&first_target).expect("first workdir should create");
        std::fs::create_dir_all(&second_target).expect("second workdir should create");
        std::fs::create_dir_all(&spaced_target).expect("spaced workdir should create");
        std::os::unix::fs::symlink(&first_target, &link).expect("workdir symlink should create");
        let mut request = ShellCommandRequest {
            command: "pwd".to_string(),
            workdir: Some("linked".to_string()),
            timeout_ms: None,
            login: None,
            sandbox_permissions: SandboxPermissions::UseDefault,
            justification: None,
            prefix_rule: None,
            additional_permissions: None,
        };

        let first = resolve_shell_cwd(&request, &project.to_string_lossy())
            .expect("relative workdir should resolve under the project");
        assert_eq!(PathBuf::from(&first), first_target.canonicalize().unwrap());

        std::fs::remove_file(&link).expect("old workdir symlink should remove");
        std::os::unix::fs::symlink(&second_target, &link)
            .expect("replacement workdir symlink should create");
        let second = resolve_shell_cwd(&request, &project.to_string_lossy())
            .expect("retargeted workdir should resolve");
        assert_ne!(first, second);

        request.workdir = Some(first.clone());
        assert_eq!(
            resolve_shell_cwd(&request, &project.to_string_lossy())
                .expect("resolved workdir should stay stable"),
            first
        );

        request.workdir = Some(" spaced ".to_string());
        assert_eq!(
            PathBuf::from(
                resolve_shell_cwd(&request, &project.to_string_lossy())
                    .expect("workdir whitespace should be preserved")
            ),
            spaced_target.canonicalize().unwrap()
        );

        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn patch_approval_cache_is_scoped_to_conversation_and_resolved_project_path() {
        let base =
            std::env::temp_dir().join(format!("halv-patch-approval-scope-{}", new_uuid_v4()));
        let project_a = base.join("a/repo");
        let project_b = base.join("b/repo");
        std::fs::create_dir_all(&project_a).expect("project A should create");
        std::fs::create_dir_all(&project_b).expect("project B should create");
        let roots_a = WorkspaceRoots::for_cwd(&project_a);
        let roots_b = WorkspaceRoots::for_cwd(&project_b);
        let key_a =
            patch_approval_key_for("conversation-a", &project_a, &roots_a, "../outside.txt")
                .expect("project A approval key should resolve");
        let other_project =
            patch_approval_key_for("conversation-a", &project_b, &roots_b, "../outside.txt")
                .expect("project B approval key should resolve");
        let other_conversation =
            patch_approval_key_for("conversation-b", &project_a, &roots_a, "../outside.txt")
                .expect("other conversation approval key should resolve");

        let cache = ApprovalCache::default();
        cache.approve_patch_paths([key_a.clone()]);
        assert!(cache.contains_patch_path(&key_a));
        assert!(!cache.contains_patch_path(&other_project));
        assert!(!cache.contains_patch_path(&other_conversation));

        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn shell_apply_patch_invocation_is_intercepted() {
        let command = "apply_patch <<'PATCH'\n*** Begin Patch\n*** Add File: hi.txt\n+hi\n*** End Patch\nPATCH\n";
        let patch = extract_apply_patch_from_shell_command(command)
            .expect("apply_patch command should intercept")
            .expect("patch should parse from heredoc");
        assert!(patch.starts_with("*** Begin Patch"));
        assert!(patch.contains("*** Add File: hi.txt"));
        assert!(extract_apply_patch_from_shell_command("apply_patcher nope").is_none());
    }

    #[test]
    fn approval_policy_labels_parse_and_serialize_consistently() {
        for label in ["on-failure", "on_failure"] {
            assert_eq!(
                ApprovalPolicy::from_storage(Some(label), None),
                ApprovalPolicy::OnFailure
            );
        }
        assert_eq!(
            serde_json::from_str::<ApprovalPolicy>("\"on-failure\"").unwrap(),
            ApprovalPolicy::OnFailure
        );
        let serialized = serde_json::to_string(&ApprovalPolicy::OnFailure).unwrap();
        assert_eq!(serialized, "\"on_failure\"");
        assert_eq!(
            serde_json::from_str::<ApprovalPolicy>(&serialized).unwrap(),
            ApprovalPolicy::OnFailure
        );

        assert_eq!(
            ApprovalPolicy::from_storage(Some("skip_all"), None),
            ApprovalPolicy::SkipAll
        );
        assert_eq!(
            ApprovalPolicy::from_storage(None, None),
            ApprovalPolicy::Ask
        );
        assert_eq!(
            ApprovalPolicy::from_storage(Some("unexpected"), Some("unknown")),
            ApprovalPolicy::Ask
        );
    }

    #[test]
    fn patch_safety_decision_table_respects_roots_policy_and_protected_paths() {
        let root = std::env::temp_dir().join(format!("halv-safety-{}", new_uuid_v4()));
        std::fs::create_dir_all(root.join(".git")).expect("test dirs should create");
        let roots = WorkspaceRoots::new(vec![root.clone()]);

        let safe = apply_patch::parse_patch(
            "*** Begin Patch\n*** Add File: safe.txt\n+ok\n*** End Patch\n",
        )
        .unwrap();
        let safe_decision = assess_patch_safety(&safe, &root, &roots, ApprovalPolicy::OnRequest);
        assert_eq!(safe_decision, PatchSafety::AutoApproved);
        assert_eq!(
            assess_patch_safety(&safe, &root, &roots, ApprovalPolicy::Ask),
            PatchSafety::RequiresApproval(Vec::new())
        );

        let outside_path = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/Users"))
            .join(format!("halv-outside-{}.txt", new_uuid_v4()))
            .to_string_lossy()
            .to_string();
        let outside = apply_patch::parse_patch(&format!(
            "*** Begin Patch\n*** Add File: {outside_path}\n+outside\n*** End Patch\n"
        ))
        .unwrap();
        assert_eq!(
            assess_patch_safety(&outside, &root, &roots, ApprovalPolicy::Ask),
            PatchSafety::RequiresApproval(vec![outside_path.clone()])
        );
        assert!(matches!(
            assess_patch_safety(&outside, &root, &roots, ApprovalPolicy::Never),
            PatchSafety::Denied(_)
        ));
        assert_eq!(
            assess_patch_safety(&outside, &root, &roots, ApprovalPolicy::SkipAll),
            PatchSafety::AutoApproved
        );

        let protected = apply_patch::parse_patch(
            "*** Begin Patch\n*** Add File: .git/config\n+nope\n*** End Patch\n",
        )
        .unwrap();
        assert!(matches!(
            assess_patch_safety(&protected, &root, &roots, ApprovalPolicy::OnRequest),
            PatchSafety::Denied(reason) if reason.contains("protected")
        ));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn project_read_path_rejects_parent_escape() {
        let base = std::env::temp_dir().join(format!("halv-project-read-{}", new_uuid_v4()));
        let root = base.join("project");
        let outside = base.join("outside.png");
        std::fs::create_dir_all(&root).expect("project root should create");
        std::fs::write(&outside, b"outside").expect("outside file should create");

        let error = resolve_project_read_path(&root, "../outside.png")
            .expect_err("parent traversal should be rejected");
        assert!(error.contains("outside the project root"));

        let _ = std::fs::remove_dir_all(base);
    }

    #[cfg(unix)]
    #[test]
    fn project_read_path_rejects_symlink_escape() {
        let base = std::env::temp_dir().join(format!("halv-project-read-link-{}", new_uuid_v4()));
        let root = base.join("project");
        let outside = base.join("outside.png");
        std::fs::create_dir_all(&root).expect("project root should create");
        std::fs::write(&outside, b"outside").expect("outside file should create");
        std::os::unix::fs::symlink(&outside, root.join("linked.png"))
            .expect("symlink should create");

        let error = resolve_project_read_path(&root, "linked.png")
            .expect_err("symlink escape should be rejected");
        assert!(error.contains("outside the project root"));
        assert!(error.contains("symbolic link"));

        let _ = std::fs::remove_dir_all(base);
    }

    #[cfg(unix)]
    #[test]
    fn symlink_escape_to_outside_workspace_is_rejected() {
        let root = std::env::temp_dir().join(format!("halv-symlink-root-{}", new_uuid_v4()));
        let outside = std::env::temp_dir().join(format!("halv-symlink-outside-{}", new_uuid_v4()));
        std::fs::create_dir_all(&root).expect("workspace should create");
        std::fs::create_dir_all(&outside).expect("outside dir should create");
        std::os::unix::fs::symlink(&outside, root.join("link")).expect("symlink should create");
        let outside_file = outside.join("escaped.txt");
        let roots = WorkspaceRoots::new(vec![root.clone()]);
        let parsed = apply_patch::parse_patch(
            "*** Begin Patch\n*** Add File: link/escaped.txt\n+escape\n*** End Patch\n",
        )
        .unwrap();

        let decision = assess_patch_safety(&parsed, &root, &roots, ApprovalPolicy::OnRequest);
        let apply_result = apply_parsed_patch_in_process(&root, &parsed, &roots);
        let rejected = matches!(decision, PatchSafety::Denied(_)) && apply_result.is_err();
        println!(
            "SECURITY_PROOF symlink_escape_outside rejected={} outside_file_exists={}",
            rejected,
            outside_file.exists()
        );

        assert!(matches!(
            decision,
            PatchSafety::Denied(reason) if reason.contains("symbolic link")
        ));
        assert!(apply_result
            .unwrap_err()
            .contains("symbolic link path component"));
        assert!(!outside_file.exists());
        let _ = std::fs::remove_dir_all(root);
        let _ = std::fs::remove_dir_all(outside);
    }

    #[cfg(unix)]
    #[test]
    fn symlink_escape_to_git_directory_is_rejected() {
        let root = std::env::temp_dir().join(format!("halv-symlink-git-{}", new_uuid_v4()));
        std::fs::create_dir_all(root.join(".git/hooks")).expect("git dirs should create");
        std::os::unix::fs::symlink(root.join(".git"), root.join("g"))
            .expect("symlink should create");
        let hook = root.join(".git/hooks/pre-commit");
        let roots = WorkspaceRoots::for_cwd(&root);
        let parsed = apply_patch::parse_patch(
            "*** Begin Patch\n*** Add File: g/hooks/pre-commit\n+escape\n*** End Patch\n",
        )
        .unwrap();

        let decision = assess_patch_safety(&parsed, &root, &roots, ApprovalPolicy::OnRequest);
        let apply_result = apply_parsed_patch_in_process(&root, &parsed, &roots);
        let rejected = matches!(decision, PatchSafety::Denied(_)) && apply_result.is_err();
        println!(
            "SECURITY_PROOF symlink_escape_git rejected={} git_hook_exists={}",
            rejected,
            hook.exists()
        );

        assert!(matches!(
            decision,
            PatchSafety::Denied(reason) if reason.contains("symbolic link")
        ));
        assert!(apply_result
            .unwrap_err()
            .contains("symbolic link path component"));
        assert!(!hook.exists());
        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[test]
    fn workspace_roots_reject_directory_replaced_by_symlink() {
        let base = std::env::temp_dir().join(format!("halv-root-swap-{}", new_uuid_v4()));
        let root = base.join("project");
        let outside = base.join("outside");
        std::fs::create_dir_all(&root).expect("project root should create");
        std::fs::create_dir_all(&outside).expect("outside root should create");
        let registered = root.canonicalize().expect("project should canonicalize");
        let roots = WorkspaceRoots::new(vec![registered.clone()]);

        std::fs::remove_dir(&root).expect("project root should remove");
        std::os::unix::fs::symlink(&outside, &root).expect("replacement symlink should create");

        assert!(roots.validate_roots().is_err());
        assert!(roots
            .resolve_write_path(&outside.join("escape.txt"))
            .is_err());
        let reconstructed = WorkspaceRoots::new(vec![registered]);
        assert!(!reconstructed
            .roots()
            .iter()
            .any(|candidate| candidate == &outside));

        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn approved_out_of_root_patch_uses_explicit_real_roots_and_rejects_symlink_escape() {
        let root = std::env::temp_dir().join(format!("halv-approved-root-{}", new_uuid_v4()));
        let outside = std::env::temp_dir().join(format!("halv-approved-outside-{}", new_uuid_v4()));
        std::fs::create_dir_all(&root).expect("workspace should create");
        std::fs::create_dir_all(&outside).expect("outside dir should create");
        let approved_path = outside
            .canonicalize()
            .expect("outside dir should canonicalize")
            .join("approved.txt");
        let approved_display = approved_path.to_string_lossy().to_string();
        let roots = WorkspaceRoots::new(vec![root.clone()]);
        let parsed = apply_patch::parse_patch(&format!(
            "*** Begin Patch\n*** Add File: {approved_display}\n+approved\n*** End Patch\n"
        ))
        .unwrap();

        assert_eq!(
            assess_patch_safety(&parsed, &root, &roots, ApprovalPolicy::Ask),
            PatchSafety::RequiresApproval(vec![approved_display.clone()])
        );
        let approved_roots = roots
            .approved_patch_roots(&root, std::slice::from_ref(&approved_display))
            .expect("approved path should expand roots");
        apply_parsed_patch_in_process(&root, &parsed, &approved_roots)
            .expect("approved explicit path should write");
        assert_eq!(
            std::fs::read_to_string(&approved_path).unwrap(),
            "approved\n"
        );

        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&outside, root.join("link")).expect("symlink should create");
            let escape = outside.join("escape.txt");
            let escape_patch = apply_patch::parse_patch(
                "*** Begin Patch\n*** Add File: link/escape.txt\n+escape\n*** End Patch\n",
            )
            .unwrap();
            assert!(apply_parsed_patch_in_process(&root, &escape_patch, &approved_roots).is_err());
            assert!(!escape.exists());
        }

        let _ = std::fs::remove_dir_all(root);
        let _ = std::fs::remove_dir_all(outside);
    }

    #[test]
    fn skip_all_patch_roots_allow_protected_paths() {
        let root = std::env::temp_dir().join(format!("halv-skip-all-root-{}", new_uuid_v4()));
        std::fs::create_dir_all(root.join(".git")).expect("git dir should create");
        let roots = WorkspaceRoots::for_cwd(&root);
        let parsed = apply_patch::parse_patch(
            "*** Begin Patch\n*** Add File: .git/config\n+skip-all\n*** End Patch\n",
        )
        .unwrap();

        assert_eq!(
            assess_patch_safety(&parsed, &root, &roots, ApprovalPolicy::SkipAll),
            PatchSafety::AutoApproved
        );
        let touched_paths = apply_patch_touched_paths(&parsed);
        let approved_roots = roots
            .with_danger_full_access()
            .approved_patch_roots(&root, &touched_paths)
            .expect("skip_all roots should accept protected paths");
        apply_parsed_patch_in_process(&root, &parsed, &approved_roots)
            .expect("skip_all protected patch should apply");
        assert_eq!(
            std::fs::read_to_string(root.join(".git/config")).unwrap(),
            "skip-all\n"
        );

        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[test]
    fn restore_refuses_snapshot_path_that_resolves_outside_workspace() {
        let root = std::env::temp_dir().join(format!("halv-restore-root-{}", new_uuid_v4()));
        let outside = std::env::temp_dir().join(format!("halv-restore-outside-{}", new_uuid_v4()));
        std::fs::create_dir_all(&root).expect("workspace should create");
        std::fs::create_dir_all(&outside).expect("outside dir should create");
        std::os::unix::fs::symlink(&outside, root.join("link")).expect("symlink should create");
        let outside_file = outside.join("escaped.txt");
        std::fs::write(&outside_file, "original\n").expect("outside file should write");
        let snapshot = FileSnapshot {
            display_path: "link/escaped.txt".to_string(),
            absolute_path: root.join("link/escaped.txt"),
            before: Some(b"restored\n".to_vec()),
            after_exists: None,
            after_sha256: None,
        };

        let error = restore_patch_snapshots(&root, &[snapshot]).unwrap_err();

        assert!(error.contains("symbolic link path component"));
        assert_eq!(std::fs::read_to_string(outside_file).unwrap(), "original\n");
        let _ = std::fs::remove_dir_all(root);
        let _ = std::fs::remove_dir_all(outside);
    }

    #[test]
    fn restore_refuses_stale_files_before_changing_any_path() {
        let root = std::env::temp_dir().join(format!("halv-stale-revert-{}", new_uuid_v4()));
        std::fs::create_dir_all(&root).expect("workspace should create");
        let root = root.canonicalize().expect("workspace should canonicalize");
        let first_path = root.join("first.txt");
        let second_path = root.join("second.txt");
        std::fs::write(&first_path, "turn-first\n").expect("first postimage should write");
        std::fs::write(&second_path, "user-second\n").expect("user edit should write");

        let mut first = FileSnapshot {
            display_path: "first.txt".to_string(),
            absolute_path: first_path.clone(),
            before: Some(b"before-first\n".to_vec()),
            after_exists: None,
            after_sha256: None,
        };
        first.capture_postimage(Some(b"turn-first\n"));
        let mut second = FileSnapshot {
            display_path: "second.txt".to_string(),
            absolute_path: second_path.clone(),
            before: Some(b"before-second\n".to_vec()),
            after_exists: None,
            after_sha256: None,
        };
        second.capture_postimage(Some(b"turn-second\n"));

        let error = restore_patch_snapshots(&root, &[first, second]).unwrap_err();

        assert!(error.contains("changed after the turn"), "{error}");
        assert_eq!(std::fs::read_to_string(first_path).unwrap(), "turn-first\n");
        assert_eq!(
            std::fs::read_to_string(second_path).unwrap(),
            "user-second\n"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn restore_preserves_a_created_file_edited_after_the_turn() {
        let root = std::env::temp_dir().join(format!("halv-stale-created-{}", new_uuid_v4()));
        std::fs::create_dir_all(&root).expect("workspace should create");
        let root = root.canonicalize().expect("workspace should canonicalize");
        let path = root.join("created.txt");
        std::fs::write(&path, "user edit\n").expect("user edit should write");
        let mut snapshot = FileSnapshot {
            display_path: "created.txt".to_string(),
            absolute_path: path.clone(),
            before: None,
            after_exists: None,
            after_sha256: None,
        };
        snapshot.capture_postimage(Some(b"turn content\n"));

        let error = restore_patch_snapshots(&root, &[snapshot]).unwrap_err();

        assert!(error.contains("changed after the turn"), "{error}");
        assert_eq!(std::fs::read_to_string(path).unwrap(), "user edit\n");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn auto_branch_slug_and_clean_dirty_gating() {
        let repo = std::env::temp_dir().join(format!("halv-git-{}", new_uuid_v4()));
        std::fs::create_dir_all(&repo).expect("repo dir should create");
        git(&repo, &["init", "-b", "main"]);
        git(&repo, &["config", "user.email", "test@example.com"]);
        git(&repo, &["config", "user.name", "Halv Test"]);
        std::fs::write(repo.join("README.md"), "hello\n").expect("readme should write");
        git(&repo, &["add", "README.md"]);
        git(&repo, &["commit", "-m", "initial"]);

        match prepare_first_write(&repo, "Stage C write tools!") {
            FirstWriteAction::Proceed(status) => {
                assert_eq!(
                    status.auto_created_branch.as_deref(),
                    Some("halv/stage-c-write-tools")
                );
                assert_eq!(
                    git_output(&repo, ["branch", "--show-current"])
                        .map(|value| value.trim().to_string()),
                    Some("halv/stage-c-write-tools".to_string())
                );
            }
            other => panic!("clean default branch should auto-branch, got {other:?}"),
        }

        git(&repo, &["switch", "main"]);
        git(&repo, &["checkout", "--detach", "HEAD"]);
        match prepare_first_write(&repo, "detached") {
            FirstWriteAction::RequiresConfirmation(status) => {
                assert!(status.branch.is_none());
                assert!(status.warning.unwrap_or_default().contains("detached HEAD"));
            }
            other => panic!("detached HEAD should require confirmation, got {other:?}"),
        }

        git(&repo, &["switch", "main"]);
        std::fs::write(repo.join("dirty.txt"), "dirty\n").expect("dirty file should write");
        match prepare_first_write(&repo, "dirty") {
            FirstWriteAction::RequiresConfirmation(status) => {
                assert!(status.warning.unwrap_or_default().contains("dirty"));
            }
            other => panic!("dirty worktree should require confirmation, got {other:?}"),
        }

        let non_git = std::env::temp_dir().join(format!("halv-nongit-{}", new_uuid_v4()));
        std::fs::create_dir_all(&non_git).expect("non git dir should create");
        match prepare_first_write(&non_git, "non git") {
            FirstWriteAction::RequiresConfirmation(status) => {
                assert!(!status.is_git_repo);
                assert!(status
                    .warning
                    .unwrap_or_default()
                    .contains("not a git repository"));
            }
            other => panic!("non-git folder should require confirmation, got {other:?}"),
        }

        let _ = std::fs::remove_dir_all(repo);
        let _ = std::fs::remove_dir_all(non_git);
    }

    #[cfg(unix)]
    #[test]
    fn managed_git_ignores_repository_fsmonitor_hooks() {
        use std::os::unix::fs::PermissionsExt;

        let repo = std::env::temp_dir().join(format!("halv-fsmonitor-{}", new_uuid_v4()));
        let sentinel = repo.join("fsmonitor-ran");
        let hook = repo.join("fsmonitor.sh");
        std::fs::create_dir_all(&repo).expect("repo dir should create");
        git(&repo, &["init", "-b", "main"]);
        git(&repo, &["config", "user.email", "test@example.com"]);
        git(&repo, &["config", "user.name", "Halv Test"]);
        std::fs::write(repo.join("README.md"), "hello\n").expect("readme should write");
        git(&repo, &["add", "README.md"]);
        git(&repo, &["commit", "-m", "initial"]);
        std::fs::write(
            &hook,
            format!(
                "#!/bin/sh\n: > '{}'\nprintf 'token\\n'\n",
                sentinel.display()
            ),
        )
        .expect("fsmonitor hook should write");
        let mut permissions = std::fs::metadata(&hook).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&hook, permissions).unwrap();
        git(
            &repo,
            &["config", "core.fsmonitor", hook.to_string_lossy().as_ref()],
        );
        git(&repo, &["config", "core.fsmonitorHookVersion", "2"]);

        git(&repo, &["status", "--porcelain"]);
        assert!(
            sentinel.exists(),
            "test hook should be executable by plain Git"
        );
        std::fs::remove_file(&sentinel).unwrap();

        let status = git_status(&repo);
        assert!(status.is_git_repo);
        assert!(
            !sentinel.exists(),
            "managed Git must disable core.fsmonitor"
        );

        let _ = std::fs::remove_dir_all(repo);
    }

    #[test]
    fn repository_program_keys_reset_to_trusted_values() {
        let listing = [
            ("system", "credential.helper\nosxkeychain"),
            ("global", "credential.https://github.com.helper\n"),
            (
                "global",
                "credential.https://github.com.helper\n!gh auth git-credential",
            ),
            ("global", "filter.lfs.clean\ngit-lfs clean -- %f"),
            ("global", "gpg.program\n/opt/homebrew/bin/gpg"),
            ("global", "core.pager\nless"),
            ("local", "core.bare"),
            ("local", "filter.lfs.clean\nevil"),
            ("local", "filter.Evil.process\nevil"),
            ("local", "diff.evil.textconv\nevil"),
            ("local", "credential.https://example.test.helper\nevil"),
            ("local", "core.sshcommand\nevil"),
            ("local", "core.askpass\nevil"),
            ("local", "filter.lfs.clean\nevil again"),
            ("worktree", "gpg.openpgp.program\nevil"),
            ("worktree", "remote.origin.uploadpack\nevil"),
            ("command", "diff.external\nhalv"),
        ]
        .iter()
        .flat_map(|(scope, item)| format!("{scope}\0{item}\0").into_bytes())
        .collect::<Vec<u8>>();

        let entries = parse_git_config_listing(&listing);
        assert_eq!(entries.len(), 17);
        assert_eq!(entries[6].key, "core.bare");
        assert_eq!(entries[6].value, "");
        let overrides = repository_command_overrides(&entries);
        let expected = [
            ("filter.lfs.clean", ""),
            ("filter.lfs.clean", "git-lfs clean -- %f"),
            ("filter.Evil.process", ""),
            ("diff.evil.textconv", ""),
            ("credential.helper", ""),
            ("credential.helper", "osxkeychain"),
            ("credential.https://github.com.helper", ""),
            (
                "credential.https://github.com.helper",
                "!gh auth git-credential",
            ),
            ("core.sshcommand", "ssh"),
            ("core.askpass", ""),
            ("gpg.program", "gpg"),
            ("gpg.program", "/opt/homebrew/bin/gpg"),
            ("remote.origin.uploadpack", "git-upload-pack"),
        ]
        .map(|(key, value)| (key.to_string(), value.to_string()));
        assert_eq!(overrides, expected);
    }

    /// Parses `(scope, "key\nvalue")` items as a config listing.
    fn config_entries(items: &[(&str, &str)]) -> Vec<GitConfigEntry> {
        let listing = items
            .iter()
            .flat_map(|(scope, item)| format!("{scope}\0{item}\0").into_bytes())
            .collect::<Vec<u8>>();
        parse_git_config_listing(&listing)
    }

    fn resets(keys: &[&str]) -> Vec<(String, String)> {
        keys.iter()
            .map(|key| (key.to_string(), String::new()))
            .collect()
    }

    #[test]
    fn repository_lfs_filter_stays_only_with_standard_values() {
        let standard = [
            ("local", "lfs.repositoryformatversion\n0"),
            ("local", "filter.lfs.clean\ngit-lfs clean -- %f"),
            ("local", "filter.lfs.smudge\ngit-lfs smudge -- %f"),
            ("local", "filter.lfs.process\ngit-lfs filter-process"),
            ("local", "filter.lfs.required\ntrue"),
        ];
        assert_eq!(
            repository_command_overrides(&config_entries(&standard)),
            Vec::new()
        );
        assert_eq!(
            repository_command_overrides(&config_entries(&[
                ("global", "filter.lfs.clean\n/opt/custom-lfs clean %f"),
                ("worktree", "filter.lfs.process\ngit-lfs filter-process"),
                ("worktree", "filter.lfs.required\nfalse"),
            ])),
            Vec::new()
        );
        // The values of "git lfs install --local --skip-smudge".
        let skip_smudge = [
            ("local", "lfs.repositoryformatversion\n0"),
            ("local", "filter.lfs.clean\ngit-lfs clean -- %f"),
            ("local", "filter.lfs.smudge\ngit-lfs smudge --skip -- %f"),
            ("local", "filter.lfs.process\ngit-lfs filter-process --skip"),
            ("local", "filter.lfs.required\ntrue"),
        ];
        assert_eq!(
            repository_command_overrides(&config_entries(&skip_smudge)),
            Vec::new()
        );
        let mut both = standard.to_vec();
        both.extend(skip_smudge);
        assert_eq!(
            repository_command_overrides(&config_entries(&both)),
            Vec::new()
        );

        // One changed or added entry resets each program key of the driver.
        let driver = resets(&[
            "filter.lfs.clean",
            "filter.lfs.smudge",
            "filter.lfs.process",
        ]);
        for changed in [
            "filter.lfs.clean\ngit-lfs clean -- %f; touch /tmp/halv-test",
            "filter.lfs.clean\ngit-lfs clean -- %f ",
            "filter.lfs.clean\n/tmp/git-lfs clean -- %f",
            "filter.lfs.clean\ngit-lfs smudge -- %f",
            "filter.lfs.clean\ngit-lfs clean --skip -- %f",
            "filter.lfs.smudge\ngit-lfs smudge --skip %f",
            "filter.lfs.smudge\ngit-lfs smudge --skip -- %f ",
            "filter.lfs.smudge\ngit-lfs smudge --skip -- %f; touch /tmp/halv-test",
            "filter.lfs.process\ngit-lfs filter-process --skip ",
            "filter.lfs.process\ngit-lfs filter-process --skip; touch /tmp/halv-test",
            "filter.lfs.process\ngit-lfs filter-process --skip --skip",
            "filter.lfs.required\nyes",
            "filter.lfs.required",
            "filter.lfs.other\ngit-lfs clean -- %f",
        ] {
            let mut listing = standard.to_vec();
            listing.push(("local", changed));
            assert_eq!(
                repository_command_overrides(&config_entries(&listing)),
                driver,
                "{changed:?}"
            );
        }

        // A standard value does not keep a driver with another name.
        for other in [
            "filter.LFS.clean",
            "filter.lfs2.clean",
            "filter.lfs.x.clean",
            "filter.git-lfs.clean",
        ] {
            let item = format!("{other}\ngit-lfs clean -- %f");
            let mut listing = standard.to_vec();
            listing.push(("local", &item));
            assert_eq!(
                repository_command_overrides(&config_entries(&listing)),
                resets(&[other]),
                "{other:?}"
            );
        }
    }

    #[test]
    fn git_lfs_program_keys_reset_to_trusted_values() {
        let entries = config_entries(&[
            ("global", "lfs.extension.ext.clean\ntrusted-ext clean %f"),
            ("local", "lfs.extension.ext.clean\nevil %f"),
            ("local", "lfs.extension.Other.smudge\nevil %f"),
            ("local", "lfs.extension.ext.priority\n0"),
            ("local", "lfs.customtransfer.agent.path\nevil"),
            ("local", "lfs.customtransfer.agent.args\n--evil"),
            // git-lfs also finds an agent in these keys.
            ("local", "x.lfs.customtransfer.agent.path\nevil"),
            ("local", "lfs.customtransfer.agent.pathx\nevil"),
            ("local", "lfs.CustomTransfer.agent.path\nevil"),
            ("local", "lfs.url\nhttps://lfs.example.invalid"),
        ]);
        let expected = [
            ("lfs.extension.ext.clean", ""),
            ("lfs.extension.ext.clean", "trusted-ext clean %f"),
            ("lfs.extension.Other.smudge", ""),
            ("lfs.customtransfer.agent.path", ""),
            ("lfs.customtransfer.agent.args", ""),
            ("x.lfs.customtransfer.agent.path", ""),
            ("lfs.customtransfer.agent.pathx", ""),
            ("lfs.CustomTransfer.agent.path", ""),
        ]
        .map(|(key, value)| (key.to_string(), value.to_string()));
        assert_eq!(repository_command_overrides(&entries), expected);
    }

    #[test]
    fn repository_lfs_storage_resets_to_the_trusted_value() {
        // An empty value gives the default directory, .git/lfs.
        assert_eq!(
            repository_command_overrides(&config_entries(&[(
                "local",
                "lfs.storage\n/tmp/halv-test"
            )])),
            resets(&["lfs.storage"])
        );
        let expected = [
            ("lfs.storage", ""),
            ("lfs.storage", "/trusted/system-lfs"),
            ("lfs.storage", "/trusted/global-lfs"),
        ]
        .map(|(key, value)| (key.to_string(), value.to_string()));
        assert_eq!(
            repository_command_overrides(&config_entries(&[
                ("system", "lfs.storage\n/trusted/system-lfs"),
                ("global", "lfs.storage\n/trusted/global-lfs"),
                ("local", "lfs.storage\n/tmp/halv-test"),
                ("worktree", "lfs.storage\n../halv-test"),
            ])),
            expected
        );
        assert_eq!(
            repository_command_overrides(&config_entries(&[(
                "global",
                "lfs.storage\n/trusted/global-lfs"
            )])),
            Vec::new()
        );

        // A standard repository filter stays, but its storage resets.
        assert_eq!(
            repository_command_overrides(&config_entries(&[
                ("local", "filter.lfs.clean\ngit-lfs clean -- %f"),
                ("local", "filter.lfs.smudge\ngit-lfs smudge -- %f"),
                ("local", "filter.lfs.process\ngit-lfs filter-process"),
                ("local", "filter.lfs.required\ntrue"),
                ("local", "lfs.storage\n/tmp/halv-test"),
            ])),
            resets(&["lfs.storage"])
        );
    }

    #[test]
    fn hidden_repository_keys_reset_each_filter_program() {
        // git-lfs reads these items as more keys, but Halv does not see them.
        let hidden = [
            "halvtest.note\nfirst\nlfs.storage=/tmp/halv-test",
            "halvtest.note\nfirst\nlfs.extension.halvtest.clean=halv-test %f",
            "lfs.storage=/tmp/halv-test.halvtest\nx",
        ];
        let standard = [
            ("filter.lfs.clean", "git-lfs clean -- %f"),
            ("filter.lfs.smudge", "git-lfs smudge -- %f"),
            ("filter.lfs.process", "git-lfs filter-process"),
            ("filter.lfs.required", "true"),
        ];
        let items = |scope: &'static str| {
            standard
                .iter()
                .map(move |(key, value)| (scope, format!("{key}\n{value}")))
                .collect::<Vec<_>>()
        };
        let listing = |items: &[(&str, String)]| {
            config_entries(
                &items
                    .iter()
                    .map(|(scope, item)| (*scope, item.as_str()))
                    .collect::<Vec<_>>(),
            )
        };
        let driver = resets(&[
            "filter.lfs.clean",
            "filter.lfs.smudge",
            "filter.lfs.process",
        ]);
        for item in hidden {
            for scope in ["local", "worktree"] {
                // The standard repository filter does not stay.
                let mut repository = items("local");
                repository.push((scope, item.to_string()));
                assert_eq!(
                    repository_command_overrides(&listing(&repository)),
                    driver,
                    "{scope} {item:?}"
                );

                // Each trusted filter program also resets, and its value
                // does not follow.
                let mut trusted = vec![("system", "filter.other.clean\nother %f".to_string())];
                trusted.extend(items("global"));
                trusted.push((scope, item.to_string()));
                assert_eq!(
                    repository_command_overrides(&listing(&trusted)),
                    resets(&[
                        "filter.other.clean",
                        "filter.lfs.clean",
                        "filter.lfs.smudge",
                        "filter.lfs.process",
                    ]),
                    "{scope} {item:?}"
                );
            }
        }

        // The user writes the trusted scopes, thus their items do not hide
        // keys.
        let mut trusted = items("global");
        trusted.extend(
            hidden
                .iter()
                .flat_map(|item| [("system", item.to_string()), ("global", item.to_string())]),
        );
        assert_eq!(repository_command_overrides(&listing(&trusted)), Vec::new());
        let mut repository = items("local");
        repository.push(("local", "halvtest.note\nfirst".to_string()));
        assert_eq!(
            repository_command_overrides(&listing(&repository)),
            Vec::new()
        );
    }

    /// Proves that the automatic git status does not start git-lfs when a
    /// repository config value has a newline, or a repository config key has
    /// a `=`. Each hidden item names an `lfs.storage` directory. The fake
    /// git-lfs makes this directory, but only in plain git. The `lfs` filter
    /// comes from the repository config, or from the global config.
    #[cfg(unix)]
    #[test]
    fn git_status_does_not_start_git_lfs_when_the_repository_config_hides_keys() {
        for global_filter in [false, true] {
            for equals_in_key in [false, true] {
                let base =
                    std::env::temp_dir().join(format!("halv-git-lfs-hidden-{}", new_uuid_v4()));
                let repo = base.join("repo");
                let storage = base.join("hidden-storage");
                let lfs = FakeGitLfs::new(&base);
                lfs_test_repository(&lfs, &repo);
                plant_standard_lfs_filter(&repo, "*.bin");
                if global_filter {
                    for (key, value) in [
                        ("filter.lfs.clean", "git-lfs clean -- %f"),
                        ("filter.lfs.smudge", "git-lfs smudge -- %f"),
                        ("filter.lfs.process", "git-lfs filter-process"),
                        ("filter.lfs.required", "true"),
                    ] {
                        lfs.git(&repo, &["config", "--global", key, value]);
                    }
                    lfs.git(&repo, &["config", "--remove-section", "filter.lfs"]);
                }
                lfs.git(&repo, &["add", "large.bin"]);
                lfs.git(&repo, &["commit", "-q", "-m", "initial"]);
                let storage = if equals_in_key {
                    let key = format!("lfs.storage={}.halvtest", storage.display());
                    lfs.git(&repo, &["config", &key, "x"]);
                    PathBuf::from(format!("{}.halvtest=x", storage.display()))
                } else {
                    let value = format!("first\nlfs.storage={}", storage.display());
                    lfs.git(&repo, &["config", "halvtest.note", &value]);
                    storage
                };
                let case = format!("global filter: {global_filter}, = in key: {equals_in_key}");
                make_stat_dirty(&repo.join("large.bin"));
                let runs = lfs.runs();

                let status = {
                    let _environment = lfs.activate();
                    git_status(&repo)
                };
                assert!(status.is_git_repo, "{case}");
                assert_eq!(
                    lfs.runs(),
                    runs,
                    "managed Git must not start git-lfs when the repository config hides keys: {case}"
                );
                assert!(!storage.exists(), "{case}");

                make_stat_dirty(&repo.join("large.bin"));
                lfs.git(&repo, &["status", "--porcelain"]);
                assert!(
                    storage.join("objects").is_dir(),
                    "plain Git should give the hidden lfs.storage key to git-lfs: {case}"
                );

                let _ = std::fs::remove_dir_all(base);
            }
        }
    }

    /// Makes a test repository with a user name and email. Then writes the
    /// file `large.bin`, but does not commit it.
    #[cfg(unix)]
    fn lfs_test_repository(lfs: &FakeGitLfs, repo: &Path) {
        std::fs::create_dir_all(repo).expect("repo dir should create");
        lfs.git(repo, &["init", "-q", "-b", "main"]);
        lfs.git(repo, &["config", "user.email", "test@example.com"]);
        lfs.git(repo, &["config", "user.name", "Halv Test"]);
        std::fs::write(repo.join("large.bin"), "large content\n").expect("file should write");
    }

    #[cfg(unix)]
    #[test]
    fn git_status_keeps_a_standard_repository_lfs_filter() {
        assert_git_status_keeps_repository_lfs_filter(&[]);
    }

    #[cfg(unix)]
    #[test]
    fn git_status_keeps_a_skip_smudge_repository_lfs_filter() {
        // The values of "git lfs install --local --skip-smudge".
        assert_git_status_keeps_repository_lfs_filter(&[
            ("filter.lfs.smudge", "git-lfs smudge --skip -- %f"),
            ("filter.lfs.process", "git-lfs filter-process --skip"),
        ]);
    }

    /// Proves that git status runs a repository `lfs` filter. The filter has
    /// the standard values, and then `changes` replaces some values.
    #[cfg(unix)]
    fn assert_git_status_keeps_repository_lfs_filter(changes: &[(&str, &str)]) {
        let base = std::env::temp_dir().join(format!("halv-git-lfs-{}", new_uuid_v4()));
        let repo = base.join("repo");
        let lfs = FakeGitLfs::new(&base);
        lfs_test_repository(&lfs, &repo);
        plant_standard_lfs_filter(&repo, "*.bin");
        for (key, value) in changes {
            lfs.git(&repo, &["config", key, value]);
        }
        lfs.git(&repo, &["add", "large.bin"]);
        lfs.git(&repo, &["commit", "-q", "-m", "initial"]);
        assert_eq!(
            lfs.git(&repo, &["cat-file", "-p", "HEAD:large.bin"]),
            FakeGitLfs::POINTER
        );
        make_stat_dirty(&repo.join("large.bin"));
        let runs = lfs.runs().len();

        let status = {
            let _environment = lfs.activate();
            git_status(&repo)
        };
        assert!(status.is_git_repo);
        assert_eq!(status.clean, Some(true));
        assert!(
            lfs.runs().len() > runs,
            "managed Git must keep a standard git-lfs filter from the repository config: {changes:?}"
        );

        let _ = std::fs::remove_dir_all(base);
    }

    #[cfg(unix)]
    #[test]
    fn git_lfs_does_not_use_a_storage_directory_from_the_repository() {
        let base = std::env::temp_dir().join(format!("halv-git-lfs-storage-{}", new_uuid_v4()));
        let repo = base.join("repo");
        let repository_storage = base.join("repository-storage");
        let trusted_storage = base.join("trusted-storage");
        let lfs = FakeGitLfs::new(&base);
        lfs_test_repository(&lfs, &repo);
        plant_standard_lfs_filter(&repo, "*.bin");
        lfs.git(&repo, &["add", "large.bin"]);
        lfs.git(&repo, &["commit", "-q", "-m", "initial"]);
        lfs.git(
            &repo,
            &[
                "config",
                "lfs.storage",
                &repository_storage.to_string_lossy(),
            ],
        );
        let managed_status = || {
            make_stat_dirty(&repo.join("large.bin"));
            let runs = lfs.runs().len();
            let status = {
                let _environment = lfs.activate();
                git_status(&repo)
            };
            assert_eq!(status.clean, Some(true));
            assert!(lfs.runs().len() > runs, "the standard filter should run");
        };

        managed_status();
        assert!(
            !repository_storage.exists(),
            "managed Git must not let the repository config choose the git-lfs storage"
        );

        lfs.git(
            &repo,
            &[
                "config",
                "--global",
                "lfs.storage",
                &trusted_storage.to_string_lossy(),
            ],
        );
        managed_status();
        assert!(
            trusted_storage.join("objects").is_dir(),
            "managed Git must keep the global git-lfs storage"
        );
        assert!(
            !repository_storage.exists(),
            "managed Git must not let the repository config choose the git-lfs storage"
        );

        make_stat_dirty(&repo.join("large.bin"));
        lfs.git(&repo, &["status", "--porcelain"]);
        assert!(
            repository_storage.join("objects").is_dir(),
            "plain Git should give the repository storage to git-lfs"
        );

        let _ = std::fs::remove_dir_all(base);
    }

    #[cfg(unix)]
    #[test]
    fn git_status_does_not_run_a_changed_repository_lfs_filter() {
        let base = std::env::temp_dir().join(format!("halv-git-lfs-changed-{}", new_uuid_v4()));
        let repo = base.join("repo");
        let sentinel = base.join("program-ran");
        let script = base.join("program");
        let lfs = FakeGitLfs::new(&base);
        lfs_test_repository(&lfs, &repo);
        lfs.git(&repo, &["add", "large.bin"]);
        lfs.git(&repo, &["commit", "-q", "-m", "initial"]);
        write_sentinel_program(&script, &sentinel, "cat > /dev/null");
        // The value starts with the standard command, and then it starts
        // another program. Without a process entry, git uses the clean entry.
        for (key, value) in [
            (
                "filter.lfs.clean",
                format!("git-lfs clean -- %f; '{}'", script.display()),
            ),
            ("filter.lfs.smudge", "git-lfs smudge -- %f".to_string()),
            ("filter.lfs.required", "true".to_string()),
        ] {
            lfs.git(&repo, &["config", key, &value]);
        }
        std::fs::create_dir_all(repo.join(".git/info")).expect("info dir should create");
        std::fs::write(repo.join(".git/info/attributes"), "*.bin filter=lfs\n")
            .expect("attributes should write");
        make_stat_dirty(&repo.join("large.bin"));

        let status = {
            let _environment = lfs.activate();
            git_status(&repo)
        };
        assert!(status.is_git_repo);
        assert!(
            !sentinel.exists(),
            "managed Git must not run a changed lfs filter from the repository config"
        );
        assert_eq!(
            lfs.runs(),
            Vec::<String>::new(),
            "managed Git must turn off each command of a changed lfs filter"
        );

        lfs.git(&repo, &["status", "--porcelain"]);
        assert!(
            sentinel.exists(),
            "the changed filter should run in plain Git"
        );

        let _ = std::fs::remove_dir_all(base);
    }

    #[cfg(unix)]
    #[test]
    fn git_status_does_not_run_a_git_lfs_program_from_the_repository() {
        let base = std::env::temp_dir().join(format!("halv-git-lfs-path-{}", new_uuid_v4()));
        let repo = base.join("repo");
        let sentinel = base.join("program-ran");
        let lfs = FakeGitLfs::new(&base);
        lfs_test_repository(&lfs, &repo);
        plant_standard_lfs_filter(&repo, "*.bin");
        lfs.git(&repo, &["add", "large.bin"]);
        lfs.git(&repo, &["commit", "-q", "-m", "initial"]);
        // The repository supplies its own git-lfs, and a relative PATH entry
        // finds it before the real program.
        std::fs::create_dir_all(repo.join("tools")).expect("tools dir should create");
        write_sentinel_program(&repo.join("tools/git-lfs"), &sentinel, "exit 1");
        std::fs::write(repo.join(".git/info/exclude"), "tools/\n").expect("exclude should write");
        let mut environment = lfs.environment();
        for (key, value) in &mut environment {
            if key == "PATH" {
                let mut path = vec![PathBuf::from("tools")];
                path.extend(std::env::split_paths(value));
                *value = std::env::join_paths(path).expect("PATH should join");
            }
        }
        make_stat_dirty(&repo.join("large.bin"));

        let status = {
            let _environment = TestGitEnvironment::set(environment.clone());
            git_status(&repo)
        };
        assert!(
            !sentinel.exists(),
            "managed Git must not find programs through a relative PATH entry"
        );
        assert_eq!(status.clean, Some(true));

        let _ = Command::new(GIT_PATH)
            .args(["status", "--porcelain"])
            .current_dir(&repo)
            .envs(environment)
            .output()
            .expect("git should run");
        assert!(
            sentinel.exists(),
            "plain Git should run the git-lfs program of the repository"
        );

        let _ = std::fs::remove_dir_all(base);
    }

    #[cfg(unix)]
    #[test]
    fn git_status_does_not_run_repository_programs() {
        let base = std::env::temp_dir().join(format!("halv-git-programs-{}", new_uuid_v4()));
        let repo = base.join("repo");
        let sentinel = base.join("program-ran");
        std::fs::create_dir_all(&repo).expect("repo dir should create");
        git(&repo, &["init", "-b", "main"]);
        git(&repo, &["config", "user.email", "test@example.com"]);
        git(&repo, &["config", "user.name", "Halv Test"]);
        std::fs::write(repo.join("README.md"), "hello\n").expect("readme should write");
        std::fs::write(repo.join("notes.txt"), "notes\n").expect("notes should write");
        git(&repo, &["add", "README.md", "notes.txt"]);
        git(&repo, &["commit", "-m", "initial"]);
        plant_repository_programs(&repo, &sentinel);

        let status = git_status(&repo);
        assert!(status.is_git_repo);
        assert_eq!(status.branch.as_deref(), Some("main"));
        assert!(
            !sentinel.exists(),
            "managed Git must not run programs from the repository config"
        );

        git(&repo, &["status", "--porcelain"]);
        assert!(
            sentinel.exists(),
            "the planted programs should run in plain Git"
        );

        let _ = std::fs::remove_dir_all(base);
    }

    #[cfg(unix)]
    #[test]
    fn managed_git_keeps_global_programs_and_skips_repository_programs() {
        let base = std::env::temp_dir().join(format!("halv-git-trust-{}", new_uuid_v4()));
        let repo = base.join("repo");
        std::fs::create_dir_all(&repo).expect("repo dir should create");
        git(&repo, &["init", "-b", "main"]);
        git(&repo, &["config", "user.email", "test@example.com"]);
        git(&repo, &["config", "user.name", "Halv Test"]);
        for name in ["trusted.txt", "local.txt", "included.txt"] {
            std::fs::write(repo.join(name), format!("{name}\n")).expect("file should write");
        }
        std::fs::write(
            repo.join(".gitattributes"),
            "trusted.txt filter=trusted\nlocal.txt filter=local\nincluded.txt filter=included\n",
        )
        .expect("attributes should write");
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "-m", "initial"]);

        let program = |name: &str, sentinel: &str, body: &str| {
            let script = base.join(name);
            let sentinel = base.join(sentinel);
            write_sentinel_program(&script, &sentinel, body);
            (format!("'{}'", script.display()), sentinel)
        };
        let (repository_filter, repository_ran) =
            program("repository-filter", "repository-ran", "cat");
        let (repository_exit, _) = program("repository-exit", "repository-ran", "exit 1");
        let (global_filter, global_filter_ran) = program("global-filter", "filter-ran", "cat");
        let (global_helper, global_helper_ran) = program("global-helper", "helper-ran", "exit 0");
        let (global_ssh, global_ssh_ran) = program("global-ssh", "ssh-ran", "exit 1");

        let global = base.join("global.gitconfig");
        std::fs::write(
            &global,
            format!(
                "[filter \"trusted\"]\n\tclean = {global_filter}\n[credential]\n\thelper = !{global_helper}\n[core]\n\tsshCommand = {global_ssh}\n"
            ),
        )
        .expect("global config should write");
        let included = base.join("included.gitconfig");
        std::fs::write(
            &included,
            format!("[filter \"included\"]\n\tclean = {repository_filter}\n"),
        )
        .expect("included config should write");
        for (key, value) in [
            ("filter.trusted.clean", repository_filter.clone()),
            ("filter.local.clean", repository_filter.clone()),
            ("include.path", included.to_string_lossy().into_owned()),
            ("credential.helper", format!("!{repository_exit}")),
            // Git runs core.askPass without a shell, so the path has no quotes.
            (
                "core.askPass",
                base.join("repository-exit").to_string_lossy().into_owned(),
            ),
            ("core.sshCommand", repository_exit.clone()),
        ] {
            git(&repo, &["config", "--add", key, &value]);
        }
        for name in ["trusted.txt", "local.txt", "included.txt"] {
            std::fs::File::options()
                .write(true)
                .open(repo.join(name))
                .and_then(|file| {
                    file.set_modified(std::time::UNIX_EPOCH + Duration::from_secs(1_000_000_000))
                })
                .expect("time stamp should change");
        }

        // The user's global config is trusted. Use a test file in its place.
        let run = |args: &[&str], distrust: bool, input: &str| {
            use std::io::Write as _;

            let mut command = Command::new(GIT_PATH);
            command.current_dir(&repo);
            scrub_git_environment(&mut command).expect("git environment should scrub");
            command
                .env("GIT_CONFIG_GLOBAL", &global)
                .env("GIT_CONFIG_NOSYSTEM", "1");
            if distrust {
                distrust_repository_commands(&mut command).expect("repository config should read");
            }
            let mut child = command
                .args(args)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .expect("git should start");
            child
                .stdin
                .take()
                .expect("git stdin should open")
                .write_all(input.as_bytes())
                .expect("git input should write");
            child.wait_with_output().expect("git should finish")
        };
        let status = ["status", "--porcelain"];
        let credential = ["credential", "fill"];
        let credential_input = "protocol=https\nhost=halv.invalid\n\n";
        let ssh = ["ls-remote", "ssh://halv.invalid/repo.git"];

        let output = run(&status, true, "");
        assert!(
            output.status.success(),
            "status failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(global_filter_ran.exists(), "a global filter must still run");
        run(&credential, true, credential_input);
        assert!(global_helper_ran.exists(), "a global helper must still run");
        run(&ssh, true, "");
        assert!(
            global_ssh_ran.exists(),
            "a global ssh command must still run"
        );
        assert!(
            !repository_ran.exists(),
            "managed Git must not run programs from the repository config"
        );

        for (args, input) in [
            (&credential[..], credential_input),
            (&ssh[..], ""),
            (&status[..], ""),
        ] {
            run(args, false, input);
            assert!(
                repository_ran.exists(),
                "git {args:?} should run the planted program without the override"
            );
            std::fs::remove_file(&repository_ran).expect("sentinel should remove");
        }

        let _ = std::fs::remove_dir_all(base);
    }

    #[cfg(unix)]
    #[test]
    fn git_status_does_not_start_git_in_submodules() {
        let base = std::env::temp_dir().join(format!("halv-git-submodule-{}", new_uuid_v4()));
        let repo = base.join("repo");
        let sentinel = base.join("submodule-program-ran");
        std::fs::create_dir_all(&repo).expect("repo dir should create");
        git(&repo, &["init", "-b", "main"]);
        git(&repo, &["config", "user.email", "test@example.com"]);
        git(&repo, &["config", "user.name", "Halv Test"]);
        std::fs::write(repo.join("README.md"), "hello\n").expect("readme should write");
        git(&repo, &["add", "README.md"]);
        git(&repo, &["commit", "-m", "initial"]);
        plant_submodule_program(&repo, &sentinel);
        git(&repo, &["config", "submodule.recurse", "true"]);

        let status = git_status(&repo);
        assert!(status.is_git_repo);
        assert_eq!(status.clean, Some(true));
        assert!(
            !sentinel.exists(),
            "managed Git must not start git in a submodule"
        );

        let recurse = managed_git_command(&repo)
            .expect("managed Git should prepare")
            .args(["config", "--get", "submodule.recurse"])
            .output()
            .expect("git config should run");
        assert_eq!(String::from_utf8_lossy(&recurse.stdout).trim(), "false");

        git(&repo, &["status", "--porcelain"]);
        assert!(
            sentinel.exists(),
            "plain Git should run the submodule filter"
        );

        let _ = std::fs::remove_dir_all(base);
    }

    #[cfg(unix)]
    #[test]
    fn automatic_branch_creation_disables_repository_hooks() {
        use std::os::unix::fs::PermissionsExt;

        let repo = std::env::temp_dir().join(format!("halv-git-hooks-{}", new_uuid_v4()));
        let sentinel = repo.join("post-checkout-ran");
        std::fs::create_dir_all(&repo).expect("repo dir should create");
        git(&repo, &["init", "-b", "main"]);
        git(&repo, &["config", "user.email", "test@example.com"]);
        git(&repo, &["config", "user.name", "Halv Test"]);
        std::fs::write(repo.join("README.md"), "hello\n").expect("readme should write");
        git(&repo, &["add", "README.md"]);
        git(&repo, &["commit", "-m", "initial"]);
        let hook = repo.join(".git/hooks/post-checkout");
        std::fs::write(&hook, format!("#!/bin/sh\n: > '{}'\n", sentinel.display()))
            .expect("post-checkout hook should write");
        let mut permissions = std::fs::metadata(&hook).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&hook, permissions).unwrap();

        match prepare_first_write(&repo, "safe branch") {
            FirstWriteAction::Proceed(status) => {
                assert_eq!(
                    status.auto_created_branch.as_deref(),
                    Some("halv/safe-branch")
                );
            }
            other => panic!("clean default branch should auto-branch, got {other:?}"),
        }
        assert!(
            !sentinel.exists(),
            "managed Git must disable post-checkout hooks"
        );

        let _ = std::fs::remove_dir_all(repo);
    }

    #[test]
    fn approval_classifier_auto_runs_only_simple_read_only_commands() {
        fn request(command: &str) -> ShellCommandRequest {
            ShellCommandRequest {
                command: command.to_string(),
                workdir: None,
                timeout_ms: None,
                login: Some(false),
                sandbox_permissions: SandboxPermissions::UseDefault,
                justification: None,
                prefix_rule: None,
                additional_permissions: None,
            }
        }

        for command in [
            "pwd",
            "ls -la src-tauri/src",
            "rg -n 'needle' src-tauri/src",
            "git status --short",
            "git diff -- src-tauri/src/lib.rs",
            "grep -rn needle . 2>/dev/null",
            "echo hi >/dev/null 2>&1",
        ] {
            assert!(
                !needs_approval(&request(command)),
                "read-only command should auto-run: {command}"
            );
        }

        for command in [
            "python3 -c 'open(\"owned\", \"w\").write(\"x\")'",
            "node -e 'require(\"fs\").writeFileSync(\"owned\", \"x\")'",
            "git checkout -- README.md",
            "cargo check",
            "./scripts/check.sh",
            "rg --pre='python3 writer.py' needle .",
            "rg --hostname-bin=writer needle .",
            "rg needle *",
            "git status; rm -rf src",
            "echo hi > out.txt",
            "echo hi >> /tmp/out.txt",
            "echo hi 3>/dev/fd/3",
        ] {
            assert!(
                needs_approval(&request(command)),
                "unknown or write-capable command should require approval: {command}"
            );
        }

        let mut login_shell = request("git status --short");
        login_shell.login = Some(true);
        assert!(needs_approval(&login_shell));

        let mut extended_timeout = request("cat README.md");
        extended_timeout.timeout_ms = Some(DEFAULT_TIMEOUT_MS + 1);
        assert!(needs_approval(&extended_timeout));
        assert!(requests_additional_execution_authority(&extended_timeout));
        assert!(validate_shell_command_request(&extended_timeout).is_ok());

        let mut excessive_timeout = request("cat README.md");
        excessive_timeout.timeout_ms = Some(u64::MAX);
        assert!(validate_shell_command_request(&excessive_timeout)
            .unwrap_err()
            .contains(&MAX_TIMEOUT_MS.to_string()));
        let plan = build_exec_plan(excessive_timeout, "/repo", rtk_database_path());
        assert_eq!(plan.timeout_ms, MAX_TIMEOUT_MS);
    }

    #[test]
    fn output_envelope_is_middle_truncated_to_model_limit() {
        let body = format!("{}{}{}", "a".repeat(7000), "middle", "z".repeat(7000));
        let (envelope, truncated) =
            format_model_output(0, Duration::from_millis(1234), body.as_bytes());
        assert!(truncated);
        assert!(envelope.starts_with("Exit code: 0\nWall time: 1.234 seconds\n"));
        assert!(envelope.contains("[Total output lines: 1]\nOutput:\n"));
        assert!(envelope.contains("[... middle truncated ...]"));
        assert!(envelope.contains(&"a".repeat(1000)));
        assert!(envelope.contains(&"z".repeat(1000)));
        assert!(envelope.len() <= MODEL_OUTPUT_LIMIT_BYTES + 80);
    }

    #[cfg(unix)]
    #[test]
    fn rtk_routes_supported_rewrites_and_preserves_cwd_db_and_output_path() {
        for rewrite_exit in [0, 3] {
            let root = std::env::temp_dir()
                .join(format!("halv-rtk-route-{rewrite_exit}-{}", new_uuid_v4()));
            std::fs::create_dir_all(&root).expect("test cwd should create");
            let rtk = write_fake_rtk(
                &root.join("bin/rtk"),
                &format!(
                    "#!/bin/sh\nif [ \"$1\" = rewrite ]; then\n  printf 'rtk wrapped-command'\n  exit {rewrite_exit}\nfi\nprintf 'WRAPPED cwd=%s db=%s telemetry=%s args=%s\\n' \"$PWD\" \"$RTK_DB_PATH\" \"$RTK_TELEMETRY_DISABLED\" \"$*\"\n"
                ),
            );
            let streamed = Arc::new(Mutex::new(Vec::<u8>::new()));
            let streamed_for_callback = streamed.clone();
            let result = run_shell_command_blocking(
                ExecPlan {
                    command: "printf ORIGINAL_SHOULD_NOT_RUN".to_string(),
                    cwd: root.clone(),
                    timeout_ms: 5_000,
                    login: false,
                    rtk_binary: Some(rtk),
                    rtk_database_path: rtk_database_path(),
                },
                SandboxMode::DangerFullAccess,
                WorkspaceRoots::for_cwd(&root),
                Arc::new(AtomicBool::new(false)),
                OutputCallback::new(move |chunk| {
                    streamed_for_callback
                        .lock()
                        .unwrap()
                        .extend_from_slice(chunk)
                }),
            )
            .expect("rewritten command should execute");

            assert_eq!(result.exit_code, 0);
            assert_eq!(result.rtk_route, Some(RtkRouteKind::Rewritten));
            assert!(!result.captured_output.contains("ORIGINAL_SHOULD_NOT_RUN"));
            let canonical_root = root.canonicalize().unwrap();
            assert!(result
                .captured_output
                .contains(&format!("cwd={}", canonical_root.display())));
            assert!(result
                .captured_output
                .contains(&format!("db={}", rtk_database_path().display())));
            assert!(result.captured_output.contains("telemetry=1"));
            assert!(result.captured_output.contains("args=wrapped-command"));
            assert_eq!(
                String::from_utf8(streamed.lock().unwrap().clone()).unwrap(),
                result.captured_output
            );
            let _ = std::fs::remove_dir_all(root);
        }
    }

    #[cfg(unix)]
    #[test]
    fn rtk_rewrite_timeout_is_observable_and_falls_back() {
        let root = std::env::temp_dir().join(format!("halv-rtk-timeout-{}", new_uuid_v4()));
        std::fs::create_dir_all(&root).expect("test directory should create");
        let rtk = write_fake_rtk(
            &root.join("bin/rtk"),
            "#!/bin/sh\nif [ \"$1\" = rewrite ]; then sleep 5; exit 0; fi\nexit 97\n",
        );

        let attempt = try_rewrite_shell_command_with_rtk(
            "printf ORIGINAL_COMMAND",
            &root,
            rtk,
            root.join("history.db"),
            &WorkspaceRoots::for_cwd(&root),
            &AtomicBool::new(false),
            RtkRewriteProbeOptions {
                sandbox_mode: SandboxMode::DangerFullAccess,
                timeout: Duration::from_millis(50),
            },
        )
        .expect("timed-out rewrite probe should fall back");

        assert!(attempt.route.is_none());
        let warning = attempt
            .warning
            .as_deref()
            .expect("timed-out rewrite probe should provide a warning");
        assert!(warning.contains("RTK rewrite timed out after"));
        assert!(warning.contains("the timeout was 50ms"));
        assert!(warning.contains("The command ran without a rewrite."));
        assert!(attempt.log_warning().is_none());
        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[test]
    fn explicitly_wrapped_rtk_command_uses_the_bundled_runtime_environment() {
        let root = std::env::temp_dir().join(format!("halv-rtk-explicit-{}", new_uuid_v4()));
        std::fs::create_dir_all(&root).expect("test cwd should create");
        let rtk = write_fake_rtk(
            &root.join("bundle/rtk-pinned"),
            "#!/bin/sh\nif [ \"$1\" = rewrite ]; then printf '%s' \"$2\"; exit 3; fi\nprintf 'EXPLICIT args=%s db=%s\\n' \"$*\" \"$RTK_DB_PATH\"\n",
        );

        let result = run_shell_command_blocking(
            ExecPlan {
                command: "rtk already-wrapped".to_string(),
                cwd: root.clone(),
                timeout_ms: 5_000,
                login: false,
                rtk_binary: Some(rtk),
                rtk_database_path: rtk_database_path(),
            },
            SandboxMode::DangerFullAccess,
            WorkspaceRoots::for_cwd(&root),
            Arc::new(AtomicBool::new(false)),
            OutputCallback::new(|_| {}),
        )
        .expect("explicit RTK command should use the bundled launcher");

        assert_eq!(result.exit_code, 0);
        assert_eq!(result.rtk_route, Some(RtkRouteKind::Rewritten));
        assert!(result
            .captured_output
            .contains("EXPLICIT args=already-wrapped"));
        assert!(result
            .captured_output
            .contains(&format!("db={}", rtk_database_path().display())));
        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[test]
    fn rtk_decline_uses_tracked_proxy_and_unavailable_binary_falls_back() {
        let root = std::env::temp_dir().join(format!("halv-rtk-fallback-{}", new_uuid_v4()));
        std::fs::create_dir_all(&root).expect("test cwd should create");
        let marker = root.join("rtk-proxied");
        let rtk = write_fake_rtk(
            &root.join("bin/rtk"),
            &format!(
                "#!/bin/sh\nif [ \"$1\" = rewrite ]; then exit 1; fi\nif [ \"$1\" = proxy ]; then\n  shift\n  if [ \"$1\" = -- ]; then shift; fi\n  : > {}\n  exec \"$@\"\nfi\nexit 97\n",
                shell_single_quote(&marker.to_string_lossy())
            ),
        );

        for (binary, expect_proxy) in [(Some(rtk), true), (None, false)] {
            let _ = std::fs::remove_file(&marker);
            let result = run_shell_command_blocking(
                ExecPlan {
                    command: "printf DIRECT_FALLBACK".to_string(),
                    cwd: root.clone(),
                    timeout_ms: 5_000,
                    login: false,
                    rtk_binary: binary,
                    rtk_database_path: rtk_database_path(),
                },
                SandboxMode::DangerFullAccess,
                WorkspaceRoots::for_cwd(&root),
                Arc::new(AtomicBool::new(false)),
                OutputCallback::new(|_| {}),
            )
            .expect("fallback command should execute");
            assert_eq!(result.exit_code, 0);
            assert_eq!(result.captured_output, "DIRECT_FALLBACK");
            assert_eq!(marker.exists(), expect_proxy);
            assert_eq!(
                result.rtk_route,
                expect_proxy.then_some(RtkRouteKind::Tracked)
            );
        }
        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[test]
    fn login_shells_and_path_assignments_use_tracked_rtk_proxy() {
        let root = std::env::temp_dir().join(format!("halv-rtk-ineligible-{}", new_uuid_v4()));
        std::fs::create_dir_all(&root).expect("test cwd should create");
        let marker = root.join("rtk-invoked");
        let rtk = write_fake_rtk(
            &root.join("bin/rtk"),
            &format!(
                "#!/bin/sh\nif [ \"$1\" = proxy ]; then\n  printf '%s\\n' \"$*\" > {}\n  shift\n  if [ \"$1\" = -- ]; then shift; fi\n  exec \"$@\"\nfi\nexit 97\n",
                shell_single_quote(&marker.to_string_lossy())
            ),
        );

        for (command, login) in [
            ("printf LOGIN_DIRECT", true),
            ("PATH=/bin printf PATH_DIRECT", false),
            ("env PATH=/usr/bin /usr/bin/printf ENV_PATH_DIRECT", false),
        ] {
            let _ = std::fs::remove_file(&marker);
            let result = run_shell_command_blocking(
                ExecPlan {
                    command: command.to_string(),
                    cwd: root.clone(),
                    timeout_ms: 5_000,
                    login,
                    rtk_binary: Some(rtk.clone()),
                    rtk_database_path: rtk_database_path(),
                },
                SandboxMode::DangerFullAccess,
                WorkspaceRoots::for_cwd(&root),
                Arc::new(AtomicBool::new(false)),
                OutputCallback::new(|_| {}),
            )
            .expect("command should execute through RTK's tracked proxy");
            assert_eq!(result.exit_code, 0, "output={:?}", result.captured_output);
            assert_eq!(result.rtk_route, Some(RtkRouteKind::Tracked));
            assert!(
                marker.exists(),
                "RTK must run from the first turn for {command}"
            );
            assert!(
                std::fs::read_to_string(&marker)
                    .unwrap()
                    .starts_with("proxy -- /bin/bash"),
                "unexpected RTK invocation for {command}"
            );
        }
        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[test]
    fn cancellation_during_rtk_rewrite_stops_before_original_command() {
        let root = std::env::temp_dir().join(format!("halv-rtk-cancel-{}", new_uuid_v4()));
        std::fs::create_dir_all(&root).expect("test cwd should create");
        let marker = root.join("original-ran");
        let rtk = write_fake_rtk(
            &root.join("bin/rtk"),
            "#!/bin/sh\nif [ \"$1\" = rewrite ]; then sleep 5; printf 'rtk noop'; exit 0; fi\n",
        );
        let cancel = Arc::new(AtomicBool::new(false));
        let cancel_later = cancel.clone();
        let toggler = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(100));
            cancel_later.store(true, Ordering::SeqCst);
        });
        let started = Instant::now();
        let result = run_shell_command_blocking(
            ExecPlan {
                command: format!(": > {}", shell_quote(&marker)),
                cwd: root.clone(),
                timeout_ms: 5_000,
                login: false,
                rtk_binary: Some(rtk),
                rtk_database_path: rtk_database_path(),
            },
            SandboxMode::DangerFullAccess,
            WorkspaceRoots::for_cwd(&root),
            cancel,
            OutputCallback::new(|_| {}),
        );
        toggler.join().unwrap();
        assert!(matches!(result, Err(ExecError::Cancelled)));
        assert!(started.elapsed() < Duration::from_secs(2));
        assert!(!marker.exists());
        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn bundled_rtk_records_a_sandboxed_real_command_in_the_shared_database() {
        let Some(rtk) = staged_rtk_binary_path().and_then(executable_rtk_file) else {
            panic!("the pinned RTK external binary must be staged before Rust tests");
        };
        let cwd = std::env::current_dir().expect("current dir should resolve");
        let before = rtk_command_count();
        let result = run_shell_command_blocking(
            ExecPlan {
                command: "git status --short".to_string(),
                cwd: cwd.clone(),
                timeout_ms: 10_000,
                login: false,
                rtk_binary: Some(rtk),
                rtk_database_path: rtk_database_path(),
            },
            SandboxMode::ReadOnly,
            WorkspaceRoots::for_cwd(&cwd),
            Arc::new(AtomicBool::new(false)),
            OutputCallback::new(|_| {}),
        )
        .expect("sandboxed RTK command should execute");
        if result.exit_code == 71
            && result
                .captured_output
                .contains("sandbox_apply: Operation not permitted")
        {
            eprintln!(
                "sandbox-exec is blocked by the outer test environment; RTK accounting assertion skipped"
            );
            return;
        }
        assert_eq!(result.exit_code, 0, "output={:?}", result.captured_output);
        assert!(result.rtk_route.is_some());
        assert!(
            rtk_command_count() > before,
            "RTK must persist this invocation where managed Headroom can poll it"
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn bundled_rtk_tracks_an_unsupported_first_prompt_command() {
        let Some(rtk) = staged_rtk_binary_path().and_then(executable_rtk_file) else {
            panic!("the pinned RTK external binary must be staged before Rust tests");
        };
        let cwd = std::env::current_dir().expect("current dir should resolve");
        let before = rtk_command_count();
        let result = run_shell_command_blocking(
            ExecPlan {
                // `printf` has no lossy RTK rewrite, so this exercises the
                // lossless tracked-proxy route used from the first prompt.
                command: "printf FIRST_PROMPT_RTK".to_string(),
                cwd: cwd.clone(),
                timeout_ms: 10_000,
                login: false,
                rtk_binary: Some(rtk),
                rtk_database_path: rtk_database_path(),
            },
            SandboxMode::ReadOnly,
            WorkspaceRoots::for_cwd(&cwd),
            Arc::new(AtomicBool::new(false)),
            OutputCallback::new(|_| {}),
        )
        .expect("unsupported first-prompt command should execute through RTK proxy");
        if result.exit_code == 71
            && result
                .captured_output
                .contains("sandbox_apply: Operation not permitted")
        {
            eprintln!(
                "sandbox-exec is blocked by the outer test environment; RTK accounting assertion skipped"
            );
            return;
        }
        assert_eq!(result.exit_code, 0, "output={:?}", result.captured_output);
        assert_eq!(result.rtk_route, Some(RtkRouteKind::Tracked));
        assert_eq!(result.captured_output, "FIRST_PROMPT_RTK");
        assert!(
            rtk_command_count() > before,
            "RTK must track unsupported commands from the first prompt"
        );
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn macos_real_sandbox_runs_echo_and_denies_writes() {
        let cwd = std::env::current_dir()
            .expect("current dir should resolve")
            .to_string_lossy()
            .to_string();
        let cancel = Arc::new(AtomicBool::new(false));
        let output = OutputCallback::new(|_| {});

        let echo = run_shell_command(
            ShellCommandRequest {
                command: "echo hi".to_string(),
                workdir: None,
                timeout_ms: Some(10_000),
                login: Some(false),
                sandbox_permissions: SandboxPermissions::UseDefault,
                justification: None,
                prefix_rule: None,
                additional_permissions: None,
            },
            cwd.clone(),
            cancel.clone(),
            output.clone(),
        )
        .await
        .expect("echo command should execute");

        if echo.exit_code == 71
            && echo
                .captured_output
                .contains("sandbox_apply: Operation not permitted")
        {
            eprintln!("sandbox-exec is blocked by the outer test environment; real Seatbelt assertions skipped");
            return;
        }

        assert_eq!(echo.exit_code, 0);
        assert!(echo.captured_output.contains("hi"));

        let dev_null = run_shell_command(
            ShellCommandRequest {
                command: "echo x > /dev/null".to_string(),
                workdir: None,
                timeout_ms: Some(10_000),
                login: Some(false),
                sandbox_permissions: SandboxPermissions::UseDefault,
                justification: None,
                prefix_rule: None,
                additional_permissions: None,
            },
            cwd.clone(),
            cancel.clone(),
            output.clone(),
        )
        .await
        .expect("/dev/null redirect should execute inside sandbox");
        println!(
            "SANDBOX_DEV_NULL_ALLOWED exit={} output={:?}",
            dev_null.exit_code, dev_null.captured_output
        );
        assert_eq!(dev_null.exit_code, 0);

        let target = std::env::temp_dir().join(format!("halv-denied-{}", new_uuid_v4()));
        let touch = run_shell_command(
            ShellCommandRequest {
                command: format!("touch {}", target.display()),
                workdir: None,
                timeout_ms: Some(10_000),
                login: Some(false),
                sandbox_permissions: SandboxPermissions::UseDefault,
                justification: None,
                prefix_rule: None,
                additional_permissions: None,
            },
            cwd,
            cancel,
            output,
        )
        .await
        .expect("touch command should execute inside sandbox");
        println!(
            "SANDBOX_REAL_WRITE_DENIED exit={} output={:?} target_exists={}",
            touch.exit_code,
            touch.captured_output,
            target.exists()
        );

        assert_ne!(touch.exit_code, 0);
        assert!(
            !target.exists(),
            "read-only sandbox must deny writes to {}",
            target.display()
        );
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn macos_workspace_write_real_probes_allow_cwd_and_deny_git_outside_and_network() {
        let cwd_path = std::env::temp_dir().join(format!("halv-workspace-{}", new_uuid_v4()));
        std::fs::create_dir_all(cwd_path.join(".git")).expect("workspace dirs should create");
        let cwd = cwd_path.to_string_lossy().to_string();
        let roots = WorkspaceRoots::for_cwd(&cwd_path);
        let cancel = Arc::new(AtomicBool::new(false));
        let output = OutputCallback::new(|_| {});

        let allowed = run_shell_command_sandboxed(
            ShellCommandRequest {
                command: "printf ok > allowed.txt".to_string(),
                workdir: None,
                timeout_ms: Some(10_000),
                login: Some(false),
                sandbox_permissions: SandboxPermissions::UseDefault,
                justification: None,
                prefix_rule: None,
                additional_permissions: None,
            },
            cwd.clone(),
            SandboxMode::WorkspaceWrite,
            Some(roots.clone()),
            cancel.clone(),
            output.clone(),
        )
        .await
        .expect("allowed workspace write probe should execute");
        if allowed.exit_code == 71
            && allowed
                .captured_output
                .contains("sandbox_apply: Operation not permitted")
        {
            eprintln!("sandbox-exec is blocked by the outer test environment; workspace-write assertions skipped");
            let _ = std::fs::remove_dir_all(cwd_path);
            return;
        }
        println!(
            "WORKSPACE_WRITE_CWD_ALLOWED exit={} output={:?} exists={}",
            allowed.exit_code,
            allowed.captured_output,
            cwd_path.join("allowed.txt").exists()
        );
        assert_eq!(allowed.exit_code, 0);
        assert_eq!(
            std::fs::read_to_string(cwd_path.join("allowed.txt")).unwrap(),
            "ok"
        );

        let git_denied = run_shell_command_sandboxed(
            ShellCommandRequest {
                command: "printf nope > .git/x".to_string(),
                workdir: None,
                timeout_ms: Some(10_000),
                login: Some(false),
                sandbox_permissions: SandboxPermissions::UseDefault,
                justification: None,
                prefix_rule: None,
                additional_permissions: None,
            },
            cwd.clone(),
            SandboxMode::WorkspaceWrite,
            Some(roots.clone()),
            cancel.clone(),
            output.clone(),
        )
        .await
        .expect(".git denial probe should execute");
        println!(
            "WORKSPACE_WRITE_GIT_DENIED exit={} output={:?} exists={}",
            git_denied.exit_code,
            git_denied.captured_output,
            cwd_path.join(".git/x").exists()
        );
        assert_ne!(git_denied.exit_code, 0);
        assert!(!cwd_path.join(".git/x").exists());

        let outside_name = format!("halv-outside-denied-{}", new_uuid_v4());
        let outside_path = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir)
            .join(&outside_name);
        let outside_denied = run_shell_command_sandboxed(
            ShellCommandRequest {
                command: format!("printf nope > \"$HOME/{outside_name}\""),
                workdir: None,
                timeout_ms: Some(10_000),
                login: Some(false),
                sandbox_permissions: SandboxPermissions::UseDefault,
                justification: None,
                prefix_rule: None,
                additional_permissions: None,
            },
            cwd.clone(),
            SandboxMode::WorkspaceWrite,
            Some(roots.clone()),
            cancel.clone(),
            output.clone(),
        )
        .await
        .expect("outside denial probe should execute");
        println!(
            "WORKSPACE_WRITE_OUTSIDE_DENIED exit={} output={:?} exists={}",
            outside_denied.exit_code,
            outside_denied.captured_output,
            outside_path.exists()
        );
        assert_ne!(outside_denied.exit_code, 0);
        assert!(!outside_path.exists());

        let network_denied = run_shell_command_sandboxed(
            ShellCommandRequest {
                command: "python3 -c 'import socket; socket.create_connection((\"1.1.1.1\", 80), timeout=1)'".to_string(),
                workdir: None,
                timeout_ms: Some(10_000),
                login: Some(false),
                sandbox_permissions: SandboxPermissions::UseDefault,
                justification: None,
                prefix_rule: None,
                additional_permissions: None,
            },
            cwd.clone(),
            SandboxMode::WorkspaceWrite,
            Some(roots),
            cancel,
            output,
        )
        .await
        .expect("network denial probe should execute");
        println!(
            "WORKSPACE_WRITE_NETWORK_DENIED exit={} output={:?}",
            network_denied.exit_code, network_denied.captured_output
        );
        assert_ne!(network_denied.exit_code, 0);

        let _ = std::fs::remove_dir_all(cwd_path);
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn linux_live_bwrap_read_only_denies_workspace_write() {
        if linux_bwrap_unavailable() {
            return;
        }

        let cwd_path = linux_live_workspace("readonly");
        let cwd = cwd_path.to_string_lossy().to_string();
        let roots = WorkspaceRoots::new(vec![cwd_path.clone()]);
        let cancel = Arc::new(AtomicBool::new(false));
        let output = OutputCallback::new(|_| {});

        let echo = run_shell_command_sandboxed(
            shell_request("echo hi"),
            cwd.clone(),
            SandboxMode::ReadOnly,
            Some(roots.clone()),
            cancel.clone(),
            output.clone(),
        )
        .await
        .expect("read-only bwrap echo probe should execute");
        if linux_bwrap_blocked_by_outer_environment(&echo) {
            eprintln!("bubblewrap is blocked by the outer test environment; Linux live assertions skipped");
            let _ = std::fs::remove_dir_all(cwd_path);
            return;
        }
        assert_eq!(echo.exit_code, 0);
        assert!(echo.captured_output.contains("hi"));

        let denied = run_shell_command_sandboxed(
            shell_request("printf nope > denied.txt"),
            cwd,
            SandboxMode::ReadOnly,
            Some(roots),
            cancel,
            output,
        )
        .await
        .expect("read-only write denial probe should execute");
        println!(
            "LINUX_BWRAP_READ_ONLY_WRITE_DENIED exit={} output={:?} exists={}",
            denied.exit_code,
            denied.captured_output,
            cwd_path.join("denied.txt").exists()
        );
        assert_ne!(denied.exit_code, 0);
        assert!(!cwd_path.join("denied.txt").exists());

        let _ = std::fs::remove_dir_all(cwd_path);
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn linux_live_bwrap_workspace_write_allows_workspace_denies_protected_and_network() {
        if linux_bwrap_unavailable() {
            return;
        }

        let cwd_path = linux_live_workspace("workspace");
        std::fs::create_dir_all(cwd_path.join(".git")).expect("protected dir should create");
        let cwd = cwd_path.to_string_lossy().to_string();
        let roots = WorkspaceRoots::new(vec![cwd_path.clone()]);
        let cancel = Arc::new(AtomicBool::new(false));
        let output = OutputCallback::new(|_| {});

        let allowed = run_shell_command_sandboxed(
            shell_request("printf ok > allowed.txt"),
            cwd.clone(),
            SandboxMode::WorkspaceWrite,
            Some(roots.clone()),
            cancel.clone(),
            output.clone(),
        )
        .await
        .expect("workspace write probe should execute");
        if linux_bwrap_blocked_by_outer_environment(&allowed) {
            eprintln!("bubblewrap is blocked by the outer test environment; Linux live assertions skipped");
            let _ = std::fs::remove_dir_all(cwd_path);
            return;
        }
        println!(
            "LINUX_BWRAP_WORKSPACE_WRITE_ALLOWED exit={} output={:?} exists={}",
            allowed.exit_code,
            allowed.captured_output,
            cwd_path.join("allowed.txt").exists()
        );
        assert_eq!(allowed.exit_code, 0);
        assert_eq!(
            std::fs::read_to_string(cwd_path.join("allowed.txt")).unwrap(),
            "ok"
        );

        let protected = run_shell_command_sandboxed(
            shell_request("printf nope > .git/x"),
            cwd.clone(),
            SandboxMode::WorkspaceWrite,
            Some(roots.clone()),
            cancel.clone(),
            output.clone(),
        )
        .await
        .expect("protected write denial probe should execute");
        println!(
            "LINUX_BWRAP_PROTECTED_WRITE_DENIED exit={} output={:?} exists={}",
            protected.exit_code,
            protected.captured_output,
            cwd_path.join(".git/x").exists()
        );
        assert_ne!(protected.exit_code, 0);
        assert!(!cwd_path.join(".git/x").exists());

        let network = run_shell_command_sandboxed(
            shell_request(
                "python3 -c 'import socket; socket.create_connection((\"1.1.1.1\", 80), timeout=1)'",
            ),
            cwd,
            SandboxMode::WorkspaceWrite,
            Some(roots),
            cancel,
            output,
        )
        .await
        .expect("network denial probe should execute");
        println!(
            "LINUX_BWRAP_NETWORK_DENIED exit={} output={:?}",
            network.exit_code, network.captured_output
        );
        assert_ne!(network.exit_code, 0);

        let _ = std::fs::remove_dir_all(cwd_path);
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn macos_approved_out_of_root_policy_allows_approved_path_and_denies_neighbor() {
        let cwd_path =
            std::env::temp_dir().join(format!("halv-approved-policy-root-{}", new_uuid_v4()));
        let outside =
            std::env::temp_dir().join(format!("halv-approved-policy-outside-{}", new_uuid_v4()));
        std::fs::create_dir_all(&cwd_path).expect("workspace dirs should create");
        std::fs::create_dir_all(&outside).expect("outside dir should create");
        let approved_path = outside
            .canonicalize()
            .expect("outside dir should canonicalize")
            .join("approved.txt");
        std::fs::write(&approved_path, "").expect("approved file should create");
        let neighbor_path = outside
            .canonicalize()
            .expect("outside dir should canonicalize")
            .join("neighbor.txt");
        let approved_display = approved_path.to_string_lossy().to_string();
        let roots = WorkspaceRoots::new(vec![cwd_path.clone()])
            .approved_patch_roots(&cwd_path, std::slice::from_ref(&approved_display))
            .expect("approved path roots should build");
        let cwd = cwd_path.to_string_lossy().to_string();
        let cancel = Arc::new(AtomicBool::new(false));
        let output = OutputCallback::new(|_| {});

        let allowed = run_shell_command_sandboxed(
            ShellCommandRequest {
                command: format!("printf ok > {}", shell_quote(&approved_path)),
                workdir: None,
                timeout_ms: Some(10_000),
                login: Some(false),
                sandbox_permissions: SandboxPermissions::UseDefault,
                justification: None,
                prefix_rule: None,
                additional_permissions: None,
            },
            cwd.clone(),
            SandboxMode::WorkspaceWrite,
            Some(roots.clone()),
            cancel.clone(),
            output.clone(),
        )
        .await
        .expect("approved write probe should execute");
        if allowed.exit_code == 71
            && allowed
                .captured_output
                .contains("sandbox_apply: Operation not permitted")
        {
            eprintln!("sandbox-exec is blocked by the outer test environment; approved path sandbox assertions skipped");
            let _ = std::fs::remove_dir_all(cwd_path);
            let _ = std::fs::remove_dir_all(outside);
            return;
        }
        println!(
            "APPROVED_OUT_OF_ROOT_SANDBOX_ALLOWED exit={} output={:?} approved_contents={:?}",
            allowed.exit_code,
            allowed.captured_output,
            std::fs::read_to_string(&approved_path).unwrap_or_default()
        );
        assert_eq!(allowed.exit_code, 0);
        assert_eq!(std::fs::read_to_string(&approved_path).unwrap(), "ok");

        let neighbor = run_shell_command_sandboxed(
            ShellCommandRequest {
                command: format!("printf neighbor > {}", shell_quote(&neighbor_path)),
                workdir: None,
                timeout_ms: Some(10_000),
                login: Some(false),
                sandbox_permissions: SandboxPermissions::UseDefault,
                justification: None,
                prefix_rule: None,
                additional_permissions: None,
            },
            cwd,
            SandboxMode::WorkspaceWrite,
            Some(roots),
            cancel,
            output,
        )
        .await
        .expect("neighbor denial probe should execute");
        println!(
            "APPROVED_OUT_OF_ROOT_NEIGHBOR_SANDBOX_DENIED exit={} output={:?} neighbor_exists={}",
            neighbor.exit_code,
            neighbor.captured_output,
            neighbor_path.exists()
        );
        assert_ne!(neighbor.exit_code, 0);
        assert!(!neighbor_path.exists());

        let _ = std::fs::remove_dir_all(cwd_path);
        let _ = std::fs::remove_dir_all(outside);
    }

    #[cfg(unix)]
    fn shell_quote(path: &std::path::Path) -> String {
        format!("'{}'", path.to_string_lossy().replace('\'', "'\\''"))
    }

    #[cfg(target_os = "linux")]
    fn linux_bwrap_unavailable() -> bool {
        if command_available_on_path(bwrap::BWRAP_PATH) {
            false
        } else {
            eprintln!("bubblewrap is not on PATH; Linux live sandbox assertions skipped");
            true
        }
    }

    #[cfg(target_os = "linux")]
    fn linux_bwrap_blocked_by_outer_environment(result: &ExecResult) -> bool {
        if result.exit_code == 0 {
            return false;
        }
        let output = result.captured_output.to_ascii_lowercase();
        output.contains("operation not permitted")
            || output.contains("creating new namespace failed")
            || output.contains("no permissions to create new namespace")
    }

    #[cfg(target_os = "linux")]
    fn linux_live_workspace(label: &str) -> PathBuf {
        let base = std::env::current_dir()
            .expect("current dir should resolve")
            .join("target");
        std::fs::create_dir_all(&base).expect("target dir should create");
        let root = base.join(format!("halv-bwrap-{label}-{}", new_uuid_v4()));
        std::fs::create_dir_all(&root).expect("live workspace should create");
        root
    }

    #[cfg(target_os = "linux")]
    fn shell_request(command: &str) -> ShellCommandRequest {
        ShellCommandRequest {
            command: command.to_string(),
            workdir: None,
            timeout_ms: Some(10_000),
            login: Some(false),
            sandbox_permissions: SandboxPermissions::UseDefault,
            justification: None,
            prefix_rule: None,
            additional_permissions: None,
        }
    }

    fn strings(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| value.to_string()).collect()
    }

    fn arg_triplet_index(args: &[String], flag: &str, source: &str, dest: &str) -> Option<usize> {
        args.windows(3)
            .position(|window| window[0] == flag && window[1] == source && window[2] == dest)
    }

    #[cfg(unix)]
    fn write_fake_rtk(path: &Path, contents: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;

        std::fs::create_dir_all(path.parent().unwrap()).expect("fake rtk parent should create");
        std::fs::write(path, contents).expect("fake rtk should write");
        let mut permissions = std::fs::metadata(path).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(path, permissions).unwrap();
        path.to_path_buf()
    }

    #[cfg(target_os = "macos")]
    fn rtk_command_count() -> i64 {
        let path = rtk_database_path();
        if !path.is_file() {
            return 0;
        }
        rusqlite::Connection::open(path)
            .and_then(|connection| {
                connection.query_row("SELECT COUNT(*) FROM commands", [], |row| row.get(0))
            })
            .unwrap_or(0)
    }

    fn git(cwd: &std::path::Path, args: &[&str]) {
        let output = Command::new(GIT_PATH)
            .args(args)
            .current_dir(cwd)
            .output()
            .expect("git should run");
        assert!(
            output.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn macos_setsid_descendant_does_not_hang_executor() {
        let cwd = std::env::current_dir()
            .expect("current dir should resolve")
            .to_string_lossy()
            .to_string();
        let cancel = Arc::new(AtomicBool::new(false));
        let output = OutputCallback::new(|_| {});
        let start = Instant::now();
        let result = run_shell_command(
            ShellCommandRequest {
                command: "python3 -c 'import os,time; pid=os.fork();\nif pid==0:\n os.setsid(); time.sleep(1000)\nelse:\n time.sleep(1000)'".to_string(),
                workdir: None,
                timeout_ms: Some(500),
                login: Some(false),
                sandbox_permissions: SandboxPermissions::UseDefault,
                justification: None,
                prefix_rule: None,
                additional_permissions: None,
            },
            cwd,
            cancel,
            output,
        )
        .await
        .expect("setsid regression command should return");

        if result.exit_code == 71
            && result
                .captured_output
                .contains("sandbox_apply: Operation not permitted")
        {
            eprintln!(
                "sandbox-exec is blocked by the outer test environment; setsid assertion skipped"
            );
            return;
        }

        assert!(
            start.elapsed() < Duration::from_secs(5),
            "executor should not hang draining inherited pipes"
        );
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn macos_cleanup_kills_a_sandboxed_daemon_that_left_the_shell() {
        use process_tree::test_support::{assert_daemon_gone, daemon_directory, PERL_PATH};

        let directory = daemon_directory("shell");
        let cancel = Arc::new(AtomicBool::new(false));
        let output = OutputCallback::new(|_| {});
        // The first child starts a new session, forks the daemon, and exits
        // at once. Then launchd is the parent of the daemon.
        let result = run_shell_command_sandboxed(
            ShellCommandRequest {
                command: format!("{PERL_PATH} daemon.pl . 0 0 0"),
                workdir: None,
                timeout_ms: Some(10_000),
                login: Some(false),
                sandbox_permissions: SandboxPermissions::UseDefault,
                justification: None,
                prefix_rule: None,
                additional_permissions: None,
            },
            directory.to_string_lossy().to_string(),
            SandboxMode::WorkspaceWrite,
            Some(WorkspaceRoots::for_cwd(&directory)),
            cancel,
            output,
        )
        .await
        .expect("daemon command should run");

        if result.exit_code == 71
            && result
                .captured_output
                .contains("sandbox_apply: Operation not permitted")
        {
            eprintln!(
                "sandbox-exec is blocked by the outer test environment; detached daemon assertion skipped"
            );
            let _ = std::fs::remove_dir_all(&directory);
            return;
        }
        assert_eq!(result.exit_code, 0, "{}", result.captured_output);
        assert_daemon_gone(&directory);
    }
}
