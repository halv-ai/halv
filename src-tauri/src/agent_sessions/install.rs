use super::AgentKind;
use chrono::Utc;
use flate2::read::GzDecoder;
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        RwLock,
    },
    time::{Duration, Instant, SystemTime},
};
use tauri::{AppHandle, Emitter};

const AGENT_CLI_INSTALL_PROGRESS_EVENT: &str = "agent-cli-install-progress";
const CLAUDE_RELEASE_BASE: &str = "https://storage.googleapis.com/claude-code-dist-86c565f3-f756-42ad-8dfa-d59b1c096819/claude-code-releases";
const CODEX_RELEASE_URL: &str = "https://api.github.com/repos/openai/codex/releases/latest";
const DOWNLOAD_PROGRESS_INTERVAL: Duration = Duration::from_millis(250);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);

static MANAGED_ROOT: RwLock<Option<PathBuf>> = RwLock::new(None);
static CLAUDE_INSTALLING: AtomicBool = AtomicBool::new(false);
static CODEX_INSTALLING: AtomicBool = AtomicBool::new(false);

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentCliInstallOutcome {
    pub agent_kind: AgentKind,
    pub version: String,
    pub binary: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum AgentCliInstallErrorCode {
    UnsupportedAgent,
    UnsupportedPlatform,
    AppDataUnavailable,
    ResolveFailed,
    DownloadFailed,
    ChecksumMismatch,
    ExtractFailed,
    WriteFailed,
    AlreadyInstalling,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AgentCliInstallError {
    pub code: AgentCliInstallErrorCode,
    pub message: String,
    pub details: Vec<String>,
    pub manual_hint: String,
}

impl AgentCliInstallError {
    fn new(
        agent_kind: AgentKind,
        code: AgentCliInstallErrorCode,
        message: impl Into<String>,
        details: Vec<String>,
    ) -> Self {
        Self {
            code,
            message: message.into(),
            details,
            manual_hint: manual_hint(agent_kind),
        }
    }

    fn single(
        agent_kind: AgentKind,
        code: AgentCliInstallErrorCode,
        message: impl Into<String>,
        detail: impl Into<String>,
    ) -> Self {
        Self::new(agent_kind, code, message, vec![detail.into()])
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum AgentCliInstallStage {
    Resolving,
    Downloading,
    Verifying,
    Extracting,
    Success,
    Error,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentCliInstallProgress {
    pub agent_kind: AgentKind,
    pub stage: AgentCliInstallStage,
    pub line: String,
    pub downloaded_bytes: Option<u64>,
    pub total_bytes: Option<u64>,
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum ManagedAgent {
    Claude,
    Codex,
}

impl ManagedAgent {
    fn directory_name(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
        }
    }

    fn binary_relative_path(self) -> &'static Path {
        Path::new(self.binary_relative_string())
    }

    fn binary_relative_string(self) -> &'static str {
        match self {
            #[cfg(not(windows))]
            Self::Claude => "claude",
            #[cfg(windows)]
            Self::Claude => "claude.exe",
            #[cfg(not(windows))]
            Self::Codex => "bin/codex",
            #[cfg(windows)]
            Self::Codex => "bin/codex.exe",
        }
    }

    fn display_name(self) -> &'static str {
        match self {
            Self::Claude => "Claude Code",
            Self::Codex => "Codex CLI",
        }
    }

    fn agent_kind(self) -> AgentKind {
        match self {
            Self::Claude => AgentKind::Claude,
            Self::Codex => AgentKind::Codex,
        }
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct CurrentInstall {
    version: String,
    binary: String,
    installed_at: String,
}

pub fn set_managed_root(root: PathBuf) {
    match MANAGED_ROOT.write() {
        Ok(mut managed_root) => *managed_root = Some(root),
        Err(poisoned) => *poisoned.into_inner() = Some(root),
    }
}

pub fn managed_root() -> Option<PathBuf> {
    match MANAGED_ROOT.read() {
        Ok(managed_root) => managed_root.clone(),
        Err(poisoned) => poisoned.into_inner().clone(),
    }
}

pub(crate) fn managed_binary(agent: ManagedAgent) -> Option<PathBuf> {
    let agent_root = managed_root()?.join(agent.directory_name());
    managed_binary_at(&agent_root, agent)
}

fn managed_binary_at(agent_root: &Path, agent: ManagedAgent) -> Option<PathBuf> {
    let current_path = agent_root.join("current.json");
    match fs::read_to_string(&current_path) {
        Ok(contents) => {
            if let Ok(current) = serde_json::from_str::<CurrentInstall>(&contents) {
                let binary = agent_root.join(current.binary);
                if is_executable(&binary) {
                    return Some(binary);
                }
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(_) => {}
    }

    newest_managed_binary(agent_root, agent)
}

fn newest_managed_binary(agent_root: &Path, agent: ManagedAgent) -> Option<PathBuf> {
    fs::read_dir(agent_root)
        .ok()?
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.contains(".tmp-") {
                return None;
            }
            let directory = entry.path();
            if !directory.is_dir() {
                return None;
            }
            let binary = directory.join(agent.binary_relative_path());
            if !is_executable(&binary) {
                return None;
            }
            let modified = entry
                .metadata()
                .and_then(|metadata| metadata.modified())
                .unwrap_or(SystemTime::UNIX_EPOCH);
            Some((modified, name, binary))
        })
        .max_by(|left, right| (left.0, &left.1).cmp(&(right.0, &right.1)))
        .map(|(_, _, binary)| binary)
}

fn is_executable(path: &Path) -> bool {
    let Ok(metadata) = path.metadata() else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

struct InstallGuard {
    flag: &'static AtomicBool,
}

impl InstallGuard {
    fn acquire(agent_kind: AgentKind) -> Result<Self, AgentCliInstallError> {
        let flag = match agent_kind {
            AgentKind::Claude => &CLAUDE_INSTALLING,
            AgentKind::Codex => &CODEX_INSTALLING,
            _ => {
                return Err(AgentCliInstallError::new(
                    agent_kind,
                    AgentCliInstallErrorCode::UnsupportedAgent,
                    "Halv can install only Claude Code and Codex CLI.",
                    Vec::new(),
                ));
            }
        };
        flag.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .map_err(|_| {
                AgentCliInstallError::new(
                    agent_kind,
                    AgentCliInstallErrorCode::AlreadyInstalling,
                    format!(
                        "A {} installation is already running.",
                        agent_name(agent_kind)
                    ),
                    Vec::new(),
                )
            })?;
        Ok(Self { flag })
    }
}

impl Drop for InstallGuard {
    fn drop(&mut self) {
        self.flag.store(false, Ordering::Release);
    }
}

#[derive(Clone)]
struct InstallProgress {
    app: AppHandle,
}

trait InstallProgressSink {
    fn emit(
        &self,
        agent_kind: AgentKind,
        stage: AgentCliInstallStage,
        line: impl Into<String>,
        downloaded_bytes: Option<u64>,
        total_bytes: Option<u64>,
    );
}

impl InstallProgressSink for InstallProgress {
    fn emit(
        &self,
        agent_kind: AgentKind,
        stage: AgentCliInstallStage,
        line: impl Into<String>,
        downloaded_bytes: Option<u64>,
        total_bytes: Option<u64>,
    ) {
        let _ = self.app.emit(
            AGENT_CLI_INSTALL_PROGRESS_EVENT,
            AgentCliInstallProgress {
                agent_kind,
                stage,
                line: line.into(),
                downloaded_bytes,
                total_bytes,
            },
        );
    }
}

#[tauri::command]
pub async fn agent_cli_install(
    app: AppHandle,
    agent_kind: AgentKind,
) -> Result<AgentCliInstallOutcome, AgentCliInstallError> {
    let progress = InstallProgress { app: app.clone() };
    let result = agent_cli_install_inner(&app, agent_kind, &progress).await;
    if let Err(error) = &result {
        progress.emit(
            agent_kind,
            AgentCliInstallStage::Error,
            error.message.clone(),
            None,
            None,
        );
    }
    result
}

async fn agent_cli_install_inner(
    app: &AppHandle,
    agent_kind: AgentKind,
    progress: &impl InstallProgressSink,
) -> Result<AgentCliInstallOutcome, AgentCliInstallError> {
    let _guard = InstallGuard::acquire(agent_kind)?;
    let app_data_dir = crate::app_data::resolved_app_data_dir(app).map_err(|error| {
        AgentCliInstallError::single(
            agent_kind,
            AgentCliInstallErrorCode::AppDataUnavailable,
            "Halv could not access its app data directory.",
            error,
        )
    })?;
    let managed_root = app_data_dir.join("bin");
    set_managed_root(managed_root.clone());
    let client = reqwest::Client::builder()
        .user_agent(format!("halv/{}", env!("CARGO_PKG_VERSION")))
        .connect_timeout(CONNECT_TIMEOUT)
        .build()
        .map_err(|error| {
            AgentCliInstallError::single(
                agent_kind,
                AgentCliInstallErrorCode::ResolveFailed,
                "Halv could not create the CLI download client.",
                error.to_string(),
            )
        })?;

    match agent_kind {
        AgentKind::Claude => install_claude(&client, &managed_root, progress).await,
        AgentKind::Codex => install_codex(&client, &managed_root, progress).await,
        _ => Err(AgentCliInstallError::new(
            agent_kind,
            AgentCliInstallErrorCode::UnsupportedAgent,
            "Halv can install only Claude Code and Codex CLI.",
            Vec::new(),
        )),
    }
}

#[derive(Debug, Deserialize)]
struct ClaudeManifest {
    version: String,
    platforms: HashMap<String, ClaudePlatformEntry>,
}

#[derive(Debug, Clone, Deserialize)]
struct ClaudePlatformEntry {
    binary: String,
    checksum: String,
    size: u64,
}

struct ClaudeRelease {
    version: String,
    url: String,
    checksum: String,
    size: u64,
}

async fn install_claude(
    client: &reqwest::Client,
    managed_root: &Path,
    progress: &impl InstallProgressSink,
) -> Result<AgentCliInstallOutcome, AgentCliInstallError> {
    let agent_kind = AgentKind::Claude;
    let platform_key = resolve_claude_platform().await?;
    let stable_url = format!("{CLAUDE_RELEASE_BASE}/stable");
    let version = resolve_text(
        client.get(&stable_url),
        agent_kind,
        "Halv could not resolve the latest Claude Code release.",
        &stable_url,
    )
    .await?
    .trim()
    .to_string();
    validate_version(&version, agent_kind)?;
    progress.emit(
        agent_kind,
        AgentCliInstallStage::Resolving,
        format!("Resolved Claude Code {version}."),
        None,
        None,
    );

    let manifest_url = format!("{CLAUDE_RELEASE_BASE}/{version}/manifest.json");
    let manifest_text = resolve_text(
        client.get(&manifest_url),
        agent_kind,
        "Halv could not resolve the Claude Code manifest.",
        &manifest_url,
    )
    .await?;
    let entry = claude_manifest_entry(&manifest_text, platform_key).map_err(|detail| {
        AgentCliInstallError::single(
            agent_kind,
            AgentCliInstallErrorCode::ResolveFailed,
            "Halv could not read the Claude Code manifest.",
            detail,
        )
    })?;
    let manifest_version = serde_json::from_str::<ClaudeManifest>(&manifest_text)
        .map(|manifest| manifest.version)
        .map_err(|error| {
            AgentCliInstallError::single(
                agent_kind,
                AgentCliInstallErrorCode::ResolveFailed,
                "Halv could not read the Claude Code manifest.",
                error.to_string(),
            )
        })?;
    if manifest_version != version {
        return Err(AgentCliInstallError::new(
            agent_kind,
            AgentCliInstallErrorCode::ResolveFailed,
            "The Claude Code manifest has an unexpected version.",
            vec![
                format!("Stable version: {version}"),
                format!("Manifest version: {manifest_version}"),
            ],
        ));
    }
    if entry.binary != ManagedAgent::Claude.binary_relative_string() {
        return Err(AgentCliInstallError::single(
            agent_kind,
            AgentCliInstallErrorCode::ResolveFailed,
            "The Claude Code manifest has an unexpected binary name.",
            entry.binary,
        ));
    }
    validate_checksum(&entry.checksum, agent_kind)?;
    let release = ClaudeRelease {
        url: format!(
            "{CLAUDE_RELEASE_BASE}/{version}/{platform_key}/{}",
            entry.binary
        ),
        version,
        checksum: entry.checksum,
        size: entry.size,
    };

    install_claude_release(client, managed_root, progress, release).await
}

async fn install_claude_release(
    client: &reqwest::Client,
    managed_root: &Path,
    progress: &impl InstallProgressSink,
    release: ClaudeRelease,
) -> Result<AgentCliInstallOutcome, AgentCliInstallError> {
    let agent = ManagedAgent::Claude;
    match prepare_install(managed_root, agent, &release.version).await? {
        PreparedInstall::Existing(paths) => {
            record_current(&paths, agent, &release.version).await?;
            let outcome = install_outcome(agent, release.version, paths.target_binary);
            progress.emit(
                AgentKind::Claude,
                AgentCliInstallStage::Success,
                format!("Claude Code {} is already installed.", outcome.version),
                None,
                None,
            );
            Ok(outcome)
        }
        PreparedInstall::Pending(paths) => {
            let result = async {
                download_verified(
                    client,
                    &release.url,
                    &paths.temp_binary,
                    &release.checksum,
                    Some(release.size),
                    agent,
                    progress,
                )
                .await?;
                force_executable(&paths.temp_binary, agent).await?;
                finish_install(&paths, agent, &release.version).await
            }
            .await;
            if result.is_err() {
                cleanup_temp(paths.temp_directory.clone()).await;
            }
            if let Ok(outcome) = &result {
                progress.emit(
                    AgentKind::Claude,
                    AgentCliInstallStage::Success,
                    format!("Installed Claude Code {}.", outcome.version),
                    None,
                    None,
                );
            }
            result
        }
    }
}

#[derive(Debug, Deserialize)]
struct CodexReleaseResponse {
    tag_name: String,
    assets: Vec<CodexReleaseAsset>,
}

#[derive(Debug, Deserialize)]
struct CodexReleaseAsset {
    name: String,
    browser_download_url: String,
}

struct CodexRelease {
    version: String,
    package_url: String,
    checksum: String,
}

async fn install_codex(
    client: &reqwest::Client,
    managed_root: &Path,
    progress: &impl InstallProgressSink,
) -> Result<AgentCliInstallOutcome, AgentCliInstallError> {
    let agent_kind = AgentKind::Codex;
    let triple = codex_triple(std::env::consts::OS, std::env::consts::ARCH).ok_or_else(|| {
        AgentCliInstallError::single(
            agent_kind,
            AgentCliInstallErrorCode::UnsupportedPlatform,
            "Halv does not provide Codex CLI for this platform.",
            format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH),
        )
    })?;
    let response = client
        .get(CODEX_RELEASE_URL)
        .header(reqwest::header::ACCEPT, "application/vnd.github+json")
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|error| {
            AgentCliInstallError::single(
                agent_kind,
                AgentCliInstallErrorCode::ResolveFailed,
                "Halv could not resolve the latest Codex CLI release.",
                format!("{CODEX_RELEASE_URL}: {error}"),
            )
        })?;
    let release = response
        .json::<CodexReleaseResponse>()
        .await
        .map_err(|error| {
            AgentCliInstallError::single(
                agent_kind,
                AgentCliInstallErrorCode::ResolveFailed,
                "Halv could not read the Codex CLI release.",
                error.to_string(),
            )
        })?;
    let version = release
        .tag_name
        .strip_prefix("rust-v")
        .or_else(|| release.tag_name.strip_prefix('v'))
        .unwrap_or(&release.tag_name)
        .to_string();
    validate_version(&version, agent_kind)?;
    progress.emit(
        agent_kind,
        AgentCliInstallStage::Resolving,
        format!("Resolved Codex CLI {version}."),
        None,
        None,
    );

    let package_name = format!("codex-package-{triple}.tar.gz");
    let package_url = release
        .assets
        .iter()
        .find(|asset| asset.name == package_name)
        .map(|asset| asset.browser_download_url.clone())
        .ok_or_else(|| {
            AgentCliInstallError::single(
                agent_kind,
                AgentCliInstallErrorCode::ResolveFailed,
                "The Codex CLI release does not include this platform package.",
                package_name.clone(),
            )
        })?;
    let sums_asset = release
        .assets
        .iter()
        .find(|asset| asset.name == "codex-package_SHA256SUMS")
        .ok_or_else(|| {
            AgentCliInstallError::new(
                agent_kind,
                AgentCliInstallErrorCode::ResolveFailed,
                "The Codex CLI release does not include its checksum list.",
                Vec::new(),
            )
        })?;
    let sums_text = resolve_text(
        client.get(&sums_asset.browser_download_url),
        agent_kind,
        "Halv could not resolve the Codex CLI checksum.",
        &sums_asset.browser_download_url,
    )
    .await?;
    let checksum = parse_codex_checksum(&sums_text, &package_name).ok_or_else(|| {
        AgentCliInstallError::single(
            agent_kind,
            AgentCliInstallErrorCode::ResolveFailed,
            "The Codex CLI checksum list does not include this package.",
            package_name,
        )
    })?;
    let release = CodexRelease {
        version,
        package_url,
        checksum,
    };

    install_codex_release(client, managed_root, progress, release).await
}

async fn install_codex_release(
    client: &reqwest::Client,
    managed_root: &Path,
    progress: &impl InstallProgressSink,
    release: CodexRelease,
) -> Result<AgentCliInstallOutcome, AgentCliInstallError> {
    let agent = ManagedAgent::Codex;
    match prepare_install(managed_root, agent, &release.version).await? {
        PreparedInstall::Existing(paths) => {
            record_current(&paths, agent, &release.version).await?;
            let outcome = install_outcome(agent, release.version, paths.target_binary);
            progress.emit(
                AgentKind::Codex,
                AgentCliInstallStage::Success,
                format!("Codex CLI {} is already installed.", outcome.version),
                None,
                None,
            );
            Ok(outcome)
        }
        PreparedInstall::Pending(paths) => {
            let result = async {
                download_verified(
                    client,
                    &release.package_url,
                    &paths.download_path,
                    &release.checksum,
                    None,
                    agent,
                    progress,
                )
                .await?;
                progress.emit(
                    AgentKind::Codex,
                    AgentCliInstallStage::Extracting,
                    "Extracting the Codex CLI package.",
                    None,
                    None,
                );
                extract_codex_package(paths.download_path.clone(), paths.temp_directory.clone())
                    .await?;
                force_executable(&paths.temp_binary, agent).await?;
                finish_install(&paths, agent, &release.version).await
            }
            .await;
            if result.is_err() {
                cleanup_temp(paths.temp_directory.clone()).await;
            }
            if let Ok(outcome) = &result {
                progress.emit(
                    AgentKind::Codex,
                    AgentCliInstallStage::Success,
                    format!("Installed Codex CLI {}.", outcome.version),
                    None,
                    None,
                );
            }
            result
        }
    }
}

async fn resolve_text(
    request: reqwest::RequestBuilder,
    agent_kind: AgentKind,
    message: &'static str,
    url: &str,
) -> Result<String, AgentCliInstallError> {
    let response = request
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|error| {
            AgentCliInstallError::single(
                agent_kind,
                AgentCliInstallErrorCode::ResolveFailed,
                message,
                format!("{url}: {error}"),
            )
        })?;
    response.text().await.map_err(|error| {
        AgentCliInstallError::single(
            agent_kind,
            AgentCliInstallErrorCode::ResolveFailed,
            message,
            format!("{url}: {error}"),
        )
    })
}

fn claude_manifest_entry(
    manifest_json: &str,
    platform_key: &str,
) -> Result<ClaudePlatformEntry, String> {
    let manifest =
        serde_json::from_str::<ClaudeManifest>(manifest_json).map_err(|error| error.to_string())?;
    if manifest.version.trim().is_empty() {
        return Err("The manifest version is empty.".to_string());
    }
    manifest
        .platforms
        .get(platform_key)
        .cloned()
        .ok_or_else(|| format!("The manifest does not include {platform_key}."))
}

fn parse_codex_checksum(contents: &str, asset_name: &str) -> Option<String> {
    contents.lines().find_map(|line| {
        let mut fields = line.split_whitespace();
        let checksum = fields.next()?;
        let name = fields.next()?;
        (name == asset_name && is_sha256(checksum)).then(|| checksum.to_ascii_lowercase())
    })
}

fn claude_platform_key(os: &str, arch: &str, musl: bool) -> Option<&'static str> {
    match (os, arch, musl) {
        ("darwin" | "macos", "aarch64", _) => Some("darwin-arm64"),
        ("darwin" | "macos", "x86_64", _) => Some("darwin-x64"),
        ("linux", "aarch64", true) => Some("linux-arm64-musl"),
        ("linux", "aarch64", false) => Some("linux-arm64"),
        ("linux", "x86_64", true) => Some("linux-x64-musl"),
        ("linux", "x86_64", false) => Some("linux-x64"),
        ("windows", "x86_64", _) => Some("win32-x64"),
        _ => None,
    }
}

async fn resolve_claude_platform() -> Result<&'static str, AgentCliInstallError> {
    let os = std::env::consts::OS;
    let arch = std::env::consts::ARCH;
    let musl = if os == "linux" {
        tokio::task::spawn_blocking(move || has_musl_loader(arch))
            .await
            .unwrap_or(false)
    } else {
        false
    };
    claude_platform_key(os, arch, musl).ok_or_else(|| {
        AgentCliInstallError::single(
            AgentKind::Claude,
            AgentCliInstallErrorCode::UnsupportedPlatform,
            "Halv does not provide Claude Code for this platform.",
            format!("{os}-{arch}"),
        )
    })
}

fn has_musl_loader(arch: &str) -> bool {
    match arch {
        "aarch64" => Path::new("/lib/ld-musl-aarch64.so.1").exists(),
        "x86_64" => Path::new("/lib/ld-musl-x86_64.so.1").exists(),
        _ => false,
    }
}

fn codex_triple(os: &str, arch: &str) -> Option<&'static str> {
    match (os, arch) {
        ("darwin" | "macos", "aarch64") => Some("aarch64-apple-darwin"),
        ("darwin" | "macos", "x86_64") => Some("x86_64-apple-darwin"),
        ("linux", "aarch64") => Some("aarch64-unknown-linux-musl"),
        ("linux", "x86_64") => Some("x86_64-unknown-linux-musl"),
        ("windows", "aarch64") => Some("aarch64-pc-windows-msvc"),
        ("windows", "x86_64") => Some("x86_64-pc-windows-msvc"),
        _ => None,
    }
}

fn validate_version(version: &str, agent_kind: AgentKind) -> Result<(), AgentCliInstallError> {
    let valid = !version.is_empty()
        && version != "."
        && version != ".."
        && version
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'));
    if valid {
        Ok(())
    } else {
        Err(AgentCliInstallError::single(
            agent_kind,
            AgentCliInstallErrorCode::ResolveFailed,
            "The CLI release has an invalid version.",
            version,
        ))
    }
}

fn validate_checksum(checksum: &str, agent_kind: AgentKind) -> Result<(), AgentCliInstallError> {
    if is_sha256(checksum) {
        Ok(())
    } else {
        Err(AgentCliInstallError::single(
            agent_kind,
            AgentCliInstallErrorCode::ResolveFailed,
            "The CLI release has an invalid checksum.",
            checksum,
        ))
    }
}

fn is_sha256(checksum: &str) -> bool {
    checksum.len() == 64 && checksum.bytes().all(|byte| byte.is_ascii_hexdigit())
}

#[derive(Clone)]
struct InstallPaths {
    agent_root: PathBuf,
    target_directory: PathBuf,
    target_binary: PathBuf,
    temp_directory: PathBuf,
    temp_binary: PathBuf,
    download_path: PathBuf,
}

enum PreparedInstall {
    Existing(InstallPaths),
    Pending(InstallPaths),
}

async fn prepare_install(
    managed_root: &Path,
    agent: ManagedAgent,
    version: &str,
) -> Result<PreparedInstall, AgentCliInstallError> {
    let agent_root = managed_root.join(agent.directory_name());
    let target_directory = agent_root.join(version);
    let temp_directory = agent_root.join(format!("{version}.tmp-{}", std::process::id()));
    let target_binary = target_directory.join(agent.binary_relative_path());
    let temp_binary = temp_directory.join(agent.binary_relative_path());
    let download_path = match agent {
        ManagedAgent::Claude => temp_binary.clone(),
        ManagedAgent::Codex => temp_directory.join(".codex-package.tar.gz"),
    };
    let paths = InstallPaths {
        agent_root,
        target_directory,
        target_binary,
        temp_directory,
        temp_binary,
        download_path,
    };
    let blocking_paths = paths.clone();
    let existing = tokio::task::spawn_blocking(move || -> io::Result<bool> {
        fs::create_dir_all(&blocking_paths.agent_root)?;
        if is_executable(&blocking_paths.target_binary) {
            return Ok(true);
        }
        remove_path_if_present(&blocking_paths.target_directory)?;
        remove_path_if_present(&blocking_paths.temp_directory)?;
        fs::create_dir_all(&blocking_paths.temp_directory)?;
        Ok(false)
    })
    .await
    .map_err(|error| write_error(agent, error.to_string()))?
    .map_err(|error| write_error(agent, error.to_string()))?;
    if existing {
        Ok(PreparedInstall::Existing(paths))
    } else {
        Ok(PreparedInstall::Pending(paths))
    }
}

fn remove_path_if_present(path: &Path) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() => fs::remove_dir_all(path),
        Ok(_) => fs::remove_file(path),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

async fn download_verified(
    client: &reqwest::Client,
    url: &str,
    destination: &Path,
    expected_checksum: &str,
    expected_size: Option<u64>,
    agent: ManagedAgent,
    progress: &impl InstallProgressSink,
) -> Result<(), AgentCliInstallError> {
    let response = client
        .get(url)
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|error| {
            AgentCliInstallError::single(
                agent.agent_kind(),
                AgentCliInstallErrorCode::DownloadFailed,
                format!("Halv could not download {}.", agent.display_name()),
                format!("{url}: {error}"),
            )
        })?;
    let total_bytes = expected_size.or_else(|| response.content_length());
    progress.emit(
        agent.agent_kind(),
        AgentCliInstallStage::Downloading,
        format!("Downloading {}.", agent.display_name()),
        Some(0),
        total_bytes,
    );

    let (sender, mut receiver) = tokio::sync::mpsc::channel::<Vec<u8>>(4);
    let writer_destination = destination.to_path_buf();
    let writer = tokio::task::spawn_blocking(move || -> io::Result<()> {
        let mut file = fs::File::create(writer_destination)?;
        while let Some(chunk) = receiver.blocking_recv() {
            file.write_all(&chunk)?;
        }
        file.sync_all()
    });
    let mut stream = response.bytes_stream();
    let mut hasher = Sha256::new();
    let mut downloaded_bytes = 0_u64;
    let mut last_progress = Instant::now();
    let mut stream_error = None;
    let mut channel_closed = false;
    while let Some(chunk) = stream.next().await {
        match chunk {
            Ok(chunk) => {
                hasher.update(&chunk);
                downloaded_bytes = downloaded_bytes.saturating_add(chunk.len() as u64);
                if sender.send(chunk.to_vec()).await.is_err() {
                    channel_closed = true;
                    break;
                }
                if last_progress.elapsed() >= DOWNLOAD_PROGRESS_INTERVAL {
                    progress.emit(
                        agent.agent_kind(),
                        AgentCliInstallStage::Downloading,
                        format!("Downloading {}.", agent.display_name()),
                        Some(downloaded_bytes),
                        total_bytes,
                    );
                    last_progress = Instant::now();
                }
            }
            Err(error) => {
                stream_error = Some(error);
                break;
            }
        }
    }
    drop(sender);
    writer
        .await
        .map_err(|error| write_error(agent, error.to_string()))?
        .map_err(|error| write_error(agent, error.to_string()))?;
    if channel_closed {
        return Err(write_error(
            agent,
            "The download writer stopped before the download completed.".to_string(),
        ));
    }
    if let Some(error) = stream_error {
        return Err(AgentCliInstallError::single(
            agent.agent_kind(),
            AgentCliInstallErrorCode::DownloadFailed,
            format!("Halv could not download {}.", agent.display_name()),
            error.to_string(),
        ));
    }

    let final_total = total_bytes.or(Some(downloaded_bytes));
    progress.emit(
        agent.agent_kind(),
        AgentCliInstallStage::Downloading,
        format!("Downloaded {}.", agent.display_name()),
        Some(downloaded_bytes),
        final_total,
    );
    progress.emit(
        agent.agent_kind(),
        AgentCliInstallStage::Verifying,
        format!("Verifying {}.", agent.display_name()),
        Some(downloaded_bytes),
        final_total,
    );

    let actual_checksum = format!("{:x}", hasher.finalize());
    if !actual_checksum.eq_ignore_ascii_case(expected_checksum) {
        remove_download_file(destination.to_path_buf()).await;
        return Err(AgentCliInstallError::new(
            agent.agent_kind(),
            AgentCliInstallErrorCode::ChecksumMismatch,
            format!("The {} download failed verification.", agent.display_name()),
            vec![
                format!("Expected SHA-256: {expected_checksum}"),
                format!("Actual SHA-256: {actual_checksum}"),
            ],
        ));
    }
    if let Some(expected_size) = expected_size {
        if downloaded_bytes != expected_size {
            remove_download_file(destination.to_path_buf()).await;
            return Err(AgentCliInstallError::new(
                agent.agent_kind(),
                AgentCliInstallErrorCode::ChecksumMismatch,
                format!(
                    "The {} download has an unexpected size.",
                    agent.display_name()
                ),
                vec![
                    format!("Expected bytes: {expected_size}"),
                    format!("Downloaded bytes: {downloaded_bytes}"),
                ],
            ));
        }
    }
    Ok(())
}

async fn remove_download_file(path: PathBuf) {
    let _ = tokio::task::spawn_blocking(move || match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    })
    .await;
}

async fn extract_codex_package(
    package_path: PathBuf,
    destination: PathBuf,
) -> Result<(), AgentCliInstallError> {
    tokio::task::spawn_blocking(move || -> io::Result<()> {
        let package = fs::File::open(&package_path)?;
        let decoder = GzDecoder::new(package);
        let mut archive = tar::Archive::new(decoder);
        archive.unpack(&destination)?;
        fs::remove_file(package_path)
    })
    .await
    .map_err(|error| {
        AgentCliInstallError::single(
            AgentKind::Codex,
            AgentCliInstallErrorCode::ExtractFailed,
            "Halv could not extract the Codex CLI package.",
            error.to_string(),
        )
    })?
    .map_err(|error| {
        AgentCliInstallError::single(
            AgentKind::Codex,
            AgentCliInstallErrorCode::ExtractFailed,
            "Halv could not extract the Codex CLI package.",
            error.to_string(),
        )
    })
}

async fn force_executable(binary: &Path, agent: ManagedAgent) -> Result<(), AgentCliInstallError> {
    let binary = binary.to_path_buf();
    tokio::task::spawn_blocking(move || -> io::Result<()> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut permissions = fs::metadata(&binary)?.permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(&binary, permissions)?;
        }
        #[cfg(not(unix))]
        {
            if !binary.is_file() {
                return Err(io::Error::new(
                    io::ErrorKind::NotFound,
                    "the installed binary is missing",
                ));
            }
        }
        Ok(())
    })
    .await
    .map_err(|error| write_error(agent, error.to_string()))?
    .map_err(|error| write_error(agent, error.to_string()))
}

async fn finish_install(
    paths: &InstallPaths,
    agent: ManagedAgent,
    version: &str,
) -> Result<AgentCliInstallOutcome, AgentCliInstallError> {
    let rename_paths = paths.clone();
    tokio::task::spawn_blocking(move || {
        fs::rename(&rename_paths.temp_directory, &rename_paths.target_directory)
    })
    .await
    .map_err(|error| write_error(agent, error.to_string()))?
    .map_err(|error| write_error(agent, error.to_string()))?;
    record_current(paths, agent, version).await?;
    Ok(install_outcome(
        agent,
        version.to_string(),
        paths.target_binary.clone(),
    ))
}

async fn record_current(
    paths: &InstallPaths,
    agent: ManagedAgent,
    version: &str,
) -> Result<(), AgentCliInstallError> {
    let current = CurrentInstall {
        version: version.to_string(),
        binary: format!("{version}/{}", agent.binary_relative_string()),
        installed_at: Utc::now().to_rfc3339(),
    };
    let contents = serde_json::to_vec_pretty(&current)
        .map_err(|error| write_error(agent, error.to_string()))?;
    let current_path = paths.agent_root.join("current.json");
    let temp_path = paths
        .agent_root
        .join(format!("current.json.tmp-{}", std::process::id()));
    tokio::task::spawn_blocking(move || -> io::Result<()> {
        let mut file = fs::File::create(&temp_path)?;
        file.write_all(&contents)?;
        file.sync_all()?;
        #[cfg(windows)]
        if current_path.exists() {
            fs::remove_file(&current_path)?;
        }
        if let Err(error) = fs::rename(&temp_path, &current_path) {
            let _ = fs::remove_file(&temp_path);
            return Err(error);
        }
        Ok(())
    })
    .await
    .map_err(|error| write_error(agent, error.to_string()))?
    .map_err(|error| write_error(agent, error.to_string()))
}

async fn cleanup_temp(path: PathBuf) {
    let _ = tokio::task::spawn_blocking(move || remove_path_if_present(&path)).await;
}

fn install_outcome(
    agent: ManagedAgent,
    version: String,
    binary: PathBuf,
) -> AgentCliInstallOutcome {
    AgentCliInstallOutcome {
        agent_kind: agent.agent_kind(),
        version,
        binary: binary.to_string_lossy().into_owned(),
    }
}

fn write_error(agent: ManagedAgent, detail: String) -> AgentCliInstallError {
    AgentCliInstallError::single(
        agent.agent_kind(),
        AgentCliInstallErrorCode::WriteFailed,
        format!(
            "Halv could not write the {} installation.",
            agent.display_name()
        ),
        detail,
    )
}

fn manual_hint(agent_kind: AgentKind) -> String {
    match agent_kind {
        AgentKind::Claude => "npm i -g @anthropic-ai/claude-code".to_string(),
        AgentKind::Codex => "npm i -g @openai/codex".to_string(),
        _ => "Install this agent CLI manually.".to_string(),
    }
}

fn agent_name(agent_kind: AgentKind) -> &'static str {
    match agent_kind {
        AgentKind::Claude => "Claude Code",
        AgentKind::Codex => "Codex CLI",
        _ => "agent CLI",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;
    use std::sync::Mutex;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[derive(Default)]
    struct NoopInstallProgress;

    impl InstallProgressSink for NoopInstallProgress {
        fn emit(
            &self,
            _agent_kind: AgentKind,
            _stage: AgentCliInstallStage,
            _line: impl Into<String>,
            _downloaded_bytes: Option<u64>,
            _total_bytes: Option<u64>,
        ) {
        }
    }

    #[derive(Default)]
    struct CountingInstallProgress {
        inner: NoopInstallProgress,
        events: Mutex<Vec<AgentCliInstallProgress>>,
    }

    impl CountingInstallProgress {
        fn events(&self) -> Vec<AgentCliInstallProgress> {
            match self.events.lock() {
                Ok(events) => events.clone(),
                Err(poisoned) => poisoned.into_inner().clone(),
            }
        }
    }

    impl InstallProgressSink for CountingInstallProgress {
        fn emit(
            &self,
            agent_kind: AgentKind,
            stage: AgentCliInstallStage,
            line: impl Into<String>,
            downloaded_bytes: Option<u64>,
            total_bytes: Option<u64>,
        ) {
            let line = line.into();
            self.inner.emit(
                agent_kind,
                stage.clone(),
                line.clone(),
                downloaded_bytes,
                total_bytes,
            );
            let event = AgentCliInstallProgress {
                agent_kind,
                stage,
                line,
                downloaded_bytes,
                total_bytes,
            };
            match self.events.lock() {
                Ok(mut events) => events.push(event),
                Err(poisoned) => poisoned.into_inner().push(event),
            }
        }
    }

    #[tokio::test(flavor = "current_thread")]
    #[ignore = "network"]
    async fn resolves_claude_release() {
        let client = test_client();
        let platform_key = resolve_claude_platform()
            .await
            .unwrap_or_else(|error| panic!("Claude platform should resolve: {error:?}"));
        let stable_url = format!("{CLAUDE_RELEASE_BASE}/stable");
        let version = resolve_text(
            client.get(&stable_url),
            AgentKind::Claude,
            "The test could not resolve the latest Claude Code release.",
            &stable_url,
        )
        .await
        .unwrap_or_else(|error| panic!("Claude release should resolve: {error:?}"));
        let version = version.trim();
        assert!(is_three_part_version(version));

        let manifest_url = format!("{CLAUDE_RELEASE_BASE}/{version}/manifest.json");
        let manifest = resolve_text(
            client.get(&manifest_url),
            AgentKind::Claude,
            "The test could not resolve the Claude Code manifest.",
            &manifest_url,
        )
        .await
        .unwrap_or_else(|error| panic!("Claude manifest should resolve: {error:?}"));
        let entry = claude_manifest_entry(&manifest, platform_key)
            .unwrap_or_else(|error| panic!("Claude platform entry should exist: {error}"));
        assert!(is_sha256(&entry.checksum));
        assert!(entry.size > 10 * 1024 * 1024);

        println!("Claude version: {version}");
    }

    #[tokio::test(flavor = "current_thread")]
    #[ignore = "network"]
    async fn resolves_codex_release() {
        let client = test_client();
        let (release, package_name) = resolve_codex_release_for_test(&client).await;

        assert!(is_three_part_version(&release.version));
        assert!(is_sha256(&release.checksum));
        println!("Codex version: {}", release.version);
        println!("Codex asset: {package_name}");
        println!("Codex SHA-256: {}", release.checksum);
    }

    #[tokio::test(flavor = "current_thread")]
    #[ignore = "network"]
    async fn installs_codex_package_end_to_end() {
        let root = TemporaryManagedRoot::new();
        let client = test_client();
        let (release, package_name) = resolve_codex_release_for_test(&client).await;
        println!("Codex version: {}", release.version);
        println!("Codex asset: {package_name}");
        println!("Codex SHA-256: {}", release.checksum);
        let progress = CountingInstallProgress::default();
        let (outcome, verified_package_size) =
            install_codex_release_for_test(&client, root.path(), release, &progress).await;
        let events = progress.events();
        let downloading_event_count = events
            .iter()
            .filter(|event| event.stage == AgentCliInstallStage::Downloading)
            .count();
        assert!(downloading_event_count > 0);
        assert!(events
            .iter()
            .any(|event| event.stage == AgentCliInstallStage::Verifying));
        let last_downloaded_bytes = events
            .iter()
            .rev()
            .find(|event| event.stage == AgentCliInstallStage::Downloading)
            .and_then(|event| event.downloaded_bytes);
        assert_eq!(last_downloaded_bytes, Some(verified_package_size));
        let binary = managed_binary(ManagedAgent::Codex)
            .unwrap_or_else(|| panic!("managed Codex binary should resolve"));

        assert_eq!(Path::new(&outcome.binary), binary);
        assert!(root.path().join("codex/current.json").exists());
        assert!(binary.exists());
        assert!(is_executable(&binary));
        let output = Command::new(&binary)
            .arg("--version")
            .output()
            .unwrap_or_else(|error| panic!("Codex binary should execute: {error}"));
        assert!(output.status.success());
        let stdout = String::from_utf8(output.stdout)
            .unwrap_or_else(|error| panic!("Codex version should be UTF-8: {error}"));
        let version_output = stdout.trim();
        assert!(version_output.bytes().any(|byte| byte.is_ascii_digit()));

        println!("Verified package bytes: {verified_package_size}");
        println!("Downloading events: {downloading_event_count}");
        println!("Codex --version: {version_output}");
    }

    #[test]
    fn maps_claude_platform_keys() {
        assert_eq!(
            claude_platform_key("darwin", "aarch64", false),
            Some("darwin-arm64")
        );
        assert_eq!(
            claude_platform_key("darwin", "x86_64", true),
            Some("darwin-x64")
        );
        assert_eq!(
            claude_platform_key("macos", "aarch64", false),
            Some("darwin-arm64")
        );
        assert_eq!(
            claude_platform_key("linux", "aarch64", false),
            Some("linux-arm64")
        );
        assert_eq!(
            claude_platform_key("linux", "aarch64", true),
            Some("linux-arm64-musl")
        );
        assert_eq!(
            claude_platform_key("linux", "x86_64", false),
            Some("linux-x64")
        );
        assert_eq!(
            claude_platform_key("linux", "x86_64", true),
            Some("linux-x64-musl")
        );
        assert_eq!(
            claude_platform_key("windows", "x86_64", false),
            Some("win32-x64")
        );
        assert_eq!(claude_platform_key("windows", "aarch64", false), None);
        assert_eq!(claude_platform_key("freebsd", "x86_64", false), None);
    }

    #[test]
    fn maps_codex_platform_triples() {
        assert_eq!(
            codex_triple("darwin", "aarch64"),
            Some("aarch64-apple-darwin")
        );
        assert_eq!(
            codex_triple("darwin", "x86_64"),
            Some("x86_64-apple-darwin")
        );
        assert_eq!(
            codex_triple("macos", "aarch64"),
            Some("aarch64-apple-darwin")
        );
        assert_eq!(
            codex_triple("linux", "aarch64"),
            Some("aarch64-unknown-linux-musl")
        );
        assert_eq!(
            codex_triple("linux", "x86_64"),
            Some("x86_64-unknown-linux-musl")
        );
        assert_eq!(
            codex_triple("windows", "aarch64"),
            Some("aarch64-pc-windows-msvc")
        );
        assert_eq!(
            codex_triple("windows", "x86_64"),
            Some("x86_64-pc-windows-msvc")
        );
        assert_eq!(codex_triple("freebsd", "x86_64"), None);
    }

    #[test]
    fn parses_codex_package_checksum() {
        let wanted = "codex-package-aarch64-apple-darwin.tar.gz";
        let checksum = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        let contents = format!(
            "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff  other.tar.gz\n{checksum}  {wanted}   \n"
        );
        assert_eq!(
            parse_codex_checksum(&contents, wanted),
            Some(checksum.to_string())
        );
        assert_eq!(parse_codex_checksum(&contents, "missing.tar.gz"), None);
    }

    #[test]
    fn parses_claude_manifest_platform_entry() {
        let manifest = r#"{
            "version": "2.1.231",
            "platforms": {
                "darwin-arm64": {
                    "binary": "claude",
                    "checksum": "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
                    "size": 294720528
                }
            }
        }"#;
        let entry = claude_manifest_entry(manifest, "darwin-arm64")
            .unwrap_or_else(|error| panic!("manifest should parse: {error}"));
        assert_eq!(entry.binary, "claude");
        assert_eq!(entry.size, 294720528);
        assert!(claude_manifest_entry(manifest, "linux-x64").is_err());
    }

    #[test]
    fn resolves_managed_binary_and_falls_back_to_newest_version() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "halv-managed-cli-test-{}-{nonce}",
            std::process::id()
        ));
        let agent_root = root.join("claude");
        let first_directory = agent_root.join("1.0.0");
        let first_binary = first_directory.join(ManagedAgent::Claude.binary_relative_path());
        fs::create_dir_all(&first_directory)
            .unwrap_or_else(|error| panic!("test directory should exist: {error}"));
        write_test_executable(&first_binary);
        let current = CurrentInstall {
            version: "1.0.0".to_string(),
            binary: format!("1.0.0/{}", ManagedAgent::Claude.binary_relative_string()),
            installed_at: "2026-08-24T00:00:00Z".to_string(),
        };
        fs::write(
            agent_root.join("current.json"),
            serde_json::to_vec(&current)
                .unwrap_or_else(|error| panic!("test marker should serialize: {error}")),
        )
        .unwrap_or_else(|error| panic!("test marker should exist: {error}"));
        set_managed_root(root.clone());
        assert_eq!(
            managed_binary(ManagedAgent::Claude),
            Some(first_binary.clone())
        );

        fs::remove_file(&first_binary)
            .unwrap_or_else(|error| panic!("test binary should be removed: {error}"));
        assert_eq!(managed_binary(ManagedAgent::Claude), None);

        fs::remove_file(agent_root.join("current.json"))
            .unwrap_or_else(|error| panic!("test marker should be removed: {error}"));
        write_test_executable(&first_binary);
        std::thread::sleep(Duration::from_millis(20));
        let second_directory = agent_root.join("2.0.0");
        let second_binary = second_directory.join(ManagedAgent::Claude.binary_relative_path());
        fs::create_dir_all(&second_directory)
            .unwrap_or_else(|error| panic!("test directory should exist: {error}"));
        write_test_executable(&second_binary);
        std::thread::sleep(Duration::from_millis(20));
        let temp_directory = agent_root.join("3.0.0.tmp-123");
        let temp_binary = temp_directory.join(ManagedAgent::Claude.binary_relative_path());
        fs::create_dir_all(&temp_directory)
            .unwrap_or_else(|error| panic!("test directory should exist: {error}"));
        write_test_executable(&temp_binary);
        assert_eq!(managed_binary(ManagedAgent::Claude), Some(second_binary));

        fs::remove_dir_all(&root)
            .unwrap_or_else(|error| panic!("test directory should be removed: {error}"));
    }

    #[test]
    fn stale_codex_marker_finds_a_complete_installation() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root =
            std::env::temp_dir().join(format!("halv-stale-codex-{}-{nonce}", std::process::id()));
        let binary = root
            .join("2.0.0")
            .join(ManagedAgent::Codex.binary_relative_path());
        fs::create_dir_all(binary.parent().unwrap()).unwrap();
        write_test_executable(&binary);
        let incomplete = root
            .join("3.0.0.tmp-123")
            .join(ManagedAgent::Codex.binary_relative_path());
        fs::create_dir_all(incomplete.parent().unwrap()).unwrap();
        write_test_executable(&incomplete);
        fs::write(
            root.join("current.json"),
            serde_json::to_vec(&CurrentInstall {
                version: "1.0.0".into(),
                binary: format!("1.0.0/{}", ManagedAgent::Codex.binary_relative_string()),
                installed_at: "2026-09-30T00:00:00Z".into(),
            })
            .unwrap(),
        )
        .unwrap();
        assert_eq!(
            managed_binary_at(&root, ManagedAgent::Codex),
            Some(binary.clone())
        );
        fs::remove_file(&binary).unwrap();
        assert_eq!(managed_binary_at(&root, ManagedAgent::Codex), None);
        fs::remove_dir_all(root).unwrap();
    }

    fn write_test_executable(path: &Path) {
        fs::write(path, b"test")
            .unwrap_or_else(|error| panic!("test binary should exist: {error}"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut permissions = fs::metadata(path)
                .unwrap_or_else(|error| panic!("test metadata should exist: {error}"))
                .permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(path, permissions)
                .unwrap_or_else(|error| panic!("test mode should be set: {error}"));
        }
    }

    fn test_client() -> reqwest::Client {
        reqwest::Client::builder()
            .user_agent(format!("halv-test/{}", env!("CARGO_PKG_VERSION")))
            .connect_timeout(CONNECT_TIMEOUT)
            .build()
            .unwrap_or_else(|error| panic!("test client should build: {error}"))
    }

    async fn resolve_codex_release_for_test(client: &reqwest::Client) -> (CodexRelease, String) {
        let triple = codex_triple(std::env::consts::OS, std::env::consts::ARCH)
            .unwrap_or_else(|| panic!("Codex should support the current platform"));
        let response = client
            .get(CODEX_RELEASE_URL)
            .header(reqwest::header::ACCEPT, "application/vnd.github+json")
            .send()
            .await
            .and_then(reqwest::Response::error_for_status)
            .unwrap_or_else(|error| panic!("Codex release should resolve: {error}"));
        let release = response
            .json::<CodexReleaseResponse>()
            .await
            .unwrap_or_else(|error| panic!("Codex release should parse: {error}"));
        let version = release
            .tag_name
            .strip_prefix("rust-v")
            .or_else(|| release.tag_name.strip_prefix('v'))
            .unwrap_or(&release.tag_name)
            .to_string();
        validate_version(&version, AgentKind::Codex)
            .unwrap_or_else(|error| panic!("Codex version should be valid: {error:?}"));
        let package_name = format!("codex-package-{triple}.tar.gz");
        let package_url = release
            .assets
            .iter()
            .find(|asset| asset.name == package_name)
            .map(|asset| asset.browser_download_url.clone())
            .unwrap_or_else(|| panic!("Codex package should exist: {package_name}"));
        let sums_asset = release
            .assets
            .iter()
            .find(|asset| asset.name == "codex-package_SHA256SUMS")
            .unwrap_or_else(|| panic!("Codex checksum list should exist"));
        let sums_text = resolve_text(
            client.get(&sums_asset.browser_download_url),
            AgentKind::Codex,
            "The test could not resolve the Codex CLI checksum.",
            &sums_asset.browser_download_url,
        )
        .await
        .unwrap_or_else(|error| panic!("Codex checksum list should resolve: {error:?}"));
        let checksum = parse_codex_checksum(&sums_text, &package_name)
            .unwrap_or_else(|| panic!("Codex package checksum should exist: {package_name}"));

        (
            CodexRelease {
                version,
                package_url,
                checksum,
            },
            package_name,
        )
    }

    async fn install_codex_release_for_test(
        client: &reqwest::Client,
        managed_root: &Path,
        release: CodexRelease,
        progress: &impl InstallProgressSink,
    ) -> (AgentCliInstallOutcome, u64) {
        let paths = match prepare_install(managed_root, ManagedAgent::Codex, &release.version)
            .await
            .unwrap_or_else(|error| panic!("Codex install path should prepare: {error:?}"))
        {
            PreparedInstall::Pending(paths) => paths,
            PreparedInstall::Existing(_) => panic!("the temporary root should be empty"),
        };
        download_verified(
            client,
            &release.package_url,
            &paths.download_path,
            &release.checksum,
            None,
            ManagedAgent::Codex,
            progress,
        )
        .await
        .unwrap_or_else(|error| panic!("Codex package should download: {error:?}"));
        let verified_package_size = fs::metadata(&paths.download_path)
            .map(|metadata| metadata.len())
            .unwrap_or_else(|error| panic!("Codex package size should resolve: {error}"));
        extract_codex_package(paths.download_path.clone(), paths.temp_directory.clone())
            .await
            .unwrap_or_else(|error| panic!("Codex package should extract: {error:?}"));
        force_executable(&paths.temp_binary, ManagedAgent::Codex)
            .await
            .unwrap_or_else(|error| panic!("Codex binary should be executable: {error:?}"));
        let outcome = finish_install(&paths, ManagedAgent::Codex, &release.version)
            .await
            .unwrap_or_else(|error| panic!("Codex install should finish: {error:?}"));
        (outcome, verified_package_size)
    }

    fn is_three_part_version(version: &str) -> bool {
        let mut parts = version.split('.');
        (0..3).all(|_| {
            parts.next().is_some_and(|part| {
                !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit())
            })
        }) && parts.next().is_none()
    }

    struct TemporaryManagedRoot {
        path: PathBuf,
        previous_root: Option<PathBuf>,
    }

    impl TemporaryManagedRoot {
        fn new() -> Self {
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "halv-codex-network-test-{}-{nonce}",
                std::process::id()
            ));
            let previous_root = managed_root();
            set_managed_root(path.clone());
            Self {
                path,
                previous_root,
            }
        }

        fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for TemporaryManagedRoot {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
            match MANAGED_ROOT.write() {
                Ok(mut managed_root) => *managed_root = self.previous_root.take(),
                Err(poisoned) => *poisoned.into_inner() = self.previous_root.take(),
            }
        }
    }
}
