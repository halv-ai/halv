use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{BTreeMap, HashMap, VecDeque},
    ffi::{OsStr, OsString},
    fs,
    io::{BufRead, BufReader, BufWriter, Read, Write},
    path::{Path, PathBuf},
    process::{Child, Command, ExitStatus, Stdio},
    sync::{
        atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
        mpsc, Arc, Mutex, OnceLock, RwLock,
    },
    time::{Duration, Instant},
};
use tauri::async_runtime::Mutex as AsyncMutex;
use tauri::{AppHandle, Emitter, Manager, Runtime};

use crate::{agent_sessions::SessionLaneKind, storage::Storage};

pub const DEFAULT_PORT: u16 = 8787;
const SIDECAR_PORT_FILE_NAME: &str = "sidecar-port.json";

#[derive(Debug, Deserialize, Serialize)]
struct SidecarPortFile {
    port: u16,
}

fn read_saved_sidecar_port(app_data_dir: &Path) -> Result<Option<u16>, String> {
    let path = app_data_dir.join(SIDECAR_PORT_FILE_NAME);
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(format!(
                "failed to read saved headroom port `{}`: {error}",
                path.display()
            ));
        }
    };
    let saved = serde_json::from_slice::<SidecarPortFile>(&bytes).map_err(|error| {
        format!(
            "failed to decode saved headroom port `{}`: {error}",
            path.display()
        )
    })?;
    if saved.port == 0 {
        return Err(format!(
            "failed to decode saved headroom port `{}`: port must not be zero",
            path.display()
        ));
    }
    Ok(Some(saved.port))
}

fn write_saved_sidecar_port(app_data_dir: &Path, port: u16) -> Result<(), String> {
    fs::create_dir_all(app_data_dir).map_err(|error| {
        format!(
            "failed to create headroom port directory `{}`: {error}",
            app_data_dir.display()
        )
    })?;
    let path = app_data_dir.join(SIDECAR_PORT_FILE_NAME);
    let bytes = serde_json::to_vec(&SidecarPortFile { port })
        .map_err(|error| format!("failed to encode headroom port: {error}"))?;
    fs::write(&path, bytes).map_err(|error| {
        format!(
            "failed to write saved headroom port `{}`: {error}",
            path.display()
        )
    })
}

/// One random free loopback port per app run. A fixed default can collide
/// with the user's own services (an external Headroom often sits nearby).
/// The manager retries this port on child respawns, so live sessions that
/// hold a base URL keep working across an auto-heal.
pub fn random_default_port() -> u16 {
    select_available_loopback_port(0).unwrap_or(DEFAULT_PORT)
}

// First run after a fresh install can take >15s (pyc compilation of the
// proxy's dependency tree); wait_for_ready returns as soon as /stats answers.
const READINESS_TIMEOUT: Duration = Duration::from_secs(45);
const POLL_INTERVAL: Duration = Duration::from_millis(300);
const START_ATTEMPTS: usize = 3;
// Restarts never stop. Each restart inside the window waits longer than the
// one before it: 0, 1s, 2s, 4s ... up to the cap. The window resets the wait.
const AUTO_HEAL_WINDOW: Duration = Duration::from_secs(10 * 60);
const AUTO_HEAL_BACKOFF_BASE: Duration = Duration::from_secs(1);
const AUTO_HEAL_BACKOFF_MAX: Duration = Duration::from_secs(30);
// The supervisor reaps an exited child without the UI poll.
const SUPERVISOR_POLL_INTERVAL: Duration = Duration::from_secs(1);
// A slow /stats answer is a busy worker. Only a run of timeouts restarts it.
const STATS_TIMEOUTS_BEFORE_RESTART: u32 = 3;
// Agents keep the proxy port from their launch, so a restart waits for the
// saved port before it moves to another one.
const SAVED_PORT_WAIT: Duration = Duration::from_secs(3);
const SAVED_PORT_WAIT_STEP: Duration = Duration::from_millis(100);
const SUPERVISOR_LOG_FILE_NAME: &str = "sidecar-supervisor.log";
const SUPERVISOR_LOG_MAX_BYTES: u64 = 512 * 1024;
const SUPERVISOR_LOG_STDERR_LINES: usize = 20;
const VERSION_COMMAND_TIMEOUT: Duration = Duration::from_secs(5);
const SIDECAR_COMMAND_OUTPUT_LIMIT_BYTES: usize = 64 * 1024;
const HEADROOM_PROXY_LOG_MAX_BYTES: u64 = 2 * 1024 * 1024;
const HEADROOM_STDERR_RING_LINES: usize = 40;
const HEADROOM_STDERR_LINE_MAX_CHARS: usize = 400;
const HEADROOM_STDERR_MESSAGE_LINES: usize = 5;
const CHILD_OUTPUT_MAX_CHARS: usize = 600;
const CHILD_OUTPUT_MIN_TAIL_LINE_CHARS: usize = 40;
const LAST_OUTPUT_MARKER: &str = ". Last output: ";
const STDERR_DRAIN_FINISH_TIMEOUT: Duration = Duration::from_secs(1);
const HALV_HEADROOM_AGENT: &str = "codex";
const SIDECAR_INSTALL_PROGRESS_EVENT: &str = "sidecar_install_progress";
const HALV_HEADROOM_BINARY_ENV: &str = "HALV_HEADROOM_BINARY";
const HALV_MANAGED_SIDECAR_ENV: &str = "HALV_MANAGED_SIDECAR";
const HALV_MANAGED_SIDECAR_VALUE: &str = "1";
#[cfg(any(target_os = "macos", target_os = "linux"))]
const HALV_MANAGED_SIDECAR_MARKER: &str = "HALV_MANAGED_SIDECAR=1";
#[cfg(unix)]
const HEADROOM_PROXY_PGREP_PATTERN: &str = "headroom proxy";
#[cfg(unix)]
const ORPHAN_REAP_INSPECTION_BUDGET: Duration = Duration::from_millis(300);
#[cfg(unix)]
const ORPHAN_REAP_TOTAL_BUDGET: Duration = Duration::from_millis(400);
#[cfg(unix)]
const ORPHAN_REAP_TERM_GRACE: Duration = Duration::from_millis(50);
#[cfg(unix)]
const ORPHAN_REAP_POLL_INTERVAL: Duration = Duration::from_millis(5);
const BUNDLED_HEADROOM_LOCK_JSON: &str =
    include_str!("../../../scripts/headroom-runtime.lock.json");

fn is_halv_headroom_agent(agent_name: &str) -> bool {
    agent_name
        .split(|character: char| !character.is_ascii_alphanumeric())
        .any(|part| part.eq_ignore_ascii_case(HALV_HEADROOM_AGENT))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BundledHeadroomLock {
    runtime_version: String,
}

fn bundled_headroom_lock() -> &'static BundledHeadroomLock {
    static LOCK: OnceLock<BundledHeadroomLock> = OnceLock::new();
    LOCK.get_or_init(|| {
        serde_json::from_str(BUNDLED_HEADROOM_LOCK_JSON)
            .expect("scripts/headroom-runtime.lock.json must be valid")
    })
}

#[derive(Debug, Clone, Serialize)]
#[serde(
    tag = "state",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum SidecarStatus {
    Running {
        spawned_by_us: bool,
        managed_by_halv: bool,
        optimization_paused: bool,
        version: Option<String>,
        pinned_version: String,
        port: u16,
    },
    Installing {
        port: u16,
    },
    Starting {
        port: u16,
    },
    NotInstalled {
        install_hint: String,
        port: u16,
    },
    Error {
        message: String,
        port: u16,
    },
}

/// Accepts process ownership or managed-install provenance.
pub(crate) fn halv_managed_listener_port(status: &SidecarStatus) -> Option<u16> {
    match status {
        SidecarStatus::Running {
            spawned_by_us,
            managed_by_halv,
            port,
            ..
        } if *spawned_by_us || *managed_by_halv => Some(*port),
        _ => None,
    }
}

// Legacy variants remain serializable for additive IPC compatibility even
// though the frozen-runtime installer no longer constructs them.
#[allow(dead_code)]
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SidecarInstallStage {
    Starting,
    // Kept for compatibility with older frontend/event consumers. Frozen
    // runtimes no longer emit the legacy installer stages.
    ResolvingUv,
    DownloadingUv,
    VerifyingUv,
    InstallingWithUv,
    InstallingWithPipx,
    DetectingHeadroom,
    Success,
    Error,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SidecarInstallProgress {
    pub stage: SidecarInstallStage,
    pub line: String,
}

// Legacy variants remain serializable for additive IPC compatibility even
// though the frozen-runtime installer no longer constructs them.
#[allow(dead_code)]
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SidecarInstallErrorCode {
    AppDataUnavailable,
    BundledRuntimeMissing,
    // Legacy installer error codes remain part of the serialized IPC contract
    // even though frozen-runtime builds no longer construct them.
    UnsupportedPlatform,
    DownloadFailed,
    ChecksumMismatch,
    ExtractFailed,
    AllStrategiesFailed,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SidecarInstallError {
    pub code: SidecarInstallErrorCode,
    pub message: String,
    pub details: Vec<String>,
    pub manual_hint: String,
}

impl SidecarInstallError {
    fn new(
        code: SidecarInstallErrorCode,
        message: impl Into<String>,
        details: Vec<String>,
    ) -> Self {
        Self {
            code,
            message: message.into(),
            details,
            manual_hint: install_hint(),
        }
    }

    fn single(
        code: SidecarInstallErrorCode,
        message: impl Into<String>,
        detail: impl Into<String>,
    ) -> Self {
        Self::new(code, message, vec![detail.into()])
    }
}

trait InstallProgressSink {
    fn emit(&self, stage: SidecarInstallStage, line: impl Into<String>);
}

struct TauriInstallProgress<R: Runtime> {
    app: AppHandle<R>,
}

impl<R: Runtime> InstallProgressSink for TauriInstallProgress<R> {
    fn emit(&self, stage: SidecarInstallStage, line: impl Into<String>) {
        let _ = self.app.emit(
            SIDECAR_INSTALL_PROGRESS_EVENT,
            SidecarInstallProgress {
                stage,
                line: line.into(),
            },
        );
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SidecarStats {
    pub port: u16,
    /// Halv-managed child spawn generation; changes even when a restart reuses the same port.
    pub instance_generation: u64,
    /// Disk-backed lifetime tokens saved when Headroom exposes persistent savings.
    pub lifetime_tokens_saved: Option<u64>,
    /// Disk-backed lifetime cost saved when Headroom exposes persistent savings.
    pub lifetime_cost_saved_usd: Option<f64>,
    /// Current exact tokens saved across every non-overlapping Headroom input
    /// layer plus tool-schema deferral. RTK is refreshed from Halv's database.
    pub tokens_saved: u64,
    /// Current proxy-process cost attributed to Halv's agent when Headroom reports agent usage.
    /// Older Headroom payloads fall back to the best available global counter.
    pub cost_saved_usd: f64,
    /// Current proxy-process request count, scoped to Halv's agent when Headroom reports agent usage.
    /// Older Headroom payloads fall back to the best available global counter.
    pub requests_total: u64,
    pub lifetime_requests: u64,
    pub lifetime_cache_savings_usd: f64,
    /// Current proxy-process request counts grouped by the upstream provider.
    pub requests_by_provider: BTreeMap<String, u64>,
    /// Current proxy-process usage grouped by the stored agent lane.
    pub usage_by_lane: Vec<LaneUsage>,
    /// Current proxy-process usage without a matching agent session.
    pub unattributed_usage: LaneUsage,
    /// Current proxy-process cached request count.
    pub requests_cached: u64,
    /// Current proxy-process failed request count.
    pub requests_failed: u64,
    /// Headroom display-session tokens saved; proxy-wide because Headroom has no session agent split.
    pub session_tokens_saved: u64,
    /// Headroom display-session cost saved; proxy-wide because Headroom has no session agent split.
    pub session_cost_saved_usd: f64,
    /// Lifetime-preferred input token count; kept for the existing IPC contract.
    pub input_tokens: u64,
    /// Current proxy-process input token count.
    pub process_input_tokens: u64,
    /// Current proxy-process output token count.
    pub output_tokens: u64,
    /// Current proxy-process proxy-compression token savings.
    pub proxy_compression_tokens_saved: u64,
    /// Compression tokens attributed to Halv's agent within that proxy.
    pub compression_tokens_saved: u64,
    /// Current tokens avoided by the selected CLI context filter.
    pub cli_filtering_tokens_saved: u64,
    /// Current Halv-process tokens avoided by RTK shell-output filtering.
    pub rtk_tokens_saved: u64,
    /// Current tokens avoided by Lean Context when it is the selected CLI filter.
    pub lean_ctx_tokens_saved: u64,
    /// Current exact tool-schema tokens deferred out of model context.
    pub tool_search_tokens_saved: u64,
    /// Counterfactual output-token reduction estimate; never included in exact totals.
    pub estimated_output_tokens_saved: u64,
    /// Machine-wide MCP compression tokens; not attributable to a Halv chat turn.
    pub mcp_tokens_saved: u64,
    /// Live WS unit savings diagnostic; overlaps completed-request compression totals.
    pub live_ws_tokens_saved: u64,
    /// Current proxy-process cache savings in USD.
    pub cache_savings_usd: f64,
    /// Current proxy-process cost after Headroom savings.
    pub cost_with_headroom_usd: f64,
    /// Lifetime/session/process savings percentage, in Headroom fallback order.
    pub savings_percent: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plan_windows: Option<Vec<PlanWindowStats>>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LaneUsage {
    pub lane: String,
    pub billing: String,
    pub provider: String,
    pub requests: u64,
    pub input_tokens: u64,
    pub output_tokens: Option<u64>,
    pub cost_usd: Option<f64>,
    pub tokens_saved: u64,
}

impl LaneUsage {
    fn for_lane(lane: SessionLaneKind) -> Self {
        let (billing, provider) = match lane {
            SessionLaneKind::ClaudeSubscription => ("plan", "claude"),
            SessionLaneKind::ClaudeApiKey => ("api", "claude"),
            SessionLaneKind::CodexChatGpt => ("plan", "openai"),
            // The GLM Coding Plan is a flat subscription.
            SessionLaneKind::ZaiGlm => ("plan", "zai"),
            SessionLaneKind::GrokApiKey => ("api", "grok"),
            SessionLaneKind::KimiApiKey => ("api", "kimi"),
            SessionLaneKind::KimiSubscription => ("plan", "kimi"),
            SessionLaneKind::Custom => ("api", "custom"),
        };
        Self {
            lane: lane.storage_name().to_string(),
            billing: billing.to_string(),
            provider: provider.to_string(),
            requests: 0,
            input_tokens: 0,
            output_tokens: None,
            cost_usd: (billing == "api").then_some(0.0),
            tokens_saved: 0,
        }
    }

    fn unattributed() -> Self {
        Self {
            lane: "unknown".to_string(),
            billing: "unknown".to_string(),
            provider: "custom".to_string(),
            requests: 0,
            input_tokens: 0,
            output_tokens: None,
            cost_usd: None,
            tokens_saved: 0,
        }
    }

    fn add_rollup(&mut self, rollup: &SavingsRollup) {
        self.requests = self
            .requests
            .saturating_add(rollup.requests.unwrap_or_default());
        self.input_tokens = self
            .input_tokens
            .saturating_add(rollup.total_input_tokens.unwrap_or_default());
        self.tokens_saved = self
            .tokens_saved
            .saturating_add(rollup.tokens_saved.unwrap_or_default());
        if let Some(cost_usd) = self.cost_usd.as_mut() {
            *cost_usd += rollup.total_input_cost_usd.unwrap_or_default();
        }
    }
}

impl SidecarStats {
    fn replace_rtk_tokens_saved(&mut self, exact_tokens_saved: u64) {
        self.cli_filtering_tokens_saved = self
            .cli_filtering_tokens_saved
            .saturating_sub(self.rtk_tokens_saved)
            .saturating_add(exact_tokens_saved);
        self.tokens_saved = self
            .tokens_saved
            .saturating_sub(self.rtk_tokens_saved)
            .saturating_add(exact_tokens_saved);
        self.rtk_tokens_saved = exact_tokens_saved;
        let exact_tokens_before = self.process_input_tokens.saturating_add(self.tokens_saved);
        self.savings_percent = (exact_tokens_before > 0)
            .then_some(self.tokens_saved as f64 / exact_tokens_before as f64 * 100.0);
    }
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PlanWindowStats {
    pub label: String,
    pub used_percent: f64,
    pub resets_at: Option<i64>,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct HeadroomStats {
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub summary: Option<StatsSummary>,
    #[serde(default)]
    pub agent_usage: Option<AgentUsageStats>,
    #[serde(default)]
    pub savings: Option<SavingsStats>,
    #[serde(default)]
    pub requests: Option<RequestStats>,
    #[serde(default)]
    pub tokens: Option<TokenStats>,
    #[serde(default)]
    pub latency: Option<TimingStats>,
    #[serde(default)]
    pub overhead: Option<TimingStats>,
    #[serde(default)]
    pub ttfb: Option<TimingStats>,
    #[serde(default)]
    pub cost: Option<CostStats>,
    #[serde(default)]
    pub display_session: Option<SavingsRollup>,
    #[serde(default)]
    pub persistent_savings: Option<PersistentSavingsStats>,
    #[serde(default)]
    pub codex_rate_limits: Option<CodexRateLimits>,
    #[serde(default)]
    pub recent_requests: Vec<RecentRequestStats>,
    #[serde(default)]
    pub request_logs: Vec<RecentRequestStats>,
    #[serde(default, flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct CodexRateLimits {
    #[serde(default)]
    pub primary: Option<CodexRateLimitWindow>,
    #[serde(default)]
    pub secondary: Option<CodexRateLimitWindow>,
    #[serde(default, flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct CodexRateLimitWindow {
    #[serde(default)]
    pub used_percent: Option<f64>,
    #[serde(default)]
    pub window_label: Option<String>,
    #[serde(default)]
    pub window_minutes: Option<u64>,
    #[serde(default)]
    pub resets_at: Option<i64>,
    #[serde(default, flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct StatsSummary {
    #[serde(default)]
    pub mode: Option<String>,
    #[serde(default)]
    pub api_requests: Option<u64>,
    #[serde(default)]
    pub primary_model: Option<String>,
    #[serde(default)]
    pub compression: Option<SummaryCompressionStats>,
    #[serde(default)]
    pub uncompressed_requests: Option<UncompressedRequestStats>,
    #[serde(default)]
    pub cost: Option<SummaryCostStats>,
    #[serde(default)]
    pub mcp: Option<SummaryMcpStats>,
    #[serde(default, flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct SummaryCompressionStats {
    #[serde(default)]
    pub requests_compressed: Option<u64>,
    #[serde(default)]
    pub avg_compression_pct: Option<f64>,
    #[serde(default)]
    pub best_compression_pct: Option<f64>,
    #[serde(default)]
    pub best_detail: Option<String>,
    #[serde(default)]
    pub total_tokens_removed: Option<u64>,
    #[serde(default)]
    pub cli_filtering_tokens_avoided: Option<u64>,
    #[serde(default)]
    pub total_tokens_saved_with_cli_filtering: Option<u64>,
    #[serde(default)]
    pub total_tokens_before_with_cli_filtering: Option<u64>,
    #[serde(default)]
    pub rtk_tokens_avoided: Option<u64>,
    #[serde(default)]
    pub total_tokens_saved_with_rtk: Option<u64>,
    #[serde(default)]
    pub total_tokens_before_with_rtk: Option<u64>,
    #[serde(default, flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct UncompressedRequestStats {
    #[serde(default)]
    pub prefix_frozen: Option<u64>,
    #[serde(default)]
    pub no_compressible_content: Option<u64>,
    #[serde(default, flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct SummaryCostStats {
    #[serde(default)]
    pub without_headroom_usd: Option<f64>,
    #[serde(default)]
    pub with_headroom_usd: Option<f64>,
    #[serde(default)]
    pub total_saved_usd: Option<f64>,
    #[serde(default)]
    pub savings_pct: Option<f64>,
    #[serde(default)]
    pub breakdown: Option<CostBreakdownStats>,
    #[serde(default, flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct CostBreakdownStats {
    #[serde(default)]
    pub cache_savings_usd: Option<f64>,
    #[serde(default)]
    pub compression_savings_usd: Option<f64>,
    #[serde(default)]
    pub cli_filtering_savings_usd: Option<f64>,
    #[serde(default)]
    pub cli_filtering_savings_note: Option<String>,
    #[serde(default)]
    pub rtk_savings_usd: Option<f64>,
    #[serde(default)]
    pub rtk_savings_note: Option<String>,
    #[serde(default, flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct SummaryMcpStats {
    #[serde(default)]
    pub compressions: Option<u64>,
    #[serde(default)]
    pub tokens_removed: Option<u64>,
    #[serde(default)]
    pub retrievals: Option<u64>,
    #[serde(default, flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct AgentUsageStats {
    #[serde(default)]
    pub agents: Vec<AgentUsageRow>,
    #[serde(default)]
    pub totals: Option<AgentUsageTotals>,
    #[serde(default)]
    pub coverage: Option<Value>,
    #[serde(default, flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct AgentUsageRow {
    #[serde(default)]
    pub agent: Option<String>,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub requests: Option<u64>,
    #[serde(default)]
    pub before_tokens: Option<u64>,
    #[serde(default)]
    pub after_tokens: Option<u64>,
    #[serde(default)]
    pub output_tokens: Option<u64>,
    #[serde(default)]
    pub tokens_saved: Option<u64>,
    #[serde(default)]
    pub models: BTreeMap<String, u64>,
    #[serde(default)]
    pub providers: BTreeMap<String, u64>,
    #[serde(default)]
    pub has_exact_tokens: Option<bool>,
    #[serde(default)]
    pub savings_percent: Option<f64>,
    #[serde(default)]
    pub after_percent: Option<f64>,
    #[serde(default)]
    pub share_of_saved_percent: Option<f64>,
    #[serde(default)]
    pub share_of_requests_percent: Option<f64>,
    #[serde(default, flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct AgentUsageTotals {
    #[serde(default)]
    pub requests: Option<u64>,
    #[serde(default)]
    pub before_tokens: Option<u64>,
    #[serde(default)]
    pub after_tokens: Option<u64>,
    #[serde(default)]
    pub output_tokens: Option<u64>,
    #[serde(default)]
    pub tokens_saved: Option<u64>,
    #[serde(default)]
    pub savings_percent: Option<f64>,
    #[serde(default, flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct SavingsStats {
    #[serde(default)]
    pub total_tokens: Option<u64>,
    #[serde(default)]
    pub per_project: BTreeMap<String, SavingsRollup>,
    #[serde(default)]
    pub by_layer: BTreeMap<String, Value>,
    #[serde(default, flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct RequestStats {
    #[serde(default)]
    pub total: Option<u64>,
    #[serde(default)]
    pub cached: Option<u64>,
    #[serde(default)]
    pub rate_limited: Option<u64>,
    #[serde(default)]
    pub failed: Option<u64>,
    #[serde(default)]
    pub by_provider: BTreeMap<String, u64>,
    #[serde(default)]
    pub by_model: BTreeMap<String, u64>,
    #[serde(default)]
    pub by_stack: BTreeMap<String, u64>,
    #[serde(default, flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct TokenStats {
    #[serde(default)]
    pub input: Option<u64>,
    #[serde(default)]
    pub output: Option<u64>,
    #[serde(default)]
    pub output_saved: Option<u64>,
    #[serde(default)]
    pub output_reduction_percent: Option<f64>,
    #[serde(default)]
    pub saved: Option<u64>,
    #[serde(default)]
    pub proxy_compression_saved: Option<u64>,
    #[serde(default)]
    pub cli_filtering_saved: Option<u64>,
    #[serde(default)]
    pub rtk_saved: Option<u64>,
    #[serde(default)]
    pub lean_ctx_saved: Option<u64>,
    #[serde(default)]
    pub cli_tokens_avoided: Option<u64>,
    #[serde(default)]
    pub proxy_total_before_compression: Option<u64>,
    #[serde(default)]
    pub total_before_compression: Option<u64>,
    #[serde(default)]
    pub all_layers_saved: Option<u64>,
    #[serde(default)]
    pub proxy_attempted_tokens: Option<u64>,
    #[serde(default)]
    pub active_savings_percent: Option<f64>,
    #[serde(default)]
    pub proxy_savings_percent: Option<f64>,
    #[serde(default)]
    pub savings_percent: Option<f64>,
    #[serde(default)]
    pub all_layers_savings_percent: Option<f64>,
    #[serde(default, flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct TimingStats {
    #[serde(default)]
    pub average_ms: Option<f64>,
    #[serde(default)]
    pub min_ms: Option<f64>,
    #[serde(default)]
    pub max_ms: Option<f64>,
    #[serde(default)]
    pub total_requests: Option<u64>,
    #[serde(default, flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct CostStats {
    #[serde(default)]
    pub total_tokens_saved: Option<u64>,
    #[serde(default)]
    pub total_input_tokens: Option<u64>,
    #[serde(default)]
    pub total_input_cost_usd: Option<f64>,
    #[serde(default)]
    pub cache_write_5m_tokens: Option<u64>,
    #[serde(default)]
    pub cache_write_1h_tokens: Option<u64>,
    #[serde(default)]
    pub per_model: BTreeMap<String, ModelCostStats>,
    #[serde(default)]
    pub cost_with_headroom_usd: Option<f64>,
    #[serde(default)]
    pub savings_usd: Option<f64>,
    #[serde(default)]
    pub budget_limit_usd: Option<f64>,
    #[serde(default)]
    pub budget_period: Option<String>,
    #[serde(default)]
    pub compression_savings_usd: Option<f64>,
    #[serde(default)]
    pub cache_savings_usd: Option<f64>,
    #[serde(default)]
    pub cli_tokens_avoided: Option<u64>,
    #[serde(default)]
    pub cli_filtering_tokens_avoided: Option<u64>,
    #[serde(default)]
    pub cli_tokens_included_in_compression: Option<bool>,
    #[serde(default)]
    pub cli_filtering_tokens_included_in_compression: Option<bool>,
    #[serde(default, flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct ModelCostStats {
    #[serde(default)]
    pub requests: Option<u64>,
    #[serde(default)]
    pub tokens_saved: Option<u64>,
    #[serde(default)]
    pub tokens_sent: Option<u64>,
    #[serde(default)]
    pub cache_write_5m_tokens: Option<u64>,
    #[serde(default)]
    pub cache_write_1h_tokens: Option<u64>,
    #[serde(default)]
    pub reduction_pct: Option<f64>,
    #[serde(default, flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct PersistentSavingsStats {
    #[serde(default)]
    pub schema_version: Option<u64>,
    #[serde(default)]
    pub storage_path: Option<String>,
    #[serde(default)]
    pub lifetime: Option<SavingsRollup>,
    #[serde(default)]
    pub display_session: Option<SavingsRollup>,
    #[serde(default)]
    pub display_session_policy: Option<Value>,
    #[serde(default)]
    pub history_points: Option<u64>,
    #[serde(default)]
    pub recent_history: Vec<SavingsHistoryPoint>,
    #[serde(default)]
    pub retention: Option<Value>,
    #[serde(default)]
    pub projects: BTreeMap<String, SavingsRollup>,
    #[serde(default)]
    pub projects_limit: Option<u64>,
    #[serde(default, flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct SavingsRollup {
    #[serde(default)]
    pub requests: Option<u64>,
    #[serde(default)]
    pub tokens_saved: Option<u64>,
    #[serde(default)]
    pub cache_read_tokens: Option<u64>,
    #[serde(default)]
    pub cache_savings_usd: Option<f64>,
    #[serde(default)]
    pub compression_savings_usd: Option<f64>,
    #[serde(default)]
    pub total_input_tokens: Option<u64>,
    #[serde(default)]
    pub total_input_cost_usd: Option<f64>,
    #[serde(default)]
    pub savings_percent: Option<f64>,
    #[serde(default)]
    pub started_at: Option<String>,
    #[serde(default)]
    pub last_activity_at: Option<String>,
    #[serde(default, flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub(crate) struct ProjectSavingsTotals {
    pub(crate) tokens_saved: u64,
    pub(crate) savings_usd: Option<f64>,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct SavingsHistoryPoint {
    #[serde(default)]
    pub timestamp: Option<String>,
    #[serde(default)]
    pub provider: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub total_tokens_saved: Option<u64>,
    #[serde(default)]
    pub compression_savings_usd: Option<f64>,
    #[serde(default)]
    pub total_input_tokens: Option<u64>,
    #[serde(default)]
    pub total_input_cost_usd: Option<f64>,
    #[serde(default, flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct RecentRequestStats {
    #[serde(default)]
    pub request_id: Option<String>,
    #[serde(default)]
    pub timestamp: Option<String>,
    #[serde(default)]
    pub provider: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub input_tokens_original: Option<u64>,
    #[serde(default)]
    pub input_tokens_optimized: Option<u64>,
    #[serde(default)]
    pub output_tokens: Option<u64>,
    #[serde(default)]
    pub tokens_saved: Option<u64>,
    #[serde(default)]
    pub savings_percent: Option<f64>,
    #[serde(default)]
    pub optimization_latency_ms: Option<f64>,
    #[serde(default)]
    pub total_latency_ms: Option<f64>,
    #[serde(default)]
    pub transforms_applied: Vec<String>,
    #[serde(default)]
    pub waste_signals: Option<Value>,
    #[serde(default, flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Clone)]
pub struct SidecarManager {
    inner: Arc<SidecarInner>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum SidecarUpstream {
    OpenAi(String),
}

struct SidecarInner {
    requested_port: RwLock<u16>,
    port: RwLock<u16>,
    saved_port: RwLock<Option<u16>>,
    persisted_port: RwLock<Option<u16>>,
    saved_port_busy_logged: AtomicBool,
    status: RwLock<SidecarStatus>,
    child: Mutex<Option<ManagedChild>>,
    binary_path: RwLock<Option<PathBuf>>,
    app_data_dir: Arc<RwLock<Option<PathBuf>>>,
    resource_dir: RwLock<Option<PathBuf>>,
    stderr_lines: Arc<Mutex<VecDeque<String>>>,
    stderr_generation: Arc<AtomicU64>,
    version: RwLock<Option<Option<String>>>,
    op_lock: AsyncMutex<()>,
    terminated: AtomicBool,
    auto_heal_in_flight: AtomicBool,
    auto_heal_generation: AtomicU64,
    auto_heal_pending: AtomicBool,
    auto_heal_attempts: Mutex<VecDeque<Instant>>,
    auto_heal_task: Mutex<Option<tauri::async_runtime::JoinHandle<()>>>,
    stats_timeouts: AtomicU32,
    optimization_paused: AtomicBool,
    upstream: Option<SidecarUpstream>,
    client: reqwest::Client,
}

#[derive(Debug, PartialEq, Eq)]
enum AutoHealReservation {
    // The generation and the wait before the restart.
    Reserved(u64, Duration),
    InFlight,
    Terminated,
}

struct AutoHealInFlightReset {
    inner: Arc<SidecarInner>,
    generation: u64,
}

impl Drop for AutoHealInFlightReset {
    fn drop(&mut self) {
        let _task_slot = self
            .inner
            .auto_heal_task
            .lock()
            .expect("auto-heal task lock poisoned");
        if self.inner.auto_heal_generation.load(Ordering::SeqCst) == self.generation {
            self.inner
                .auto_heal_in_flight
                .store(false, Ordering::SeqCst);
        }
    }
}

impl SidecarManager {
    pub fn new(port: u16) -> Self {
        Self::new_with_upstream(port, None)
    }

    fn new_with_upstream(port: u16, upstream: Option<SidecarUpstream>) -> Self {
        // A single-worker proxy answers /stats slowly while it compresses a
        // large request; 2 seconds misread that as unhealthy. A timeout is
        // still classified as busy, never as a reason to restart.
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(6))
            .build()
            .expect("failed to build headroom stats HTTP client");

        Self {
            inner: Arc::new(SidecarInner {
                requested_port: RwLock::new(port),
                port: RwLock::new(port),
                saved_port: RwLock::new(None),
                persisted_port: RwLock::new(None),
                saved_port_busy_logged: AtomicBool::new(false),
                status: RwLock::new(SidecarStatus::Starting { port }),
                child: Mutex::new(None),
                binary_path: RwLock::new(None),
                app_data_dir: Arc::new(RwLock::new(None)),
                resource_dir: RwLock::new(None),
                stderr_lines: Arc::new(Mutex::new(VecDeque::new())),
                stderr_generation: Arc::new(AtomicU64::new(0)),
                version: RwLock::new(None),
                op_lock: AsyncMutex::new(()),
                terminated: AtomicBool::new(false),
                auto_heal_in_flight: AtomicBool::new(false),
                auto_heal_generation: AtomicU64::new(0),
                auto_heal_pending: AtomicBool::new(false),
                auto_heal_attempts: Mutex::new(VecDeque::new()),
                auto_heal_task: Mutex::new(None),
                stats_timeouts: AtomicU32::new(0),
                optimization_paused: AtomicBool::new(false),
                upstream,
                client,
            }),
        }
    }

    pub fn set_app_data_dir(&self, app_data_dir: PathBuf) {
        *self
            .inner
            .app_data_dir
            .write()
            .expect("app data dir lock poisoned") = Some(app_data_dir.clone());
        match read_saved_sidecar_port(&app_data_dir) {
            Ok(Some(port)) => {
                *self
                    .inner
                    .requested_port
                    .write()
                    .expect("requested port lock poisoned") = port;
                *self.inner.port.write().expect("port lock poisoned") = port;
                *self
                    .inner
                    .saved_port
                    .write()
                    .expect("saved port lock poisoned") = Some(port);
                *self
                    .inner
                    .persisted_port
                    .write()
                    .expect("persisted port lock poisoned") = Some(port);
                self.set_status(SidecarStatus::Starting { port });
            }
            Ok(None) => {}
            Err(error) => eprintln!("{error}"),
        }
    }

    pub fn set_resource_dir(&self, resource_dir: PathBuf) {
        *self
            .inner
            .resource_dir
            .write()
            .expect("resource dir lock poisoned") = Some(resource_dir);
    }

    pub async fn refresh_status(&self) -> SidecarStatus {
        let Ok(_guard) = self.inner.op_lock.try_lock() else {
            return self.current_status();
        };

        self.refresh_status_locked().await
    }

    pub async fn ensure_ready(&self) -> SidecarStatus {
        let _guard = self.inner.op_lock.lock().await;
        self.ensure_ready_locked().await
    }

    /// Always false: the Headroom proxy never pauses for the plan of the
    /// account (ADR 0021).
    pub fn optimization_paused(&self) -> bool {
        self.inner.optimization_paused.load(Ordering::SeqCst)
    }

    /// Start the immutable Headroom runtime shipped in this Halv build. The
    /// bundle is version-coupled to the app and is never replaced by uv/pipx.
    pub async fn startup(&self) {
        self.reap_orphaned_headroom_proxies();
        self.ensure_ready().await;
        self.spawn_supervisor();
    }

    /// Watches the managed child until shutdown. The UI poll used to be the
    /// only thing that noticed an exit; a closed window left the proxy dead.
    fn spawn_supervisor(&self) {
        let manager = self.clone();
        tauri::async_runtime::spawn(async move {
            while !manager.is_terminated() {
                tokio::time::sleep(SUPERVISOR_POLL_INTERVAL).await;
                manager.supervise_tick().await;
            }
        });
    }

    /// One supervisor pass: reap an exited child and schedule its restart.
    /// A pass skips when another lifecycle operation holds the lock.
    async fn supervise_tick(&self) {
        let Ok(_guard) = self.inner.op_lock.try_lock() else {
            return;
        };
        if self.is_terminated() {
            return;
        }
        let current = self.current_status();
        if matches!(
            &current,
            SidecarStatus::NotInstalled { .. } | SidecarStatus::Installing { .. }
        ) {
            return;
        }
        if let Some(status) = self.take_child_exit_status() {
            self.handle_managed_child_failure(status);
            return;
        }
        if matches!(&current, SidecarStatus::Error { .. })
            && self.inner.auto_heal_pending.load(Ordering::SeqCst)
            && !self.inner.auto_heal_in_flight.load(Ordering::SeqCst)
            && self.binary_is_managed_by_halv()
        {
            self.schedule_auto_heal(current);
        }
    }

    /// The error status of a child that exited, or `None` while it runs.
    fn take_child_exit_status(&self) -> Option<SidecarStatus> {
        let port = self.port();
        let exit = self.take_child_exit()?;
        Some(SidecarStatus::Error {
            message: exit.runtime_message(port),
            port,
        })
    }

    /// Writes one line per child exit to the supervisor log, so a field
    /// report can say why the proxy fell.
    fn record_child_exit(&self, exit: &ManagedChildExit) {
        let restarts = self
            .inner
            .auto_heal_attempts
            .lock()
            .map(|attempts| attempts.len())
            .unwrap_or(0);
        let line = supervisor_log_line(chrono::Utc::now(), self.port(), exit, restarts);
        eprintln!("{line}");
        let Some(app_data_dir) = self.app_data_dir() else {
            return;
        };
        if let Err(error) = append_supervisor_log(&app_data_dir, &line, SUPERVISOR_LOG_MAX_BYTES) {
            eprintln!("failed to write {SUPERVISOR_LOG_FILE_NAME}: {error}");
        }
    }

    /// Counts one /stats timeout. True when the run reached the restart
    /// threshold; the count then starts again.
    fn note_stats_timeout(&self) -> bool {
        let count = self.inner.stats_timeouts.fetch_add(1, Ordering::SeqCst) + 1;
        if count >= STATS_TIMEOUTS_BEFORE_RESTART {
            self.clear_stats_timeouts();
            return true;
        }
        false
    }

    fn clear_stats_timeouts(&self) {
        self.inner.stats_timeouts.store(0, Ordering::SeqCst);
    }

    /// Selects the start port and waits a short time for the saved port when
    /// it is still busy. The kernel closes the listener of the dead child a
    /// moment after the exit.
    async fn select_start_port_waiting(&self, preferred_port: u16) -> Result<u16, String> {
        if preferred_port == 0 {
            return self.select_start_port(preferred_port);
        }
        let deadline = Instant::now() + SAVED_PORT_WAIT;
        loop {
            let free = choose_available_loopback_port(Some(preferred_port))?.reused_preferred;
            if free || Instant::now() >= deadline || self.is_terminated() {
                return self.select_start_port(preferred_port);
            }
            tokio::time::sleep(SAVED_PORT_WAIT_STEP).await;
        }
    }

    async fn install_headroom(
        &self,
        resource_dir: PathBuf,
        progress: &impl InstallProgressSink,
    ) -> Result<SidecarStatus, SidecarInstallError> {
        self.cancel_auto_heal_task();
        let _guard = self.inner.op_lock.lock().await;
        self.cancel_auto_heal_task();
        let port = self.port();
        if self.is_terminated() {
            return Ok(self.set_terminated_status(port));
        }

        self.set_resource_dir(resource_dir.clone());
        self.set_status(SidecarStatus::Installing { port });
        progress.emit(SidecarInstallStage::Starting, "Setting up the proxy.");

        if bundled_headroom_path(&resource_dir).is_none() {
            let error = SidecarInstallError::single(
                SidecarInstallErrorCode::BundledRuntimeMissing,
                "This Halv build is missing its bundled proxy.",
                format!(
                    "Expected {} under {}.",
                    bundled_binary_name(),
                    resource_dir.join("headroom-runtime").display()
                ),
            );
            progress.emit(SidecarInstallStage::Error, error.message.clone());
            self.set_status(SidecarStatus::NotInstalled {
                install_hint: install_hint(),
                port,
            });
            return Err(error);
        }

        progress.emit(
            SidecarInstallStage::DetectingHeadroom,
            "Proxy found; starting it.",
        );
        self.kill_child();
        self.set_binary_path(None);
        self.clear_cached_version();
        let status = self.ensure_ready_locked().await;
        if matches!(&status, SidecarStatus::Running { .. }) {
            self.reset_auto_heal_after_manual_success();
        }
        match &status {
            SidecarStatus::Running { port, .. } => progress.emit(
                SidecarInstallStage::Success,
                format!("Proxy is running on port {port}."),
            ),
            SidecarStatus::Starting { port } => progress.emit(
                SidecarInstallStage::DetectingHeadroom,
                format!("Proxy is starting on port {port}."),
            ),
            SidecarStatus::NotInstalled { .. } => progress.emit(
                SidecarInstallStage::Error,
                "Proxy installed, but the binary was not found afterward.",
            ),
            SidecarStatus::Error { message, .. } => progress.emit(
                SidecarInstallStage::Error,
                format!("Proxy installed, but it did not start: {message}"),
            ),
            // ensure_ready_locked never yields Installing.
            SidecarStatus::Installing { .. } => {}
        }
        Ok(status)
    }

    async fn refresh_status_locked(&self) -> SidecarStatus {
        let port = self.port();
        if self.is_terminated() {
            return self.current_status();
        }
        let current = self.current_status();
        if matches!(
            &current,
            SidecarStatus::NotInstalled { .. } | SidecarStatus::Installing { .. }
        ) {
            return current;
        }
        if let Some(status) = self.take_child_exit_status() {
            return self.handle_managed_child_failure(status);
        }
        if self.inner.auto_heal_pending.load(Ordering::SeqCst) && self.binary_is_managed_by_halv() {
            if self.inner.auto_heal_in_flight.load(Ordering::SeqCst) {
                return current;
            }
            if matches!(&current, SidecarStatus::Error { .. }) {
                return self.schedule_auto_heal(current);
            }
        }
        if !self.has_managed_child_slot() {
            let status = match current {
                SidecarStatus::NotInstalled { .. }
                | SidecarStatus::Installing { .. }
                | SidecarStatus::Error { .. } => current,
                _ => SidecarStatus::Error {
                    message: unmanaged_endpoint_message(port),
                    port,
                },
            };
            self.set_status(status.clone());
            return status;
        }

        match self.fetch_managed_stats_classified().await {
            Ok(stats) => match self.set_running_from_stats(&stats, port, None).await {
                Ok(status) => status,
                Err(error) => {
                    let status = SidecarStatus::Error {
                        message: error,
                        port,
                    };
                    self.handle_managed_child_failure(status)
                }
            },
            // A slow answer is a busy single worker, not a dead one. The
            // child exit check above catches a dead one at once.
            Err(error) if error.busy && !self.note_stats_timeout() => current,
            Err(error) => {
                let message = self.managed_runtime_message(
                    port,
                    format!("managed proxy stopped responding: {}", error.message),
                );
                let status = SidecarStatus::Error { message, port };
                self.handle_managed_child_failure(status)
            }
        }
    }

    async fn ensure_ready_locked(&self) -> SidecarStatus {
        let current_port = self.port();
        if self.is_terminated() {
            return self.set_terminated_status(current_port);
        }

        if self.managed_child_is_running() {
            match self.fetch_managed_stats_classified().await {
                Ok(stats) => {
                    if let Ok(status) = self
                        .set_running_from_stats(&stats, current_port, None)
                        .await
                    {
                        return status;
                    }
                }
                Err(error) if error.busy => {
                    // The worker is busy with a live request. Killing it here
                    // dropped in-flight agent calls and wiped the metrics;
                    // report the current status and let the child finish.
                    eprintln!(
                        "{}",
                        self.managed_runtime_message(
                            current_port,
                            format!(
                                "Managed Headroom on port {current_port} is busy; keeping it: {}",
                                error.message
                            ),
                        )
                    );
                    return self.current_status();
                }
                Err(error) => {
                    eprintln!(
                        "{}",
                        self.managed_runtime_message(
                            current_port,
                            format!(
                            "Managed Headroom on port {current_port} is unhealthy; restarting: {}",
                            error.message
                        ),
                        )
                    );
                }
            }
            self.kill_child();
        } else if self.has_managed_child_slot() {
            eprintln!(
                "{}",
                self.managed_runtime_message(
                    current_port,
                    format!(
                        "managed headroom proxy was found dead on port {current_port}; restarting"
                    ),
                )
            );
            self.kill_child();
        }

        let Some(binary_path) =
            locate_headroom_binary_async(self.app_data_dir(), self.resource_dir()).await
        else {
            let status = SidecarStatus::NotInstalled {
                install_hint: install_hint(),
                port: current_port,
            };
            self.set_status(status.clone());
            return status;
        };

        self.set_binary_path(Some(binary_path.clone()));
        self.clear_cached_version();
        let version = read_headroom_version_async(binary_path.clone()).await;
        self.cache_version_outcome(version.clone());

        if self.is_terminated() {
            return self.set_terminated_status(current_port);
        }

        let mut last_error = "managed proxy did not start".to_string();
        for attempt in 0..START_ATTEMPTS {
            // Every attempt keeps the saved port. Agents from this run and
            // the previous one reach the proxy only there.
            let preferred_port = self.requested_port();
            let port = match self.select_start_port_waiting(preferred_port).await {
                Ok(port) => port,
                Err(error) => {
                    let status = SidecarStatus::Error {
                        message: error,
                        port: self.port(),
                    };
                    self.set_status(status.clone());
                    return status;
                }
            };
            self.set_status(SidecarStatus::Starting { port });

            match self.spawn_headroom_proxy(&binary_path, port) {
                Ok(child) => {
                    if self.is_terminated() {
                        stop_child(child);
                        return self.set_terminated_status(port);
                    }
                    self.replace_child(Some(child));
                }
                Err(error) => {
                    let status = SidecarStatus::Error {
                        message: format!("failed to spawn managed proxy: {error}"),
                        port,
                    };
                    self.set_status(status.clone());
                    return status;
                }
            }

            match self.wait_for_ready(READINESS_TIMEOUT).await {
                Ok(stats) => match self
                    .set_running_from_stats(&stats, port, Some(binary_path.clone()))
                    .await
                {
                    Ok(status) => return status,
                    Err(error) => last_error = error,
                },
                Err(error) => last_error = error,
            }

            if attempt + 1 < START_ATTEMPTS {
                self.kill_child();
            }
        }

        self.kill_child();
        let port = self.port();
        let message = if last_error.contains(LAST_OUTPUT_MARKER) {
            last_error
        } else {
            truncate_child_output(&format!(
                "managed proxy did not become ready after {START_ATTEMPTS} attempts: {last_error}"
            ))
        };
        let status = SidecarStatus::Error { message, port };
        self.set_status(status.clone());
        status
    }

    pub async fn restart(&self) -> SidecarStatus {
        self.cancel_auto_heal_task();
        let _guard = self.inner.op_lock.lock().await;
        self.cancel_auto_heal_task();
        self.restart_locked().await
    }

    async fn restart_locked(&self) -> SidecarStatus {
        let port = self.port();
        if self.is_terminated() {
            return self.set_terminated_status(port);
        }

        self.kill_child();
        self.clear_cached_version();
        let status = self.ensure_ready_locked().await;
        if matches!(&status, SidecarStatus::Running { .. }) {
            self.reset_auto_heal_after_manual_success();
        }
        status
    }

    pub async fn live_stats(
        &self,
        lane_by_project: &HashMap<String, SessionLaneKind>,
    ) -> Result<SidecarStats, String> {
        let _guard = self.inner.op_lock.lock().await;
        let port = self.port();
        let stats = match self.fetch_managed_stats().await {
            Ok(stats) => stats,
            Err(error) => {
                let message = self
                    .managed_runtime_message(port, format!("managed proxy unavailable: {error}"));
                self.kill_child();
                self.set_status(SidecarStatus::Error {
                    message: message.clone(),
                    port,
                });
                return Err(message);
            }
        };

        self.set_running_from_stats(&stats, port, None).await?;
        let mut sidecar_stats =
            stats.to_sidecar_stats(port, self.proxy_instance_generation(), lane_by_project);
        if let Some(exact_rtk_tokens_saved) = crate::exec::rtk_tokens_saved() {
            sidecar_stats.replace_rtk_tokens_saved(exact_rtk_tokens_saved);
        }
        Ok(sidecar_stats)
    }

    pub async fn project_savings_totals(
        &self,
        project: &str,
    ) -> Result<ProjectSavingsTotals, String> {
        let _guard = self.inner.op_lock.lock().await;
        let stats = self.fetch_managed_stats().await?;
        Ok(stats.project_savings_totals(project))
    }

    /// (requests, tokens_saved, cache_read_tokens) for one project. Request
    /// growth places a zero-savings point per message; cache reads feed the
    /// context-reuse display.
    pub async fn project_savings_snapshot(&self, project: &str) -> Result<(u64, u64, u64), String> {
        let _guard = self.inner.op_lock.lock().await;
        let stats = self.fetch_managed_stats().await?;
        Ok(stats.project_savings_snapshot(project))
    }

    pub(crate) async fn start_isolated_upstream(
        &self,
        upstream: SidecarUpstream,
    ) -> Result<Self, String> {
        let manager = Self::new_with_upstream(0, Some(upstream));
        if let Some(resource_dir) = self.resource_dir() {
            manager.set_resource_dir(resource_dir);
        }

        match manager.ensure_ready().await {
            SidecarStatus::Running { .. } => Ok(manager),
            SidecarStatus::NotInstalled { install_hint, .. } => Err(install_hint),
            SidecarStatus::Error { message, .. } => Err(message),
            SidecarStatus::Installing { .. } | SidecarStatus::Starting { .. } => {
                Err("isolated Headroom proxy did not become ready".to_string())
            }
        }
    }

    pub fn shutdown(&self) {
        self.inner.terminated.store(true, Ordering::SeqCst);
        self.cancel_auto_heal_task();
        self.reap_orphaned_headroom_proxies();
        self.kill_child();
    }

    async fn wait_for_ready(&self, timeout: Duration) -> Result<HeadroomStats, String> {
        let deadline = Instant::now() + timeout;

        loop {
            if let Some(exit) = self.take_child_exit() {
                return Err(exit.startup_message(
                    self.port(),
                    "managed child exited before its readiness probe",
                ));
            }
            if !self.has_managed_child_slot() {
                return Err(unmanaged_endpoint_message(self.port()));
            }

            let first_probe = self.fetch_stats_unchecked().await;
            if first_probe.is_ok() {
                tokio::time::sleep(POLL_INTERVAL).await;
                if let Some(exit) = self.take_child_exit() {
                    return Err(exit.startup_message(
                        self.port(),
                        "a different listener answered /stats while the managed child exited",
                    ));
                }
                if !self.has_managed_child_slot() {
                    return Err(unmanaged_endpoint_message(self.port()));
                }

                match self.fetch_stats_unchecked().await {
                    Ok(stats) => {
                        if let Some(exit) = self.take_child_exit() {
                            return Err(exit.startup_message(
                                self.port(),
                                "managed child exited after the readiness probe",
                            ));
                        }
                        if self.has_managed_child_slot() {
                            return Ok(stats);
                        }
                        return Err(unmanaged_endpoint_message(self.port()));
                    }
                    Err(error) if Instant::now() >= deadline => {
                        return Err(error.to_string());
                    }
                    Err(_) => {}
                }
            } else if let Some(exit) = self.take_child_exit() {
                return Err(exit.startup_message(
                    self.port(),
                    &first_probe
                        .expect_err("failed probe should contain an error")
                        .to_string(),
                ));
            } else if Instant::now() >= deadline {
                return Err(first_probe
                    .expect_err("failed probe should contain an error")
                    .to_string());
            }

            if Instant::now() >= deadline {
                return Err("managed proxy readiness timed out".to_string());
            }
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }

    async fn fetch_managed_stats(&self) -> Result<HeadroomStats, String> {
        self.fetch_managed_stats_classified()
            .await
            .map_err(|error| error.message)
    }

    async fn fetch_managed_stats_classified(&self) -> Result<HeadroomStats, FetchStatsError> {
        let port = self.port();
        if !self.managed_child_is_running() {
            return Err(FetchStatsError::unhealthy(unmanaged_endpoint_message(port)));
        }

        let stats = self.fetch_stats_unchecked().await?;
        if !self.managed_child_is_running() {
            return Err(FetchStatsError::unhealthy(format!(
                "managed proxy exited while querying /stats on port {port}"
            )));
        }
        self.clear_stats_timeouts();
        Ok(stats)
    }

    async fn fetch_stats_unchecked(&self) -> Result<HeadroomStats, FetchStatsError> {
        let port = self.port();
        let response = self
            .inner
            .client
            .get(stats_url(port))
            .send()
            .await
            .map_err(|error| FetchStatsError::from_request_error(port, error))?;

        let status = response.status();
        if !status.is_success() {
            return Err(FetchStatsError::unhealthy(format!(
                "GET /stats returned HTTP {status} on port {port}"
            )));
        }

        let stats = response.json::<HeadroomStats>().await.map_err(|error| {
            FetchStatsError::unhealthy(format!(
                "failed to decode /stats JSON on port {port}: {error}"
            ))
        })?;

        validate_headroom_stats(stats, port)
    }

    fn requested_port(&self) -> u16 {
        *self
            .inner
            .requested_port
            .read()
            .expect("requested port lock poisoned")
    }

    pub(crate) fn port(&self) -> u16 {
        *self.inner.port.read().expect("port lock poisoned")
    }

    fn proxy_instance_generation(&self) -> u64 {
        self.inner.stderr_generation.load(Ordering::SeqCst)
    }

    fn select_start_port(&self, preferred_port: u16) -> Result<u16, String> {
        let selection =
            choose_available_loopback_port((preferred_port != 0).then_some(preferred_port))?;
        if !selection.reused_preferred
            && *self
                .inner
                .saved_port
                .read()
                .expect("saved port lock poisoned")
                == Some(preferred_port)
            && !self
                .inner
                .saved_port_busy_logged
                .swap(true, Ordering::SeqCst)
        {
            eprintln!(
                "headroom port {preferred_port} was busy; agents from the previous run will not reach the proxy"
            );
        }
        let port = selection.port;
        *self.inner.port.write().expect("port lock poisoned") = port;
        Ok(port)
    }

    fn persist_selected_port(&self, port: u16) {
        *self
            .inner
            .requested_port
            .write()
            .expect("requested port lock poisoned") = port;
        if *self
            .inner
            .persisted_port
            .read()
            .expect("persisted port lock poisoned")
            == Some(port)
        {
            return;
        }
        let Some(app_data_dir) = self.app_data_dir() else {
            return;
        };
        match write_saved_sidecar_port(&app_data_dir, port) {
            Ok(()) => {
                *self
                    .inner
                    .persisted_port
                    .write()
                    .expect("persisted port lock poisoned") = Some(port);
            }
            Err(error) => eprintln!("{error}"),
        }
    }

    fn set_status(&self, status: SidecarStatus) {
        *self.inner.status.write().expect("status lock poisoned") = status;
    }

    fn handle_managed_child_failure(&self, status: SidecarStatus) -> SidecarStatus {
        if !self.binary_is_managed_by_halv() {
            self.kill_child();
            self.set_status(status.clone());
            return status;
        }

        self.inner.auto_heal_pending.store(true, Ordering::SeqCst);
        self.set_status(status.clone());
        self.schedule_auto_heal(status)
    }

    fn schedule_auto_heal(&self, status: SidecarStatus) -> SidecarStatus {
        self.schedule_auto_heal_at(status, Instant::now())
    }

    fn schedule_auto_heal_at(&self, status: SidecarStatus, now: Instant) -> SidecarStatus {
        let port = match &status {
            SidecarStatus::Error { port, .. } => *port,
            SidecarStatus::NotInstalled { .. }
            | SidecarStatus::Installing { .. }
            | SidecarStatus::Running { .. }
            | SidecarStatus::Starting { .. } => return status,
        };
        let mut task_slot = self
            .inner
            .auto_heal_task
            .lock()
            .expect("auto-heal task lock poisoned");

        match self.reserve_auto_heal_at(now) {
            AutoHealReservation::Reserved(generation, delay) => {
                let starting = SidecarStatus::Starting { port };
                self.set_status(starting.clone());
                let manager = self.clone();
                let reset = AutoHealInFlightReset {
                    inner: self.inner.clone(),
                    generation,
                };
                *task_slot = Some(tauri::async_runtime::spawn(async move {
                    let _reset = reset;
                    // The lock covers the wait, so a status poll does not
                    // report the gap as an unmanaged listener. A manual
                    // restart cancels this task first.
                    let _guard = manager.inner.op_lock.lock().await;
                    if !delay.is_zero() {
                        eprintln!(
                            "managed proxy restart on port {port} waits {}s",
                            delay.as_secs()
                        );
                        tokio::time::sleep(delay).await;
                    }
                    if manager.is_terminated() {
                        return;
                    }
                    manager.ensure_ready_locked().await;
                }));
                starting
            }
            AutoHealReservation::InFlight => self.current_status(),
            AutoHealReservation::Terminated => self.current_status(),
        }
    }

    fn reserve_auto_heal_at(&self, now: Instant) -> AutoHealReservation {
        if self.is_terminated() {
            return AutoHealReservation::Terminated;
        }

        let mut attempts = self
            .inner
            .auto_heal_attempts
            .lock()
            .expect("auto-heal attempts lock poisoned");
        while attempts
            .front()
            .is_some_and(|attempt| now.saturating_duration_since(*attempt) >= AUTO_HEAL_WINDOW)
        {
            attempts.pop_front();
        }
        let delay = auto_heal_backoff(attempts.len());
        if self
            .inner
            .auto_heal_in_flight
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return AutoHealReservation::InFlight;
        }
        if self.is_terminated() {
            self.inner
                .auto_heal_in_flight
                .store(false, Ordering::SeqCst);
            return AutoHealReservation::Terminated;
        }

        let generation = self
            .inner
            .auto_heal_generation
            .fetch_add(1, Ordering::SeqCst)
            .wrapping_add(1);
        attempts.push_back(now);
        AutoHealReservation::Reserved(generation, delay)
    }

    fn cancel_auto_heal_task(&self) {
        self.inner
            .auto_heal_generation
            .fetch_add(1, Ordering::SeqCst);
        let task = self
            .inner
            .auto_heal_task
            .lock()
            .expect("auto-heal task lock poisoned")
            .take();
        if let Some(task) = task {
            task.abort();
        }
        self.inner
            .auto_heal_in_flight
            .store(false, Ordering::SeqCst);
    }

    fn reset_auto_heal_after_manual_success(&self) {
        self.inner
            .auto_heal_attempts
            .lock()
            .expect("auto-heal attempts lock poisoned")
            .clear();
        self.inner.auto_heal_pending.store(false, Ordering::SeqCst);
    }

    fn binary_is_managed_by_halv(&self) -> bool {
        is_halv_managed_binary_path(
            self.binary_path().as_deref(),
            self.app_data_dir().as_deref(),
            self.resource_dir().as_deref(),
        )
    }

    async fn set_running_from_stats(
        &self,
        stats: &HeadroomStats,
        port: u16,
        binary_path: Option<PathBuf>,
    ) -> Result<SidecarStatus, String> {
        if self.port() != port {
            return Err(unmanaged_endpoint_message(port));
        }
        if !self.managed_child_is_running() {
            return Err(self.managed_runtime_message(port, unmanaged_endpoint_message(port)));
        }
        let version = self.resolve_version(stats, binary_path).await;
        if self.port() != port {
            return Err(unmanaged_endpoint_message(port));
        }
        if !self.managed_child_is_running() {
            return Err(self.managed_runtime_message(port, unmanaged_endpoint_message(port)));
        }
        let managed_by_halv = is_halv_managed_binary_path(
            self.binary_path().as_deref(),
            self.app_data_dir().as_deref(),
            self.resource_dir().as_deref(),
        );
        let status = SidecarStatus::Running {
            spawned_by_us: true,
            managed_by_halv,
            optimization_paused: self.optimization_paused(),
            version,
            pinned_version: pinned_headroom_version().to_string(),
            port,
        };
        self.persist_selected_port(port);
        self.inner.auto_heal_pending.store(false, Ordering::SeqCst);
        self.set_status(status.clone());
        Ok(status)
    }

    fn current_status(&self) -> SidecarStatus {
        self.inner
            .status
            .read()
            .expect("status lock poisoned")
            .clone()
    }

    fn is_terminated(&self) -> bool {
        self.inner.terminated.load(Ordering::SeqCst)
    }

    fn set_terminated_status(&self, port: u16) -> SidecarStatus {
        let status = SidecarStatus::Error {
            message: "Proxy is shutting down".to_string(),
            port,
        };
        self.set_status(status.clone());
        status
    }

    fn set_binary_path(&self, path: Option<PathBuf>) {
        *self
            .inner
            .binary_path
            .write()
            .expect("binary path lock poisoned") = path;
    }

    fn binary_path(&self) -> Option<PathBuf> {
        self.inner
            .binary_path
            .read()
            .expect("binary path lock poisoned")
            .clone()
    }

    fn app_data_dir(&self) -> Option<PathBuf> {
        self.inner
            .app_data_dir
            .read()
            .expect("app data dir lock poisoned")
            .clone()
    }

    fn resource_dir(&self) -> Option<PathBuf> {
        self.inner
            .resource_dir
            .read()
            .expect("resource dir lock poisoned")
            .clone()
    }

    fn cache_version(&self, version: Option<String>) {
        self.cache_version_outcome(version);
    }

    fn cache_version_outcome(&self, version: Option<String>) {
        *self.inner.version.write().expect("version lock poisoned") = Some(version);
    }

    fn cached_version(&self) -> Option<Option<String>> {
        self.inner
            .version
            .read()
            .expect("version lock poisoned")
            .clone()
    }

    fn clear_cached_version(&self) {
        *self.inner.version.write().expect("version lock poisoned") = None;
    }

    fn managed_child_is_running(&self) -> bool {
        let mut child_slot = self.inner.child.lock().expect("child lock poisoned");
        let Some(child) = child_slot.as_mut() else {
            return false;
        };
        matches!(child.process.try_wait(), Ok(None))
    }

    fn reap_orphaned_headroom_proxies(&self) {
        #[cfg(windows)]
        {
            // Windows has neither the pgrep/ps inspection contract nor /proc;
            // skip reaping rather than risk identifying an unrelated process.
            return;
        }

        #[cfg(unix)]
        {
            let mut child_slot = self.inner.child.lock().expect("child lock poisoned");
            let current_child_pid =
                child_slot
                    .as_mut()
                    .and_then(|child| match child.process.try_wait() {
                        Ok(Some(_)) => None,
                        Ok(None) | Err(_) => Some(child.process.id()),
                    });
            reap_orphaned_headroom_proxies(current_child_pid);
        }
    }

    fn has_managed_child_slot(&self) -> bool {
        self.inner
            .child
            .lock()
            .expect("child lock poisoned")
            .is_some()
    }

    fn take_child_exit(&self) -> Option<ManagedChildExit> {
        let (status, mut child) = {
            let mut child_slot = self.inner.child.lock().expect("child lock poisoned");
            let child = child_slot.as_mut()?;
            let status = match child.process.try_wait() {
                Ok(Some(status)) => status,
                Ok(None) | Err(_) => return None,
            };
            let child = child_slot.take().expect("exited child should be present");
            (status, child)
        };
        child.finish_stderr_drain();
        let exit = ManagedChildExit {
            status,
            stderr_lines: stderr_tail_snapshot(&self.inner.stderr_lines),
        };
        self.record_child_exit(&exit);
        Some(exit)
    }

    fn managed_runtime_message(&self, port: u16, fallback: String) -> String {
        self.take_child_exit()
            .map(|exit| exit.runtime_message(port))
            .unwrap_or_else(|| {
                message_with_stderr_tail(fallback, &stderr_tail_snapshot(&self.inner.stderr_lines))
            })
    }

    fn spawn_headroom_proxy(&self, binary_path: &Path, port: u16) -> Result<ManagedChild, String> {
        spawn_headroom_proxy_with_log_limit(
            binary_path,
            port,
            Arc::clone(&self.inner.stderr_lines),
            Arc::clone(&self.inner.stderr_generation),
            Arc::clone(&self.inner.app_data_dir),
            HEADROOM_PROXY_LOG_MAX_BYTES,
            self.optimization_paused(),
            self.inner.upstream.as_ref(),
        )
    }

    fn kill_child(&self) {
        self.replace_child(None);
    }

    fn replace_child(&self, next_child: Option<ManagedChild>) {
        let mut child_slot = self.inner.child.lock().expect("child lock poisoned");
        if let Some(child) = child_slot.take() {
            stop_child(child);
        }
        *child_slot = next_child;
    }

    async fn resolve_version(
        &self,
        stats: &HeadroomStats,
        binary_path: Option<PathBuf>,
    ) -> Option<String> {
        if let Some(version) = stats.reported_version() {
            self.cache_version(Some(version.clone()));
            return Some(version);
        }

        if let Some(version) = self.cached_version() {
            return version;
        }

        let binary_path = binary_path.or_else(|| {
            self.inner
                .binary_path
                .read()
                .ok()
                .and_then(|path| path.clone())
        });
        let binary_path = match binary_path {
            Some(path) => Some(path),
            None => locate_headroom_binary_async(self.app_data_dir(), self.resource_dir()).await,
        };

        if let Some(path) = binary_path.clone() {
            self.set_binary_path(Some(path.clone()));
        }

        let version = match binary_path {
            Some(path) => read_headroom_version_async(path).await,
            None => None,
        };
        self.cache_version(version.clone());
        version
    }
}

impl Drop for SidecarInner {
    fn drop(&mut self) {
        if let Ok(mut child_slot) = self.child.lock() {
            if let Some(child) = child_slot.take() {
                stop_child(child);
            }
        }
    }
}

#[derive(Debug, Clone)]
struct FetchStatsError {
    message: String,
    // A timed-out probe means the single worker is busy with a request,
    // not that the child died; the caller must not restart it for that.
    busy: bool,
}

impl FetchStatsError {
    fn from_request_error(port: u16, error: reqwest::Error) -> Self {
        Self {
            message: format!("GET /stats failed on port {port}: {error}"),
            busy: error.is_timeout(),
        }
    }

    fn unhealthy(message: String) -> Self {
        Self {
            message,
            busy: false,
        }
    }
}

impl std::fmt::Display for FetchStatsError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

struct ManagedChild {
    process: Child,
    stderr_drain: Option<std::thread::JoinHandle<()>>,
    stderr_done: Option<mpsc::Receiver<()>>,
}

impl ManagedChild {
    fn finish_stderr_drain(&mut self) {
        let finished = match self.stderr_done.take() {
            Some(done) => matches!(
                done.recv_timeout(STDERR_DRAIN_FINISH_TIMEOUT),
                Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected)
            ),
            None => true,
        };
        let Some(stderr_drain) = self.stderr_drain.take() else {
            return;
        };
        if finished {
            let _ = stderr_drain.join();
        }
    }
}

#[derive(Debug)]
struct ManagedChildExit {
    status: ExitStatus,
    stderr_lines: Vec<String>,
}

impl ManagedChildExit {
    fn runtime_message(&self, port: u16) -> String {
        message_with_stderr_tail(
            format!("managed proxy exited ({}) on port {port}", self.status),
            &self.stderr_lines,
        )
    }

    fn startup_message(&self, port: u16, last_probe: &str) -> String {
        message_with_stderr_tail(
            format!(
                "managed proxy exited ({}) before becoming ready on selected port {port}. Last readiness probe: {last_probe}",
                self.status
            ),
            &self.stderr_lines,
        )
    }
}

fn validate_headroom_stats(
    stats: HeadroomStats,
    port: u16,
) -> Result<HeadroomStats, FetchStatsError> {
    if stats.looks_like_headroom() {
        Ok(stats)
    } else {
        Err(FetchStatsError::unhealthy(format!(
            "service on port {port} answered /stats but does not look like a managed proxy"
        )))
    }
}

fn stop_child(mut child: ManagedChild) {
    let _ = child.process.kill();
    let _ = child.process.wait();
    child.finish_stderr_drain();
}

#[cfg(any(unix, test))]
#[derive(Debug, Clone, PartialEq, Eq)]
struct ReapCandidate {
    pid: u32,
    ppid: Option<u32>,
    parent_alive: Option<bool>,
    marker_present: bool,
    marker_check_unavailable: bool,
    exact_command_shape: bool,
}

#[cfg(any(unix, test))]
fn orphaned_managed_sidecar_pids(
    candidates: &[ReapCandidate],
    current_child_pid: Option<u32>,
) -> Vec<u32> {
    candidates
        .iter()
        .filter(|candidate| {
            let is_orphaned = candidate.ppid == Some(1) || candidate.parent_alive == Some(false);
            let is_managed = candidate.marker_present
                || (candidate.marker_check_unavailable
                    && candidate.exact_command_shape
                    && candidate.ppid == Some(1));

            current_child_pid != Some(candidate.pid) && is_orphaned && is_managed
        })
        .map(|candidate| candidate.pid)
        .collect()
}

#[cfg(unix)]
fn reap_orphaned_headroom_proxies(current_child_pid: Option<u32>) {
    let started = Instant::now();
    let inspection_deadline = started + ORPHAN_REAP_INSPECTION_BUDGET;
    let operation_deadline = started + ORPHAN_REAP_TOTAL_BUDGET;
    let candidate_pids = candidate_headroom_proxy_pids(inspection_deadline);
    let mut term_sent = Vec::with_capacity(candidate_pids.len());
    for pid in candidate_pids {
        if Instant::now() >= inspection_deadline {
            break;
        }
        let Some(candidate) = inspect_reap_candidate(pid, inspection_deadline) else {
            continue;
        };
        if orphaned_managed_sidecar_pids(&[candidate], current_child_pid) == [pid]
            && send_unix_signal(pid, libc::SIGTERM)
        {
            term_sent.push(pid);
        }
    }
    if term_sent.is_empty() {
        return;
    }

    let requested_grace_deadline = Instant::now() + ORPHAN_REAP_TERM_GRACE;
    let grace_deadline = requested_grace_deadline.min(operation_deadline);
    while Instant::now() < grace_deadline && term_sent.iter().copied().any(unix_process_is_alive) {
        let remaining = grace_deadline.saturating_duration_since(Instant::now());
        std::thread::sleep(remaining.min(ORPHAN_REAP_POLL_INTERVAL));
    }

    let mut reaped = Vec::with_capacity(term_sent.len());
    for pid in term_sent.iter().copied() {
        if Instant::now() >= operation_deadline {
            break;
        }
        if !unix_process_is_alive(pid) {
            reaped.push(pid);
            continue;
        }
        let Some(candidate) = inspect_reap_candidate(pid, operation_deadline) else {
            continue;
        };
        if orphaned_managed_sidecar_pids(&[candidate], current_child_pid) == [pid]
            && send_unix_signal(pid, libc::SIGKILL)
        {
            reaped.push(pid);
        }
    }

    eprintln!(
        "Reaped orphaned Halv-managed Headroom proxy pids: {:?}",
        reaped
    );
}

#[cfg(unix)]
fn candidate_headroom_proxy_pids(deadline: Instant) -> Vec<u32> {
    let mut command = Command::new("pgrep");
    command.args(["-f", HEADROOM_PROXY_PGREP_PATTERN]);
    let Some(output) = command_output_before_deadline(command, deadline) else {
        return Vec::new();
    };

    let mut pids = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| line.trim().parse::<u32>().ok())
        .collect::<Vec<_>>();
    pids.sort_unstable();
    pids.dedup();
    pids
}

#[cfg(unix)]
fn inspect_reap_candidate(pid: u32, deadline: Instant) -> Option<ReapCandidate> {
    let (ppid, command, marker_present, marker_check_unavailable) =
        inspect_reap_process(pid, deadline)?;
    let parent_alive = if ppid <= 1 {
        Some(true)
    } else {
        Some(unix_process_is_alive(ppid))
    };

    Some(ReapCandidate {
        pid,
        ppid: Some(ppid),
        parent_alive,
        marker_present,
        marker_check_unavailable,
        exact_command_shape: has_exact_headroom_proxy_command_shape(&command),
    })
}

#[cfg(target_os = "macos")]
fn inspect_reap_process(pid: u32, deadline: Instant) -> Option<(u32, String, bool, bool)> {
    if let Some((ppid, command)) = read_ps_process(pid, true, deadline) {
        let marker_present = command
            .split_whitespace()
            .any(|field| field == HALV_MANAGED_SIDECAR_MARKER);
        return Some((ppid, command, marker_present, false));
    }

    let (ppid, command) = read_ps_process(pid, false, deadline)?;
    Some((ppid, command, false, true))
}

#[cfg(target_os = "linux")]
fn inspect_reap_process(pid: u32, deadline: Instant) -> Option<(u32, String, bool, bool)> {
    let (ppid, command) = read_ps_process(pid, false, deadline)?;
    let expected_marker = HALV_MANAGED_SIDECAR_MARKER.as_bytes();
    let (marker_present, marker_check_unavailable) = match fs::read(format!("/proc/{pid}/environ"))
    {
        Ok(environment) => (
            environment
                .split(|byte| *byte == 0)
                .any(|entry| entry == expected_marker),
            false,
        ),
        Err(_) => (false, true),
    };
    Some((ppid, command, marker_present, marker_check_unavailable))
}

#[cfg(all(unix, not(any(target_os = "macos", target_os = "linux"))))]
fn inspect_reap_process(pid: u32, deadline: Instant) -> Option<(u32, String, bool, bool)> {
    let (ppid, command) = read_ps_process(pid, false, deadline)?;
    Some((ppid, command, false, true))
}

#[cfg(unix)]
fn read_ps_process(
    pid: u32,
    include_environment: bool,
    deadline: Instant,
) -> Option<(u32, String)> {
    let pid = pid.to_string();
    let mut command = Command::new("ps");
    if include_environment {
        command.arg("-E");
    }
    command.args(["-p", &pid, "-o", "ppid=", "-o", "command="]);
    let output = command_output_before_deadline(command, deadline)?;
    if !output.success {
        return None;
    }

    let output = String::from_utf8_lossy(&output.stdout);
    let mut fields = output.trim().splitn(2, char::is_whitespace);
    let ppid = fields.next()?.parse::<u32>().ok()?;
    let command = fields.next().unwrap_or_default().trim().to_string();
    Some((ppid, command))
}

#[cfg(unix)]
fn has_exact_headroom_proxy_command_shape(command: &str) -> bool {
    let fields = command.split_whitespace().collect::<Vec<_>>();
    let argument_offset = if fields.get(2) == Some(&"--no-optimize") {
        3
    } else {
        2
    };
    if fields.len() != argument_offset + 7 {
        return false;
    }
    let port_is_valid = fields[argument_offset + 3]
        .parse::<u16>()
        .is_ok_and(|port| port != 0);

    Path::new(fields[0]).file_name() == Some(OsStr::new("headroom"))
        && fields[1] == "proxy"
        && fields[argument_offset] == "--host"
        && fields[argument_offset + 1] == "127.0.0.1"
        && fields[argument_offset + 2] == "--port"
        && port_is_valid
        && fields[argument_offset + 4] == "--workers"
        && fields[argument_offset + 5] == "1"
        && fields[argument_offset + 6] == "--no-http2"
}

#[cfg(unix)]
struct DeadlineCommandOutput {
    success: bool,
    stdout: Vec<u8>,
}

#[cfg(unix)]
fn command_output_before_deadline(
    mut command: Command,
    deadline: Instant,
) -> Option<DeadlineCommandOutput> {
    if Instant::now() >= deadline {
        return None;
    }
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut child = command.spawn().ok()?;

    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let mut stdout = Vec::new();
                child.stdout.take()?.read_to_end(&mut stdout).ok()?;
                return Some(DeadlineCommandOutput {
                    success: status.success(),
                    stdout,
                });
            }
            Ok(None) if Instant::now() < deadline => {
                let remaining = deadline.saturating_duration_since(Instant::now());
                std::thread::sleep(remaining.min(ORPHAN_REAP_POLL_INTERVAL));
            }
            Ok(None) | Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
}

#[cfg(unix)]
fn send_unix_signal(pid: u32, signal: libc::c_int) -> bool {
    let Ok(pid) = libc::pid_t::try_from(pid) else {
        return false;
    };
    unsafe { libc::kill(pid, signal) == 0 }
}

#[cfg(unix)]
fn unix_process_is_alive(pid: u32) -> bool {
    let Ok(pid) = libc::pid_t::try_from(pid) else {
        return false;
    };
    if unsafe { libc::kill(pid, 0) } == 0 {
        return true;
    }
    std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

#[cfg(test)]
fn record_stderr_line(stderr_lines: &Mutex<VecDeque<String>>, line: &str) {
    let line = truncate_to_chars(line, HEADROOM_STDERR_LINE_MAX_CHARS);
    let mut stderr_lines = match stderr_lines.lock() {
        Ok(stderr_lines) => stderr_lines,
        Err(poisoned) => poisoned.into_inner(),
    };
    push_stderr_line(&mut stderr_lines, line);
}

fn record_stderr_line_for_generation(
    stderr_lines: &Mutex<VecDeque<String>>,
    stderr_generation: &AtomicU64,
    generation: u64,
    line: &str,
) -> bool {
    let line = truncate_to_chars(line, HEADROOM_STDERR_LINE_MAX_CHARS);
    let mut stderr_lines = match stderr_lines.lock() {
        Ok(stderr_lines) => stderr_lines,
        Err(poisoned) => poisoned.into_inner(),
    };
    if stderr_generation.load(Ordering::SeqCst) != generation {
        return false;
    }
    push_stderr_line(&mut stderr_lines, line);
    true
}

fn push_stderr_line(stderr_lines: &mut VecDeque<String>, line: String) {
    while stderr_lines.len() >= HEADROOM_STDERR_RING_LINES {
        stderr_lines.pop_front();
    }
    stderr_lines.push_back(line);
}

fn stderr_tail_snapshot(stderr_lines: &Mutex<VecDeque<String>>) -> Vec<String> {
    let stderr_lines = match stderr_lines.lock() {
        Ok(stderr_lines) => stderr_lines,
        Err(poisoned) => poisoned.into_inner(),
    };
    stderr_lines
        .iter()
        .skip(
            stderr_lines
                .len()
                .saturating_sub(HEADROOM_STDERR_MESSAGE_LINES),
        )
        .cloned()
        .collect()
}

fn message_with_stderr_tail(message: String, stderr_lines: &[String]) -> String {
    if stderr_lines.is_empty() {
        return truncate_child_output(&message);
    }

    const SEPARATOR: &str = " | ";
    let stderr_lines = &stderr_lines[stderr_lines
        .len()
        .saturating_sub(HEADROOM_STDERR_MESSAGE_LINES)..];
    let separator_chars = SEPARATOR.chars().count() * stderr_lines.len().saturating_sub(1);
    let tail_reserve = CHILD_OUTPUT_MIN_TAIL_LINE_CHARS * stderr_lines.len();
    let message_chars = CHILD_OUTPUT_MAX_CHARS
        .saturating_sub(LAST_OUTPUT_MARKER.chars().count())
        .saturating_sub(separator_chars)
        .saturating_sub(tail_reserve);
    let message = truncate_to_chars(&message, message_chars);
    let available_chars = CHILD_OUTPUT_MAX_CHARS
        .saturating_sub(message.chars().count())
        .saturating_sub(LAST_OUTPUT_MARKER.chars().count())
        .saturating_sub(separator_chars);
    let line_chars = available_chars / stderr_lines.len();
    let tail = stderr_lines
        .iter()
        .map(|line| truncate_to_chars(line, line_chars))
        .collect::<Vec<_>>()
        .join(SEPARATOR);
    truncate_child_output(&format!("{message}{LAST_OUTPUT_MARKER}{tail}"))
}

fn truncate_child_output(output: &str) -> String {
    truncate_to_chars(output.trim(), CHILD_OUTPUT_MAX_CHARS)
}

fn truncate_to_chars(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_string();
    }
    if max_chars <= 3 {
        return ".".repeat(max_chars);
    }
    format!(
        "{}...",
        value.chars().take(max_chars - 3).collect::<String>()
    )
}

impl HeadroomStats {
    fn project_rollups(&self) -> BTreeMap<&str, &SavingsRollup> {
        let mut rollups = BTreeMap::new();
        if let Some(persistent) = self.persistent_savings.as_ref() {
            for (project, rollup) in &persistent.projects {
                rollups.insert(project.as_str(), rollup);
            }
        }
        if let Some(savings) = self.savings.as_ref() {
            for (project, rollup) in &savings.per_project {
                rollups.insert(project.as_str(), rollup);
            }
        }
        rollups
    }

    fn usage_by_lane(
        &self,
        lane_by_project: &HashMap<String, SessionLaneKind>,
    ) -> (Vec<LaneUsage>, LaneUsage) {
        let mut usage_by_lane = BTreeMap::new();
        let mut unattributed_usage = LaneUsage::unattributed();

        for (project, rollup) in self.project_rollups() {
            let Some(lane) = lane_by_project.get(project) else {
                unattributed_usage.add_rollup(rollup);
                continue;
            };
            usage_by_lane
                .entry(lane.storage_name())
                .or_insert_with(|| LaneUsage::for_lane(*lane))
                .add_rollup(rollup);
        }

        (usage_by_lane.into_values().collect(), unattributed_usage)
    }

    fn project_rollup(&self, project: &str) -> Option<&SavingsRollup> {
        // Current Headroom serves the projects map at savings.per_project;
        // older builds put it under a top-level persistent_savings key.
        self.savings
            .as_ref()
            .and_then(|stats| stats.per_project.get(project))
            .or_else(|| {
                self.persistent_savings
                    .as_ref()
                    .and_then(|stats| stats.projects.get(project))
            })
    }

    #[cfg(test)]
    fn project_tokens_saved(&self, project: &str) -> u64 {
        self.project_rollup(project)
            .and_then(|stats| stats.tokens_saved)
            .unwrap_or_default()
    }

    fn project_savings_totals(&self, project: &str) -> ProjectSavingsTotals {
        let rollup = self.project_rollup(project);
        ProjectSavingsTotals {
            tokens_saved: rollup
                .and_then(|stats| stats.tokens_saved)
                .unwrap_or_default(),
            savings_usd: rollup.and_then(|stats| stats.compression_savings_usd),
        }
    }

    fn project_savings_snapshot(&self, project: &str) -> (u64, u64, u64) {
        let rollup = self.project_rollup(project);
        (
            rollup.and_then(|stats| stats.requests).unwrap_or_default(),
            rollup
                .and_then(|stats| stats.tokens_saved)
                .unwrap_or_default(),
            rollup
                .and_then(|stats| stats.cache_read_tokens)
                .unwrap_or_default(),
        )
    }

    fn reported_version(&self) -> Option<String> {
        self.version.clone().or_else(|| {
            self.extra
                .get("version")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
    }

    fn looks_like_headroom(&self) -> bool {
        self.summary.is_some()
            || self.requests.is_some()
            || self.tokens.is_some()
            || self.persistent_savings.is_some()
            || self.cost.is_some()
            || self.savings.is_some()
    }

    fn halv_agent_savings(&self) -> Option<(u64, u64)> {
        let mut found = false;
        let mut tokens_saved = 0u64;
        let mut requests = 0u64;
        #[cfg(debug_assertions)]
        let mut seen_agent_names = Vec::new();

        for agent in &self.agent_usage.as_ref()?.agents {
            let Some(agent_name) = agent.agent.as_deref() else {
                continue;
            };
            #[cfg(debug_assertions)]
            seen_agent_names.push(agent_name);
            if !is_halv_headroom_agent(agent_name) {
                continue;
            }

            found = true;
            tokens_saved = tokens_saved.saturating_add(agent.tokens_saved.unwrap_or_default());
            requests = requests.saturating_add(agent.requests.unwrap_or_default());
        }

        #[cfg(debug_assertions)]
        if !found {
            static UNMATCHED_AGENT_NAMES_LOGGED: OnceLock<()> = OnceLock::new();
            UNMATCHED_AGENT_NAMES_LOGGED.get_or_init(|| {
                eprintln!(
                    "Headroom agent usage did not match Halv. Seen agent names: {seen_agent_names:?}"
                );
            });
        }

        found.then_some((tokens_saved, requests))
    }

    fn plan_windows(&self) -> Option<Vec<PlanWindowStats>> {
        let limits = self.codex_rate_limits.as_ref()?;
        let weekly = [&limits.primary, &limits.secondary]
            .into_iter()
            .filter_map(Option::as_ref)
            .find(|window| is_weekly_plan_window(window))
            // Older responses omitted labels and durations but placed the
            // weekly window in `secondary`; preserve that compatibility.
            .or_else(|| {
                limits
                    .secondary
                    .as_ref()
                    .filter(|window| plan_window_shape_is_unknown(window))
            })
            .or_else(|| {
                limits
                    .primary
                    .as_ref()
                    .filter(|window| plan_window_shape_is_unknown(window))
            })?;
        let mut weekly = plan_window_stats(weekly, "168h")?;
        weekly.label = "7d".to_string();
        Some(vec![weekly])
    }

    fn to_sidecar_stats(
        &self,
        port: u16,
        instance_generation: u64,
        lane_by_project: &HashMap<String, SessionLaneKind>,
    ) -> SidecarStats {
        let lifetime = self
            .persistent_savings
            .as_ref()
            .and_then(|stats| stats.lifetime.as_ref());
        let session = self.display_session.as_ref().or_else(|| {
            self.persistent_savings
                .as_ref()
                .and_then(|stats| stats.display_session.as_ref())
        });
        let requests = self.requests.as_ref();
        let tokens = self.tokens.as_ref();
        let cost = self.cost.as_ref();
        let summary = self.summary.as_ref();
        let summary_compression = summary.and_then(|summary| summary.compression.as_ref());
        let summary_cost = summary.and_then(|summary| summary.cost.as_ref());
        let has_agent_usage = self.agent_usage.is_some();
        let halv_agent_savings = self.halv_agent_savings();
        let rtk_tokens_saved = tokens
            .and_then(|stats| stats.rtk_saved)
            .or_else(|| summary_compression.and_then(|stats| stats.rtk_tokens_avoided))
            .unwrap_or_default();
        let lean_ctx_tokens_saved = tokens
            .and_then(|stats| stats.lean_ctx_saved)
            .unwrap_or_default();
        let cli_filtering_tokens_saved = tokens
            .and_then(|stats| stats.cli_filtering_saved)
            .or_else(|| summary_compression.and_then(|stats| stats.cli_filtering_tokens_avoided))
            .unwrap_or_else(|| rtk_tokens_saved.saturating_add(lean_ctx_tokens_saved));
        let tool_search_tokens_saved = self
            .savings
            .as_ref()
            .and_then(|stats| stats.by_layer.get("tool_search"))
            .and_then(|stats| {
                stats
                    .get("tokens_saved")
                    .or_else(|| stats.get("tokens"))
                    .and_then(Value::as_u64)
            })
            .unwrap_or_default();
        let estimated_output_tokens_saved = tokens
            .and_then(|stats| stats.output_saved)
            .unwrap_or_default();
        let mcp_tokens_saved = summary
            .and_then(|stats| stats.mcp.as_ref())
            .and_then(|stats| stats.tokens_removed)
            .unwrap_or_default();
        let live_ws_tokens_saved = summary
            .and_then(|stats| stats.extra.get("codex_ws"))
            .and_then(|stats| stats.get("tokens_saved"))
            .and_then(Value::as_u64)
            .unwrap_or_default();
        let proxy_compression_tokens_saved = tokens
            .and_then(|stats| stats.proxy_compression_saved)
            .or_else(|| summary_compression.and_then(|stats| stats.total_tokens_removed))
            .unwrap_or_default();
        let global_compression_tokens_saved = lifetime
            .and_then(|stats| stats.tokens_saved)
            .or(Some(proxy_compression_tokens_saved).filter(|tokens| *tokens > 0))
            .or_else(|| cost.and_then(|stats| stats.total_tokens_saved))
            .or_else(|| summary_compression.and_then(|stats| stats.total_tokens_removed))
            .or_else(|| {
                tokens
                    .and_then(|stats| stats.saved.or(stats.all_layers_saved))
                    .map(|total| total.saturating_sub(cli_filtering_tokens_saved))
            })
            .or_else(|| {
                self.savings
                    .as_ref()
                    .and_then(|stats| stats.total_tokens)
                    .map(|total| total.saturating_sub(cli_filtering_tokens_saved))
            })
            .unwrap_or_default();
        let global_cost_saved_usd = lifetime
            .and_then(|stats| stats.compression_savings_usd)
            .or_else(|| cost.and_then(|stats| stats.compression_savings_usd))
            .or_else(|| cost.and_then(|stats| stats.savings_usd))
            .or_else(|| summary_cost.and_then(|stats| stats.total_saved_usd))
            .unwrap_or_default();
        let global_requests_total = lifetime
            .and_then(|stats| stats.requests)
            .or_else(|| requests.and_then(|stats| stats.total))
            .or_else(|| summary.and_then(|stats| stats.api_requests))
            .unwrap_or_default();
        let compression_tokens_saved = match halv_agent_savings {
            Some((tokens_saved, _)) => tokens_saved,
            None => global_compression_tokens_saved,
        };
        let all_input_layers_tokens_saved = tokens
            .and_then(|stats| stats.all_layers_saved.or(stats.saved))
            .or_else(|| self.savings.as_ref().and_then(|stats| stats.total_tokens))
            .or_else(|| {
                summary_compression.and_then(|stats| stats.total_tokens_saved_with_cli_filtering)
            })
            .or_else(|| summary_compression.and_then(|stats| stats.total_tokens_saved_with_rtk))
            .unwrap_or_else(|| compression_tokens_saved.saturating_add(cli_filtering_tokens_saved));
        let other_input_layers_tokens_saved =
            all_input_layers_tokens_saved.saturating_sub(proxy_compression_tokens_saved);
        let tokens_saved = compression_tokens_saved
            .saturating_add(other_input_layers_tokens_saved)
            .saturating_add(tool_search_tokens_saved);
        // Headroom reports compression USD globally, so per-agent USD is an attribution approximation.
        let cost_saved_usd = match halv_agent_savings {
            Some((0, _)) => 0.0,
            Some((agent_tokens, _)) if global_compression_tokens_saved > 0 => {
                global_cost_saved_usd
                    * (agent_tokens as f64 / global_compression_tokens_saved as f64)
            }
            Some(_) => 0.0,
            None => global_cost_saved_usd,
        };
        let process_input_tokens = tokens
            .and_then(|stats| stats.input)
            .or_else(|| cost.and_then(|stats| stats.total_input_tokens))
            .unwrap_or_default();
        let exact_tokens_before = process_input_tokens.saturating_add(tokens_saved);
        let savings_percent = if exact_tokens_before > 0 {
            Some(tokens_saved as f64 / exact_tokens_before as f64 * 100.0)
        } else {
            lifetime
                .and_then(|stats| stats.savings_percent)
                .or_else(|| session.and_then(|stats| stats.savings_percent))
                .or_else(|| tokens.and_then(|stats| stats.all_layers_savings_percent))
                .or_else(|| tokens.and_then(|stats| stats.savings_percent))
                .or_else(|| summary_cost.and_then(|stats| stats.savings_pct))
        };

        let (usage_by_lane, unattributed_usage) = self.usage_by_lane(lane_by_project);

        SidecarStats {
            port,
            instance_generation,
            lifetime_tokens_saved: lifetime.and_then(|stats| stats.tokens_saved),
            lifetime_cost_saved_usd: lifetime.and_then(|stats| stats.compression_savings_usd),
            tokens_saved,
            cost_saved_usd,
            requests_total: match halv_agent_savings {
                Some((_, requests)) => requests,
                None if has_agent_usage => 0,
                None => global_requests_total,
            },
            lifetime_requests: lifetime
                .and_then(|stats| stats.requests)
                .unwrap_or_default(),
            lifetime_cache_savings_usd: lifetime
                .and_then(|stats| stats.cache_savings_usd)
                .unwrap_or_default(),
            requests_by_provider: requests
                .map(|stats| stats.by_provider.clone())
                .unwrap_or_default(),
            usage_by_lane,
            unattributed_usage,
            requests_cached: requests.and_then(|stats| stats.cached).unwrap_or_default(),
            requests_failed: requests.and_then(|stats| stats.failed).unwrap_or_default(),
            // display_session has no per-agent breakdown in Headroom, so session_* remain proxy-wide.
            session_tokens_saved: session
                .and_then(|stats| stats.tokens_saved)
                .unwrap_or_default(),
            session_cost_saved_usd: session
                .and_then(|stats| stats.compression_savings_usd)
                .unwrap_or_default(),
            input_tokens: lifetime
                .and_then(|stats| stats.total_input_tokens)
                .or_else(|| tokens.and_then(|stats| stats.input))
                .or_else(|| cost.and_then(|stats| stats.total_input_tokens))
                .unwrap_or_default(),
            process_input_tokens,
            output_tokens: tokens.and_then(|stats| stats.output).unwrap_or_default(),
            proxy_compression_tokens_saved,
            compression_tokens_saved,
            cli_filtering_tokens_saved,
            rtk_tokens_saved,
            lean_ctx_tokens_saved,
            tool_search_tokens_saved,
            estimated_output_tokens_saved,
            mcp_tokens_saved,
            live_ws_tokens_saved,
            cache_savings_usd: cost
                .and_then(|stats| stats.cache_savings_usd)
                .or_else(|| {
                    summary_cost.and_then(|stats| {
                        stats
                            .breakdown
                            .as_ref()
                            .and_then(|stats| stats.cache_savings_usd)
                    })
                })
                .unwrap_or_default(),
            cost_with_headroom_usd: cost
                .and_then(|stats| stats.cost_with_headroom_usd)
                .or_else(|| summary_cost.and_then(|stats| stats.with_headroom_usd))
                .unwrap_or_default(),
            savings_percent,
            plan_windows: self.plan_windows(),
        }
    }
}

fn plan_window_stats(
    window: &CodexRateLimitWindow,
    fallback_label: &str,
) -> Option<PlanWindowStats> {
    let used_percent = window.used_percent?;
    let label = normalize_plan_window_label(
        window
            .window_label
            .as_deref()
            .filter(|label| !label.trim().is_empty())
            .unwrap_or(fallback_label),
    );
    Some(PlanWindowStats {
        label,
        used_percent,
        resets_at: window.resets_at,
    })
}

fn is_weekly_plan_window(window: &CodexRateLimitWindow) -> bool {
    const WEEK_MINUTES: u64 = 7 * 24 * 60;
    window.window_minutes == Some(WEEK_MINUTES)
        || window.window_label.as_deref().is_some_and(|label| {
            matches!(label.trim().to_ascii_lowercase().as_str(), "168h" | "7d")
        })
}

fn plan_window_shape_is_unknown(window: &CodexRateLimitWindow) -> bool {
    window.window_minutes.is_none()
        && window
            .window_label
            .as_deref()
            .map_or(true, |label| label.trim().is_empty())
}

fn normalize_plan_window_label(label: &str) -> String {
    if label == "168h" {
        "7d".to_string()
    } else {
        label.to_string()
    }
}

#[tauri::command]
pub async fn sidecar_status(
    manager: tauri::State<'_, SidecarManager>,
) -> Result<SidecarStatus, String> {
    Ok(manager.refresh_status().await)
}

fn lane_map_from_rows(rows: Vec<(String, String)>) -> HashMap<String, SessionLaneKind> {
    rows.into_iter()
        .filter_map(
            |(id, lane)| match SessionLaneKind::from_storage_name(&lane) {
                Ok(lane_kind) => Some((id, lane_kind)),
                Err(error) => {
                    eprintln!(
                        "sidecar stats: skipping agent session `{id}` with unknown lane `{lane}`: {error}"
                    );
                    None
                }
            },
        )
        .collect()
}

#[tauri::command]
pub async fn sidecar_stats(
    manager: tauri::State<'_, SidecarManager>,
    storage: tauri::State<'_, Storage>,
) -> Result<SidecarStats, String> {
    let lane_by_project = lane_map_from_rows(
        storage
            .list_agent_session_lanes()
            .map_err(|error| error.to_string())?,
    );
    manager.live_stats(&lane_by_project).await
}

#[tauri::command]
pub async fn sidecar_restart(
    manager: tauri::State<'_, SidecarManager>,
) -> Result<SidecarStatus, String> {
    Ok(manager.restart().await)
}

#[tauri::command]
pub async fn sidecar_install_headroom<R: Runtime>(
    app: AppHandle<R>,
    manager: tauri::State<'_, SidecarManager>,
) -> Result<SidecarStatus, SidecarInstallError> {
    let resource_dir = app.path().resource_dir().map_err(|error| {
        SidecarInstallError::single(
            SidecarInstallErrorCode::AppDataUnavailable,
            "Could not resolve Halv's bundled resources directory.",
            error.to_string(),
        )
    })?;
    let progress = TauriInstallProgress { app };
    manager.install_headroom(resource_dir, &progress).await
}

#[tauri::command]
pub async fn sidecar_update_headroom<R: Runtime>(
    app: AppHandle<R>,
    manager: tauri::State<'_, SidecarManager>,
) -> Result<SidecarStatus, SidecarInstallError> {
    let resource_dir = app.path().resource_dir().map_err(|error| {
        SidecarInstallError::single(
            SidecarInstallErrorCode::AppDataUnavailable,
            "Could not resolve Halv's bundled resources directory.",
            error.to_string(),
        )
    })?;
    let progress = TauriInstallProgress { app };
    progress.emit(
        SidecarInstallStage::Starting,
        format!(
            "Using bundled proxy {} from this Halv release.",
            pinned_headroom_version()
        ),
    );
    manager.install_headroom(resource_dir, &progress).await
}

fn stats_url(port: u16) -> String {
    format!("http://127.0.0.1:{port}/stats")
}

fn unmanaged_endpoint_message(port: u16) -> String {
    format!(
        "managed proxy is not running; refusing to use an unmanaged listener on 127.0.0.1:{port}"
    )
}

/// The wait before a restart: none for the first one in the window, then
/// 1s, 2s, 4s ... up to the cap.
fn auto_heal_backoff(previous_attempts: usize) -> Duration {
    if previous_attempts == 0 {
        return Duration::ZERO;
    }
    let exponent = u32::try_from(previous_attempts - 1)
        .unwrap_or(u32::MAX)
        .min(16);
    AUTO_HEAL_BACKOFF_BASE
        .saturating_mul(1u32 << exponent)
        .min(AUTO_HEAL_BACKOFF_MAX)
}

/// One supervisor log line. The stderr tail keeps the last lines only.
fn supervisor_log_line(
    at: chrono::DateTime<chrono::Utc>,
    port: u16,
    exit: &ManagedChildExit,
    restarts_in_window: usize,
) -> String {
    let start = exit
        .stderr_lines
        .len()
        .saturating_sub(SUPERVISOR_LOG_STDERR_LINES);
    let stderr = exit.stderr_lines[start..]
        .iter()
        .map(|line| line.trim())
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join(" | ");
    format!(
        "{} port={port} exit=\"{}\" restarts_in_window={restarts_in_window} stderr=\"{stderr}\"",
        at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        exit.status
    )
}

/// Appends one line to the supervisor log. A full log moves aside once.
fn append_supervisor_log(app_data_dir: &Path, line: &str, max_bytes: u64) -> std::io::Result<()> {
    let logs_dir = app_data_dir.join("logs");
    fs::create_dir_all(&logs_dir)?;
    let path = logs_dir.join(SUPERVISOR_LOG_FILE_NAME);
    if fs::metadata(&path).is_ok_and(|metadata| metadata.len() >= max_bytes) {
        let _ = fs::rename(
            &path,
            logs_dir.join(format!("{SUPERVISOR_LOG_FILE_NAME}.old")),
        );
    }
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    writeln!(file, "{line}")
}

#[derive(Debug, PartialEq, Eq)]
struct PortSelection {
    port: u16,
    reused_preferred: bool,
}

fn choose_available_loopback_port(preferred_port: Option<u16>) -> Result<PortSelection, String> {
    if let Some(preferred_port) = preferred_port.filter(|port| *port != 0) {
        if let Ok(listener) = std::net::TcpListener::bind(("127.0.0.1", preferred_port)) {
            let port = listener
                .local_addr()
                .map(|address| address.port())
                .map_err(|error| format!("failed to inspect loopback listener: {error}"))?;
            return Ok(PortSelection {
                port,
                reused_preferred: true,
            });
        }
    }

    let listener = std::net::TcpListener::bind(("127.0.0.1", 0))
        .map_err(|error| format!("failed to reserve an unused loopback port: {error}"))?;
    let port = listener
        .local_addr()
        .map(|address| address.port())
        .map_err(|error| format!("failed to inspect reserved loopback port: {error}"))?;
    Ok(PortSelection {
        port,
        reused_preferred: false,
    })
}

fn select_available_loopback_port(preferred_port: u16) -> Result<u16, String> {
    choose_available_loopback_port((preferred_port != 0).then_some(preferred_port))
        .map(|selection| selection.port)
}

struct HeadroomProxyLog {
    path: PathBuf,
    old_path: PathBuf,
    file: Option<BufWriter<fs::File>>,
    bytes_written: u64,
    max_bytes: u64,
}

impl HeadroomProxyLog {
    fn open(app_data_dir: &Path, max_bytes: u64) -> std::io::Result<Self> {
        let logs_dir = app_data_dir.join("logs");
        fs::create_dir_all(&logs_dir)?;
        let path = logs_dir.join("headroom-proxy.log");
        let old_path = logs_dir.join("headroom-proxy.log.old");
        let bytes_written = match fs::metadata(&path) {
            Ok(metadata) => metadata.len(),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => 0,
            Err(error) => return Err(error),
        };
        if bytes_written > max_bytes {
            rotate_headroom_proxy_log_files(&path, &old_path)?;
        }
        let bytes_written = match fs::metadata(&path) {
            Ok(metadata) => metadata.len(),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => 0,
            Err(error) => return Err(error),
        };
        let file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)?;
        Ok(Self {
            path,
            old_path,
            file: Some(BufWriter::new(file)),
            bytes_written,
            max_bytes,
        })
    }

    fn write_line(&mut self, line: &str) -> std::io::Result<()> {
        let file = self
            .file
            .as_mut()
            .ok_or_else(|| std::io::Error::other("headroom proxy log file is not available"))?;
        file.write_all(line.as_bytes())?;
        file.write_all(b"\n")?;
        self.bytes_written = self
            .bytes_written
            .saturating_add(line.len() as u64)
            .saturating_add(1);
        if self.bytes_written > self.max_bytes {
            self.rotate()?;
        } else {
            // A crash trace must reach disk before the child dies; the
            // sidecar writes little, so a per-line flush costs nothing.
            self.file
                .as_mut()
                .ok_or_else(|| std::io::Error::other("headroom proxy log file is not available"))?
                .flush()?;
        }
        Ok(())
    }

    fn rotate(&mut self) -> std::io::Result<()> {
        if let Some(mut file) = self.file.take() {
            file.flush()?;
        }
        rotate_headroom_proxy_log_files(&self.path, &self.old_path)?;
        self.file = Some(BufWriter::new(
            fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&self.path)?,
        ));
        self.bytes_written = 0;
        Ok(())
    }
}

fn rotate_headroom_proxy_log_files(path: &Path, old_path: &Path) -> std::io::Result<()> {
    match fs::remove_file(old_path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    match fs::rename(path, old_path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn log_headroom_proxy_file_error(error: &std::io::Error) {
    eprintln!("Headroom proxy stderr file logging disabled: {error}");
}

#[allow(clippy::too_many_arguments)]
fn drain_child_stderr<R: Read>(
    stderr: R,
    stderr_lines: Arc<Mutex<VecDeque<String>>>,
    stderr_generation: Arc<AtomicU64>,
    generation: u64,
    app_data_dir: Arc<RwLock<Option<PathBuf>>>,
    mut log: Option<HeadroomProxyLog>,
    mut file_logging_disabled: bool,
    log_max_bytes: u64,
) {
    let mut stderr = BufReader::new(stderr);
    let mut buffer = Vec::new();
    loop {
        buffer.clear();
        match stderr.read_until(b'\n', &mut buffer) {
            Ok(0) => break,
            Ok(_) => {
                if stderr_generation.load(Ordering::SeqCst) != generation {
                    continue;
                }
                let line = String::from_utf8_lossy(&buffer);
                let line = line.trim_end_matches(['\r', '\n']);
                if !record_stderr_line_for_generation(
                    &stderr_lines,
                    &stderr_generation,
                    generation,
                    line,
                ) {
                    continue;
                }

                if log.is_none() && !file_logging_disabled {
                    let app_data_dir = match app_data_dir.read() {
                        Ok(app_data_dir) => app_data_dir.clone(),
                        Err(poisoned) => poisoned.into_inner().clone(),
                    };
                    if let Some(app_data_dir) = app_data_dir {
                        match HeadroomProxyLog::open(&app_data_dir, log_max_bytes) {
                            Ok(opened_log) => log = Some(opened_log),
                            Err(error) => {
                                log_headroom_proxy_file_error(&error);
                                file_logging_disabled = true;
                            }
                        }
                    }
                }
                if stderr_generation.load(Ordering::SeqCst) != generation {
                    continue;
                }
                if let Some(active_log) = log.as_mut() {
                    if let Err(error) = active_log.write_line(line) {
                        log_headroom_proxy_file_error(&error);
                        log = None;
                        file_logging_disabled = true;
                    }
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn spawn_headroom_proxy_with_log_limit(
    binary_path: &Path,
    port: u16,
    stderr_lines: Arc<Mutex<VecDeque<String>>>,
    stderr_generation: Arc<AtomicU64>,
    app_data_dir: Arc<RwLock<Option<PathBuf>>>,
    log_max_bytes: u64,
    optimization_paused: bool,
    upstream: Option<&SidecarUpstream>,
) -> Result<ManagedChild, String> {
    let generation = stderr_generation
        .fetch_add(1, Ordering::SeqCst)
        .wrapping_add(1);
    match stderr_lines.lock() {
        Ok(mut stderr_lines) => stderr_lines.clear(),
        Err(poisoned) => poisoned.into_inner().clear(),
    }

    let initial_app_data_dir = match app_data_dir.read() {
        Ok(app_data_dir) => app_data_dir.clone(),
        Err(poisoned) => poisoned.into_inner().clone(),
    };
    let (log, file_logging_disabled) = match initial_app_data_dir.as_ref() {
        Some(app_data_dir) => match HeadroomProxyLog::open(app_data_dir, log_max_bytes) {
            Ok(log) => (Some(log), false),
            Err(error) => {
                log_headroom_proxy_file_error(&error);
                (None, true)
            }
        },
        None => (None, false),
    };

    let mut process = headroom_proxy_command(
        binary_path,
        port,
        initial_app_data_dir.as_deref(),
        optimization_paused,
        upstream,
    )
    .spawn()
    .map_err(|error| error.to_string())?;
    let stderr = match process.stderr.take() {
        Some(stderr) => stderr,
        None => {
            let _ = process.kill();
            let _ = process.wait();
            return Err("managed proxy stderr pipe was unavailable".to_string());
        }
    };
    let (stderr_done_tx, stderr_done) = mpsc::channel();
    let stderr_drain = std::thread::Builder::new()
        .name("headroom-proxy-stderr".to_string())
        .spawn(move || {
            drain_child_stderr(
                stderr,
                stderr_lines,
                stderr_generation,
                generation,
                app_data_dir,
                log,
                file_logging_disabled,
                log_max_bytes,
            );
            let _ = stderr_done_tx.send(());
        })
        .map_err(|error| {
            let _ = process.kill();
            let _ = process.wait();
            format!("failed to start managed proxy stderr drain: {error}")
        })?;

    Ok(ManagedChild {
        process,
        stderr_drain: Some(stderr_drain),
        stderr_done: Some(stderr_done),
    })
}

/// The variables that turn off the telemetry and the update check of the
/// bundled Headroom proxy.
const HEADROOM_PRIVACY_ENV: [(&str, &str); 3] = [
    ("HEADROOM_BEACON", "off"),
    ("DO_NOT_TRACK", "1"),
    ("HEADROOM_UPDATE_CHECK", "off"),
];

fn headroom_proxy_command(
    binary_path: &Path,
    port: u16,
    app_data_dir: Option<&Path>,
    optimization_paused: bool,
    upstream: Option<&SidecarUpstream>,
) -> Command {
    let mut command = Command::new(binary_path);
    crate::process::hide_console(&mut command);
    command.arg("proxy");
    if optimization_paused {
        command.arg("--no-optimize");
    }
    command
        .args(["--host", "127.0.0.1"])
        .arg("--port")
        .arg(port.to_string())
        .args(["--workers", "1"])
        .arg("--no-http2")
        .env(HALV_MANAGED_SIDECAR_ENV, HALV_MANAGED_SIDECAR_VALUE)
        .env("HEADROOM_DESKTOP_SUPERVISED", "1")
        .env("HEADROOM_COMPRESS_USER_MESSAGES", "1")
        .env("HEADROOM_COMPRESS_RESPONSES_MESSAGES", "1")
        .env("HEADROOM_PROTECT_RECENT", "2")
        .env("HEADROOM_MODE", "token")
        .env(
            crate::exec::RTK_DB_PATH_ENV,
            crate::exec::rtk_database_path(),
        )
        .env("RTK_TELEMETRY_DISABLED", "1")
        // Halv sends no usage data without consent (ADR 0021), so the proxy
        // never uploads its own telemetry or checks for updates.
        .envs(HEADROOM_PRIVACY_ENV.iter().copied())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    if let Some(upstream) = upstream {
        command.arg("--stateless");
        match upstream {
            SidecarUpstream::OpenAi(url) => {
                command.env("OPENAI_TARGET_API_URL", url);
            }
        }
    }
    if let Some(rtk_path) = crate::exec::rtk_path_environment() {
        command.env("PATH", rtk_path);
    }
    if let Some(app_data_dir) = app_data_dir {
        let headroom_data_dir = app_data_dir.join("headroom");
        command
            .env(
                "HEADROOM_WORKSPACE_DIR",
                headroom_data_dir.join("workspace"),
            )
            .env("HF_HOME", headroom_data_dir.join("huggingface"));
    }
    command
}

async fn locate_headroom_binary_async(
    app_data_dir: Option<PathBuf>,
    resource_dir: Option<PathBuf>,
) -> Option<PathBuf> {
    tauri::async_runtime::spawn_blocking(move || {
        locate_headroom_binary(app_data_dir.as_deref(), resource_dir.as_deref())
    })
    .await
    .ok()
    .flatten()
}

fn locate_headroom_binary(
    app_data_dir: Option<&Path>,
    resource_dir: Option<&Path>,
) -> Option<PathBuf> {
    locate_headroom_binary_with_env(
        app_data_dir,
        resource_dir,
        std::env::var_os("PATH"),
        std::env::var_os("HOME"),
        std::env::var_os(HALV_HEADROOM_BINARY_ENV),
    )
}

fn locate_headroom_binary_with_env(
    app_data_dir: Option<&Path>,
    resource_dir: Option<&Path>,
    path_env: Option<OsString>,
    home_env: Option<OsString>,
    explicit_override: Option<OsString>,
) -> Option<PathBuf> {
    explicit_override
        .and_then(|path| executable_file(PathBuf::from(path)))
        .or_else(|| resource_dir.and_then(bundled_headroom_path))
        .or_else(|| app_data_dir.and_then(app_data_headroom_path))
        .or_else(|| find_on_path(binary_name(), path_env.as_deref()))
        .or_else(|| home_env.and_then(home_local_headroom_path))
}

fn bundled_headroom_path(resource_dir: &Path) -> Option<PathBuf> {
    executable_file(
        resource_dir
            .join("headroom-runtime")
            .join(bundled_binary_name()),
    )
}

fn app_data_headroom_path(app_data_dir: &Path) -> Option<PathBuf> {
    executable_file(app_data_dir.join("bin").join(binary_name()))
}

fn is_halv_managed_binary_path(
    binary_path: Option<&Path>,
    app_data_dir: Option<&Path>,
    resource_dir: Option<&Path>,
) -> bool {
    let Some(binary_path) = binary_path else {
        return false;
    };
    app_data_dir
        .is_some_and(|app_data_dir| binary_path == app_data_dir.join("bin").join(binary_name()))
        || resource_dir.is_some_and(|resource_dir| {
            binary_path
                == resource_dir
                    .join("headroom-runtime")
                    .join(bundled_binary_name())
        })
}

fn home_local_headroom_path(home: OsString) -> Option<PathBuf> {
    executable_file(PathBuf::from(home).join(".local/bin").join(binary_name()))
}

fn executable_file(path: PathBuf) -> Option<PathBuf> {
    path.is_file().then_some(path)
}

fn find_on_path(name: &str, path_env: Option<&OsStr>) -> Option<PathBuf> {
    let path_env = path_env?;
    std::env::split_paths(path_env)
        .map(|dir| dir.join(name))
        .find_map(executable_file)
}

fn binary_name() -> &'static str {
    if cfg!(windows) {
        "headroom.exe"
    } else {
        "headroom"
    }
}

fn bundled_binary_name() -> &'static str {
    if cfg!(windows) {
        "headroom-runtime.exe"
    } else {
        "headroom-runtime"
    }
}

fn pinned_headroom_version() -> &'static str {
    &bundled_headroom_lock().runtime_version
}

fn read_headroom_version(binary_path: &Path) -> Option<String> {
    read_headroom_version_with_timeout(binary_path, VERSION_COMMAND_TIMEOUT)
}

fn read_headroom_version_with_timeout(binary_path: &Path, timeout: Duration) -> Option<String> {
    let mut command = Command::new(binary_path);
    command.arg("--version");
    crate::process::hide_console(&mut command);
    let output =
        crate::exec::run_process_with_limits(command, timeout, SIDECAR_COMMAND_OUTPUT_LIMIT_BYTES)
            .ok()?;
    if !output.status.success() || output.truncated {
        return None;
    }

    let raw = String::from_utf8_lossy(&output.output);

    raw.split_whitespace()
        .last()
        .map(|version| version.trim_matches(',').to_string())
        .filter(|version| !version.is_empty())
}

async fn read_headroom_version_async(binary_path: PathBuf) -> Option<String> {
    tauri::async_runtime::spawn_blocking(move || read_headroom_version(&binary_path))
        .await
        .ok()
        .flatten()
}

fn install_hint() -> String {
    "This Halv build is missing its bundled proxy. Reinstall Halv with an official platform installer."
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::Cursor,
        net::TcpListener,
        sync::{
            atomic::{AtomicUsize, Ordering as AtomicOrdering},
            Mutex as StdMutex, OnceLock,
        },
        thread::{self, JoinHandle},
    };

    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    const CAPTURED_STATS: &str = include_str!("../../tests/fixtures/headroom_stats.json");

    fn running_status(spawned_by_us: bool, managed_by_halv: bool, port: u16) -> SidecarStatus {
        SidecarStatus::Running {
            spawned_by_us,
            managed_by_halv,
            optimization_paused: false,
            version: Some("v-test".to_string()),
            pinned_version: "v-pin".to_string(),
            port,
        }
    }

    fn reap_candidate(
        pid: u32,
        ppid: u32,
        parent_alive: bool,
        marker_present: bool,
    ) -> ReapCandidate {
        ReapCandidate {
            pid,
            ppid: Some(ppid),
            parent_alive: Some(parent_alive),
            marker_present,
            marker_check_unavailable: false,
            exact_command_shape: true,
        }
    }

    #[test]
    fn lane_map_from_rows_skips_unknown_lanes() {
        let lanes = lane_map_from_rows(vec![
            (
                "claude-session".to_string(),
                "claude_subscription".to_string(),
            ),
            ("unknown-session".to_string(), "bogus".to_string()),
            ("codex-session".to_string(), "codex_chatgpt".to_string()),
        ]);

        assert_eq!(lanes.len(), 2);
        assert_eq!(
            lanes.get("claude-session"),
            Some(&SessionLaneKind::ClaudeSubscription)
        );
        assert_eq!(
            lanes.get("codex-session"),
            Some(&SessionLaneKind::CodexChatGpt)
        );
    }

    #[test]
    fn managed_proxy_command_turns_off_headroom_telemetry() {
        let command = headroom_proxy_command(
            Path::new("headroom"),
            DEFAULT_PORT,
            Some(Path::new("app-data")),
            false,
            None,
        );
        let env_value = |key: &str| {
            command
                .get_envs()
                .find_map(|(name, value)| (name == OsStr::new(key)).then_some(value).flatten())
        };
        assert_eq!(env_value("HEADROOM_BEACON"), Some(OsStr::new("off")));
        assert_eq!(env_value("DO_NOT_TRACK"), Some(OsStr::new("1")));
        assert_eq!(env_value("HEADROOM_UPDATE_CHECK"), Some(OsStr::new("off")));
        assert_eq!(env_value("RTK_TELEMETRY_DISABLED"), Some(OsStr::new("1")));
    }

    #[test]
    fn managed_proxy_command_sets_managed_sidecar_marker() {
        let app_data_dir = PathBuf::from("app-data");
        let command = headroom_proxy_command(
            Path::new("headroom"),
            DEFAULT_PORT,
            Some(&app_data_dir),
            false,
            None,
        );
        let marker_value = command.get_envs().find_map(|(name, value)| {
            (name == OsStr::new(HALV_MANAGED_SIDECAR_ENV))
                .then_some(value)
                .flatten()
        });
        let compress_user_messages_value = command.get_envs().find_map(|(name, value)| {
            (name == OsStr::new("HEADROOM_COMPRESS_USER_MESSAGES"))
                .then_some(value)
                .flatten()
        });
        let compress_responses_messages_value = command.get_envs().find_map(|(name, value)| {
            (name == OsStr::new("HEADROOM_COMPRESS_RESPONSES_MESSAGES"))
                .then_some(value)
                .flatten()
        });
        let protect_recent_value = command.get_envs().find_map(|(name, value)| {
            (name == OsStr::new("HEADROOM_PROTECT_RECENT"))
                .then_some(value)
                .flatten()
        });
        let mode_value = command.get_envs().find_map(|(name, value)| {
            (name == OsStr::new("HEADROOM_MODE"))
                .then_some(value)
                .flatten()
        });
        let supervised_value = command.get_envs().find_map(|(name, value)| {
            (name == OsStr::new("HEADROOM_DESKTOP_SUPERVISED"))
                .then_some(value)
                .flatten()
        });
        let workspace_value = command.get_envs().find_map(|(name, value)| {
            (name == OsStr::new("HEADROOM_WORKSPACE_DIR"))
                .then_some(value)
                .flatten()
        });
        let hf_home_value = command
            .get_envs()
            .find_map(|(name, value)| (name == OsStr::new("HF_HOME")).then_some(value).flatten());
        let rtk_db_value = command.get_envs().find_map(|(name, value)| {
            (name == OsStr::new(crate::exec::RTK_DB_PATH_ENV))
                .then_some(value)
                .flatten()
        });
        let rtk_path_value = command
            .get_envs()
            .find_map(|(name, value)| (name == OsStr::new("PATH")).then_some(value).flatten());
        let rtk_telemetry_value = command.get_envs().find_map(|(name, value)| {
            (name == OsStr::new("RTK_TELEMETRY_DISABLED"))
                .then_some(value)
                .flatten()
        });

        assert_eq!(marker_value, Some(OsStr::new(HALV_MANAGED_SIDECAR_VALUE)));
        assert_eq!(compress_user_messages_value, Some(OsStr::new("1")));
        assert_eq!(compress_responses_messages_value, Some(OsStr::new("1")));
        assert_eq!(protect_recent_value, Some(OsStr::new("2")));
        assert_eq!(mode_value, Some(OsStr::new("token")));
        assert_eq!(supervised_value, Some(OsStr::new("1")));
        assert_eq!(
            workspace_value,
            Some(app_data_dir.join("headroom/workspace").as_os_str())
        );
        assert_eq!(
            hf_home_value,
            Some(app_data_dir.join("headroom/huggingface").as_os_str())
        );
        assert_eq!(
            rtk_db_value,
            Some(crate::exec::rtk_database_path().as_os_str())
        );
        if let Some(rtk_path_value) = rtk_path_value {
            let rtk_name = if cfg!(windows) { "rtk.exe" } else { "rtk" };
            assert!(
                std::env::split_paths(rtk_path_value)
                    .any(|directory| directory.join(rtk_name).is_file()),
                "managed Headroom PATH should resolve the exact RTK launcher"
            );
        }
        assert_eq!(rtk_telemetry_value, Some(OsStr::new("1")));
        assert!(!command
            .get_args()
            .any(|argument| argument == OsStr::new("--no-optimize")));
        assert!(command
            .get_args()
            .any(|argument| argument == OsStr::new("--no-http2")));
    }

    #[test]
    fn managed_listener_guard_accepts_each_verified_signal_and_refuses_foreign_listener() {
        assert_eq!(
            halv_managed_listener_port(&running_status(true, false, 4101)),
            Some(4101)
        );
        assert_eq!(
            halv_managed_listener_port(&running_status(false, true, 4102)),
            Some(4102)
        );
        assert_eq!(
            halv_managed_listener_port(&running_status(false, false, 4103)),
            None
        );
    }

    #[test]
    fn isolated_openai_proxy_command_sets_upstream_and_stateless_mode() {
        let upstream = SidecarUpstream::OpenAi("https://api.moonshot.ai/v1".to_string());
        let command = headroom_proxy_command(
            Path::new("headroom"),
            DEFAULT_PORT,
            None,
            false,
            Some(&upstream),
        );
        let upstream_value = command.get_envs().find_map(|(name, value)| {
            (name == OsStr::new("OPENAI_TARGET_API_URL"))
                .then_some(value)
                .flatten()
        });

        assert_eq!(
            upstream_value,
            Some(OsStr::new("https://api.moonshot.ai/v1"))
        );
        assert!(command
            .get_args()
            .any(|argument| argument == OsStr::new("--stateless")));
    }

    #[test]
    fn paused_proxy_command_disables_optimization() {
        let command = headroom_proxy_command(Path::new("headroom"), DEFAULT_PORT, None, true, None);
        let arguments = command
            .get_args()
            .map(|argument| argument.to_string_lossy().into_owned())
            .collect::<Vec<_>>();

        assert_eq!(arguments.first().map(String::as_str), Some("proxy"));
        assert_eq!(arguments.get(1).map(String::as_str), Some("--no-optimize"));
        assert!(has_exact_headroom_proxy_command_shape(&format!(
            "headroom {}",
            arguments.join(" ")
        )));
    }

    #[test]
    fn reap_decision_kills_marked_orphan() {
        let candidate = reap_candidate(101, 1, true, true);

        assert_eq!(orphaned_managed_sidecar_pids(&[candidate], None), vec![101]);
    }

    #[test]
    fn reap_decision_keeps_marked_current_child() {
        let candidate = reap_candidate(102, 1, true, true);

        assert!(orphaned_managed_sidecar_pids(&[candidate], Some(102)).is_empty());
    }

    #[test]
    fn reap_decision_keeps_unmarked_orphan() {
        let candidate = reap_candidate(103, 1, true, false);

        assert!(orphaned_managed_sidecar_pids(&[candidate], None).is_empty());
    }

    #[test]
    fn reap_decision_keeps_unmarked_process_with_live_parent() {
        let candidate = reap_candidate(104, 42, true, false);

        assert!(orphaned_managed_sidecar_pids(&[candidate], None).is_empty());
    }

    struct FakeStatsServer {
        port: u16,
        requests: Arc<AtomicUsize>,
        stop: Arc<AtomicBool>,
        thread: Option<JoinHandle<()>>,
    }

    impl FakeStatsServer {
        fn start(exit_marker: Option<PathBuf>) -> Option<Self> {
            let listener = match TcpListener::bind(("127.0.0.1", 0)) {
                Ok(listener) => listener,
                Err(error) => {
                    eprintln!("skipping fake-listener test; cannot bind localhost: {error}");
                    return None;
                }
            };
            listener
                .set_nonblocking(true)
                .expect("fake listener should become nonblocking");
            let port = listener
                .local_addr()
                .expect("fake listener should have an address")
                .port();
            let requests = Arc::new(AtomicUsize::new(0));
            let stop = Arc::new(AtomicBool::new(false));
            let requests_for_thread = requests.clone();
            let stop_for_thread = stop.clone();
            let thread = thread::spawn(move || {
                while !stop_for_thread.load(AtomicOrdering::SeqCst) {
                    match listener.accept() {
                        Ok((mut stream, _)) => {
                            let _ = stream.set_read_timeout(Some(Duration::from_secs(1)));
                            let mut request = [0_u8; 4096];
                            let _ = stream.read(&mut request);
                            requests_for_thread.fetch_add(1, AtomicOrdering::SeqCst);
                            if let Some(marker) = exit_marker.as_ref() {
                                let _ = fs::write(marker, b"exit");
                            }
                            let body = r#"{"requests":{}}"#;
                            let response = format!(
                                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                                body.len()
                            );
                            let _ = stream.write_all(response.as_bytes());
                        }
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            thread::sleep(Duration::from_millis(5));
                        }
                        Err(_) => break,
                    }
                }
            });
            Some(Self {
                port,
                requests,
                stop,
                thread: Some(thread),
            })
        }

        fn request_count(&self) -> usize {
            self.requests.load(AtomicOrdering::SeqCst)
        }
    }

    impl Drop for FakeStatsServer {
        fn drop(&mut self) {
            self.stop.store(true, AtomicOrdering::SeqCst);
            if let Some(thread) = self.thread.take() {
                let _ = thread.join();
            }
        }
    }

    fn captured_stats() -> HeadroomStats {
        serde_json::from_str(CAPTURED_STATS).expect("captured /stats fixture should parse")
    }

    fn captured_stats_without_agent_usage() -> HeadroomStats {
        let mut value: Value =
            serde_json::from_str(CAPTURED_STATS).expect("captured /stats fixture should parse");
        value
            .as_object_mut()
            .expect("captured /stats fixture should be a JSON object")
            .remove("agent_usage");
        serde_json::from_value(value).expect("captured /stats fixture variant should parse")
    }

    fn assert_f64_close(actual: f64, expected: f64) {
        assert!(
            (actual - expected).abs() < 0.000_000_001,
            "expected {actual} to be close to {expected}"
        );
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("halv-sidecar-{name}-{}", unique_suffix()));
        fs::create_dir_all(&dir).expect("temp dir should create");
        dir
    }

    fn unique_suffix() -> String {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default();
        format!("{}-{nanos}", std::process::id())
    }

    fn one_dir_path_env(dir: &Path) -> OsString {
        std::env::join_paths([dir]).expect("test PATH should build")
    }

    fn cleanup_dir(path: &Path) {
        let _ = fs::remove_dir_all(path);
    }

    fn write_file(path: &Path, contents: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("parent dir should create");
        }
        fs::write(path, contents).expect("test file should write");
    }

    #[cfg(unix)]
    fn write_executable(path: &Path, contents: &str) {
        write_file(path, contents);
        let mut permissions = fs::metadata(path)
            .expect("script metadata should read")
            .permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(path, permissions).expect("script should be executable");
    }

    fn env_lock() -> &'static StdMutex<()> {
        static LOCK: OnceLock<StdMutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| StdMutex::new(()))
    }

    #[test]
    fn locate_headroom_binary_prefers_override_then_bundle_then_legacy_locations() {
        let _guard = env_lock().lock().expect("env lock should not be poisoned");
        let root = temp_dir("resolver-precedence");
        let app_data = root.join("app-data");
        let resources = root.join("resources");
        let path_dir = root.join("path-bin");
        let home = root.join("home");
        let explicit_headroom = root.join("explicit").join(binary_name());
        let bundled_headroom = resources
            .join("headroom-runtime")
            .join(bundled_binary_name());
        let app_data_headroom = app_data.join("bin").join(binary_name());
        let path_headroom = path_dir.join(binary_name());
        let home_headroom = home.join(".local/bin").join(binary_name());

        write_file(&explicit_headroom, "explicit");
        write_file(&bundled_headroom, "bundled");
        write_file(&app_data_headroom, "app-data");
        write_file(&path_headroom, "path");
        write_file(&home_headroom, "home");

        let path_env = one_dir_path_env(&path_dir);
        assert_eq!(
            locate_headroom_binary_with_env(
                Some(&app_data),
                Some(&resources),
                Some(path_env.clone()),
                Some(home.clone().into_os_string()),
                Some(explicit_headroom.clone().into_os_string()),
            ),
            Some(explicit_headroom.clone())
        );

        fs::remove_file(&explicit_headroom).expect("explicit binary should remove");
        assert_eq!(
            locate_headroom_binary_with_env(
                Some(&app_data),
                Some(&resources),
                Some(path_env.clone()),
                Some(home.clone().into_os_string()),
                None,
            ),
            Some(bundled_headroom.clone())
        );

        fs::remove_file(&bundled_headroom).expect("bundled binary should remove");
        assert_eq!(
            locate_headroom_binary_with_env(
                Some(&app_data),
                Some(&resources),
                Some(path_env.clone()),
                Some(home.clone().into_os_string()),
                None,
            ),
            Some(app_data_headroom.clone())
        );

        fs::remove_file(&app_data_headroom).expect("app-data binary should remove");
        assert_eq!(
            locate_headroom_binary_with_env(
                Some(&app_data),
                Some(&resources),
                Some(path_env.clone()),
                Some(home.clone().into_os_string()),
                None,
            ),
            Some(path_headroom.clone())
        );

        fs::remove_file(&path_headroom).expect("PATH binary should remove");
        assert_eq!(
            locate_headroom_binary_with_env(
                Some(&app_data),
                Some(&resources),
                Some(path_env),
                Some(home.into_os_string()),
                None,
            ),
            Some(home_headroom)
        );
        cleanup_dir(&root);
    }

    #[test]
    fn pinned_headroom_version_comes_from_the_immutable_bundle_lock() {
        assert_eq!(pinned_headroom_version(), "0.35.0-dev+halv.g1dc3f3a0c0f3");
    }

    #[test]
    fn managed_detection_accepts_bundled_and_legacy_app_data_paths() {
        let app_data = PathBuf::from("app-data");
        let resources = PathBuf::from("resources");
        let managed_install = app_data.join("bin").join(binary_name());
        let bundled = resources
            .join("headroom-runtime")
            .join(bundled_binary_name());
        assert!(is_halv_managed_binary_path(
            Some(&managed_install),
            Some(app_data.as_path()),
            Some(resources.as_path()),
        ));
        assert!(is_halv_managed_binary_path(
            Some(&bundled),
            Some(app_data.as_path()),
            Some(resources.as_path()),
        ));
        assert!(!is_halv_managed_binary_path(
            Some(Path::new("/usr/local/bin").join(binary_name()).as_path()),
            Some(app_data.as_path()),
            Some(resources.as_path()),
        ));
        assert!(!is_halv_managed_binary_path(
            None,
            Some(&app_data),
            Some(&resources)
        ));
    }

    #[test]
    fn running_status_serializes_managed_and_pinned_fields() {
        let status = SidecarStatus::Running {
            spawned_by_us: true,
            managed_by_halv: true,
            optimization_paused: true,
            version: Some("0.27.0".to_string()),
            pinned_version: pinned_headroom_version().to_string(),
            port: DEFAULT_PORT,
        };
        let value = serde_json::to_value(&status).expect("running status should serialize");
        assert_eq!(value["state"], "running");
        assert_eq!(value["managedByHalv"], true);
        assert_eq!(value["optimizationPaused"], true);
        assert_eq!(value["pinnedVersion"], pinned_headroom_version());
        assert_eq!(value["version"], "0.27.0");
    }

    #[test]
    fn stats_timeouts_restart_only_after_a_run_of_three() {
        let manager = SidecarManager::new(DEFAULT_PORT);
        assert!(!manager.note_stats_timeout());
        assert!(!manager.note_stats_timeout());
        // A good answer in between starts the count again.
        manager.clear_stats_timeouts();
        assert!(!manager.note_stats_timeout());
        assert!(!manager.note_stats_timeout());
        assert!(manager.note_stats_timeout());
        assert_eq!(manager.inner.stats_timeouts.load(Ordering::SeqCst), 0);
    }

    #[cfg(unix)]
    #[test]
    fn the_supervisor_log_keeps_one_line_per_exit_with_the_stderr_tail() {
        let exit = ManagedChildExit {
            status: Command::new("/bin/sh")
                .args(["-c", "exit 3"])
                .status()
                .expect("sh should run"),
            stderr_lines: (0..25).map(|index| format!("line {index}")).collect(),
        };
        let at = chrono::DateTime::parse_from_rfc3339("2026-10-19T12:00:00Z")
            .expect("fixed time")
            .with_timezone(&chrono::Utc);
        let line = supervisor_log_line(at, 8787, &exit, 2);
        assert!(line.starts_with("2026-10-19T12:00:00Z port=8787 exit=\"exit status: 3\" restarts_in_window=2 stderr=\"line 5 | "), "{line}");
        assert!(line.ends_with("| line 24\""), "{line}");
        assert!(!line.contains("line 4 |"));

        let root = temp_dir("supervisor-log");
        append_supervisor_log(&root, &line, 64).expect("first write");
        append_supervisor_log(&root, &line, 64).expect("second write moves the full log aside");
        let logs = root.join("logs");
        assert_eq!(
            fs::read_to_string(logs.join(SUPERVISOR_LOG_FILE_NAME)).expect("new log"),
            format!("{line}\n")
        );
        assert!(logs
            .join(format!("{SUPERVISOR_LOG_FILE_NAME}.old"))
            .exists());
        cleanup_dir(&root);
    }

    #[tokio::test]
    async fn a_restart_waits_for_the_saved_port_and_keeps_it() {
        let manager = SidecarManager::new(0);
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("bind a port");
        let saved = listener.local_addr().expect("local addr").port();
        drop(listener);

        // A free saved port comes back at once.
        let port = manager
            .select_start_port_waiting(saved)
            .await
            .expect("free port");
        assert_eq!(port, saved);

        // A busy saved port releases during the wait.
        let holder = std::net::TcpListener::bind(("127.0.0.1", saved)).expect("hold the port");
        let release = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(300)).await;
            drop(holder);
        });
        let started = Instant::now();
        let port = manager
            .select_start_port_waiting(saved)
            .await
            .expect("port after the wait");
        release.await.expect("release task");
        assert_eq!(port, saved);
        assert!(started.elapsed() >= Duration::from_millis(250));
        assert!(started.elapsed() < SAVED_PORT_WAIT);
    }

    #[test]
    fn installing_status_serializes_with_state_tag() {
        let status = SidecarStatus::Installing { port: DEFAULT_PORT };
        let value = serde_json::to_value(&status).expect("installing status should serialize");
        assert_eq!(
            value,
            serde_json::json!({ "state": "installing", "port": DEFAULT_PORT })
        );
    }

    #[test]
    fn automatic_heal_never_stops_and_waits_longer_each_time() {
        assert_eq!(auto_heal_backoff(0), Duration::ZERO);
        assert_eq!(auto_heal_backoff(1), Duration::from_secs(1));
        assert_eq!(auto_heal_backoff(2), Duration::from_secs(2));
        assert_eq!(auto_heal_backoff(3), Duration::from_secs(4));
        assert_eq!(auto_heal_backoff(6), AUTO_HEAL_BACKOFF_MAX);
        assert_eq!(auto_heal_backoff(40), AUTO_HEAL_BACKOFF_MAX);

        let manager = SidecarManager::new(DEFAULT_PORT);
        let now = Instant::now();
        let mut delays = Vec::new();
        for _ in 0..8 {
            match manager.reserve_auto_heal_at(now) {
                AutoHealReservation::Reserved(_, delay) => delays.push(delay),
                other => panic!("every restart should be reserved, got {other:?}"),
            }
            manager
                .inner
                .auto_heal_in_flight
                .store(false, Ordering::SeqCst);
        }
        assert_eq!(
            delays,
            [0, 1, 2, 4, 8, 16, 30, 30]
                .into_iter()
                .map(Duration::from_secs)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn automatic_heal_budget_prunes_expired_attempts() {
        let manager = SidecarManager::new(DEFAULT_PORT);
        let base = Instant::now();

        for _ in 0..3 {
            assert!(matches!(
                manager.reserve_auto_heal_at(base),
                AutoHealReservation::Reserved(..)
            ));
            manager
                .inner
                .auto_heal_in_flight
                .store(false, Ordering::SeqCst);
        }

        let after_window = base + AUTO_HEAL_WINDOW + Duration::from_millis(1);
        assert!(matches!(
            manager.reserve_auto_heal_at(after_window),
            AutoHealReservation::Reserved(_, Duration::ZERO)
        ));
        assert_eq!(
            manager
                .inner
                .auto_heal_attempts
                .lock()
                .expect("auto-heal attempts lock should not be poisoned")
                .len(),
            1
        );
    }

    #[test]
    fn automatic_heal_guard_prevents_second_in_flight_attempt() {
        let manager = SidecarManager::new(DEFAULT_PORT);
        let now = Instant::now();

        assert!(matches!(
            manager.reserve_auto_heal_at(now),
            AutoHealReservation::Reserved(..)
        ));
        assert_eq!(
            manager.reserve_auto_heal_at(now),
            AutoHealReservation::InFlight
        );
        assert_eq!(
            manager
                .inner
                .auto_heal_attempts
                .lock()
                .expect("auto-heal attempts lock should not be poisoned")
                .len(),
            1
        );

        manager
            .inner
            .auto_heal_in_flight
            .store(false, Ordering::SeqCst);
        assert!(matches!(
            manager.reserve_auto_heal_at(now),
            AutoHealReservation::Reserved(..)
        ));
    }

    #[cfg(unix)]
    #[test]
    fn deserializes_captured_real_headroom_stats() {
        let stats = captured_stats();
        let sidecar_stats = stats.to_sidecar_stats(DEFAULT_PORT, 0, &HashMap::new());

        assert_eq!(
            stats
                .summary
                .as_ref()
                .and_then(|summary| summary.mode.as_deref()),
            Some("token")
        );
        assert!(sidecar_stats.tokens_saved > 0);
        assert!(sidecar_stats.cost_saved_usd > 0.0);
        assert!(sidecar_stats.requests_total > 0);
        assert!(sidecar_stats.process_input_tokens > 0);
    }

    #[test]
    fn reads_project_tokens_saved_from_persistent_stats() {
        let stats: HeadroomStats = serde_json::from_value(serde_json::json!({
            "persistent_savings": {
                "projects": {
                    "session-123": {
                        "tokens_saved": 456,
                        "compression_savings_usd": 0.012
                    }
                }
            }
        }))
        .expect("project savings fixture should parse");

        assert_eq!(stats.project_tokens_saved("session-123"), 456);
        assert_eq!(stats.project_tokens_saved("missing"), 0);
        assert_eq!(
            stats.project_savings_totals("session-123"),
            ProjectSavingsTotals {
                tokens_saved: 456,
                savings_usd: Some(0.012),
            }
        );
    }

    #[test]
    fn project_tokens_saved_reads_the_savings_per_project_map() {
        let stats = serde_json::from_value::<HeadroomStats>(serde_json::json!({
            "savings": {
                "total_tokens": 1000,
                "per_project": {
                    "session-123": { "tokens_saved": 789 }
                }
            }
        }))
        .expect("per-project savings fixture should parse");
        assert_eq!(stats.project_tokens_saved("session-123"), 789);
        assert_eq!(stats.project_tokens_saved("other"), 0);
    }

    #[test]
    fn overlapping_project_rollups_keep_each_turn_isolated() {
        let before = serde_json::from_value::<HeadroomStats>(serde_json::json!({
            "persistent_savings": {
                "lifetime": { "tokens_saved": 100 }
            },
            "savings": { "per_project": {} }
        }))
        .expect("before /stats payload should parse");
        let after = serde_json::from_value::<HeadroomStats>(serde_json::json!({
            "persistent_savings": {
                "lifetime": { "tokens_saved": 140 }
            },
            "savings": {
                "per_project": {
                    "gui:conversation:turn-a": {
                        "tokens_saved": 20,
                        "compression_savings_usd": 0.2
                    },
                    "gui:conversation:turn-b": {
                        "tokens_saved": 20,
                        "compression_savings_usd": 0.2
                    }
                }
            }
        }))
        .expect("after /stats payload should parse");

        for project in ["gui:conversation:turn-a", "gui:conversation:turn-b"] {
            let before = before.project_savings_totals(project);
            let after = after.project_savings_totals(project);
            assert_eq!(after.tokens_saved.saturating_sub(before.tokens_saved), 20);
            assert_eq!(after.savings_usd, Some(0.2));
        }
    }

    #[test]
    fn usage_by_lane_separates_plan_and_api_costs() {
        let stats = serde_json::from_value::<HeadroomStats>(serde_json::json!({
            "savings": {
                "per_project": {
                    "claude-plan": {
                        "requests": 2,
                        "total_input_tokens": 120,
                        "total_input_cost_usd": 0.12,
                        "tokens_saved": 80
                    },
                    "codex-plan": {
                        "requests": 3,
                        "total_input_tokens": 230,
                        "total_input_cost_usd": 0.23,
                        "tokens_saved": 140
                    },
                    "codex-plan-secondary": {
                        "requests": 1,
                        "total_input_tokens": 70,
                        "total_input_cost_usd": 0.07,
                        "tokens_saved": 60
                    },
                    "kimi-api": {
                        "requests": 4,
                        "total_input_tokens": 340,
                        "total_input_cost_usd": 1.25,
                        "tokens_saved": 210
                    }
                }
            }
        }))
        .expect("lane usage fixture should parse");
        let lanes = HashMap::from([
            (
                "claude-plan".to_string(),
                SessionLaneKind::ClaudeSubscription,
            ),
            ("codex-plan".to_string(), SessionLaneKind::CodexChatGpt),
            (
                "codex-plan-secondary".to_string(),
                SessionLaneKind::CodexChatGpt,
            ),
            ("kimi-api".to_string(), SessionLaneKind::KimiApiKey),
        ]);

        let (usage, unattributed) = stats.usage_by_lane(&lanes);

        assert_eq!(usage.len(), 3);
        assert_eq!(usage.iter().filter(|row| row.billing == "plan").count(), 2);
        assert!(usage
            .iter()
            .filter(|row| row.billing == "plan")
            .all(|row| row.cost_usd.is_none()));
        assert_eq!(
            usage
                .iter()
                .find(|row| row.lane == "codex_chatgpt")
                .expect("Codex plan usage should exist")
                .tokens_saved,
            200
        );
        assert_eq!(
            usage
                .iter()
                .find(|row| row.lane == "kimi_api_key")
                .expect("Kimi API usage should exist"),
            &LaneUsage {
                lane: "kimi_api_key".to_string(),
                billing: "api".to_string(),
                provider: "kimi".to_string(),
                requests: 4,
                input_tokens: 340,
                output_tokens: None,
                cost_usd: Some(1.25),
                tokens_saved: 210,
            }
        );
        assert_eq!(unattributed, LaneUsage::unattributed());
    }

    #[test]
    fn usage_by_lane_collects_unknown_projects_without_pricing_them() {
        let stats = serde_json::from_value::<HeadroomStats>(serde_json::json!({
            "persistent_savings": {
                "projects": {
                    "missing-session": {
                        "requests": 7,
                        "total_input_tokens": 890,
                        "total_input_cost_usd": 3.45
                    }
                }
            }
        }))
        .expect("unattributed usage fixture should parse");

        let (usage, unattributed) = stats.usage_by_lane(&HashMap::new());

        assert!(usage.is_empty());
        assert_eq!(
            unattributed,
            LaneUsage {
                lane: "unknown".to_string(),
                billing: "unknown".to_string(),
                provider: "custom".to_string(),
                requests: 7,
                input_tokens: 890,
                output_tokens: None,
                cost_usd: None,
                tokens_saved: 0,
            }
        );
    }

    #[test]
    fn usage_by_lane_is_empty_without_project_rollups() {
        let stats = HeadroomStats::default();
        let lanes = HashMap::from([("codex-plan".to_string(), SessionLaneKind::CodexChatGpt)]);

        let (usage, unattributed) = stats.usage_by_lane(&lanes);

        assert!(usage.is_empty());
        assert_eq!(unattributed, LaneUsage::unattributed());
    }

    #[test]
    fn to_sidecar_stats_attributes_lifetime_savings_to_codex_agent() {
        let stats = captured_stats();
        let sidecar_stats = stats.to_sidecar_stats(DEFAULT_PORT, 0, &HashMap::new());
        let (agent_tokens, agent_requests) = stats
            .halv_agent_savings()
            .expect("fixture should include a codex agent row");
        let lifetime = stats
            .persistent_savings
            .as_ref()
            .and_then(|stats| stats.lifetime.as_ref())
            .expect("fixture should include lifetime savings");
        let global_tokens = lifetime
            .tokens_saved
            .expect("fixture should include lifetime tokens");
        let global_cost = lifetime
            .compression_savings_usd
            .expect("fixture should include lifetime USD savings");
        let expected_cost = global_cost * (agent_tokens as f64 / global_tokens as f64);

        assert_eq!(agent_tokens, 24_000);
        assert_eq!(agent_requests, 12);
        assert_eq!(sidecar_stats.lifetime_tokens_saved, Some(global_tokens));
        assert_eq!(sidecar_stats.lifetime_cost_saved_usd, Some(global_cost));
        assert_eq!(sidecar_stats.tokens_saved, agent_tokens);
        assert_eq!(sidecar_stats.requests_total, agent_requests);
        assert_f64_close(sidecar_stats.cost_saved_usd, expected_cost);
        assert_eq!(sidecar_stats.session_tokens_saved, 464_887);
        assert_f64_close(sidecar_stats.session_cost_saved_usd, 4.386_785);
    }

    #[test]
    fn to_sidecar_stats_falls_back_to_global_lifetime_without_agent_usage() {
        let stats = captured_stats_without_agent_usage();
        let sidecar_stats = stats.to_sidecar_stats(DEFAULT_PORT, 0, &HashMap::new());

        assert!(stats.agent_usage.is_none());
        assert_eq!(sidecar_stats.tokens_saved, 464_887);
        assert_eq!(sidecar_stats.requests_total, 653);
        assert_f64_close(sidecar_stats.cost_saved_usd, 4.386_785);
    }

    #[test]
    fn to_sidecar_stats_falls_back_to_global_lifetime_without_matching_agent() {
        let mut stats = captured_stats();
        let agent_usage = stats
            .agent_usage
            .as_mut()
            .expect("fixture should include agent usage");
        for agent in &mut agent_usage.agents {
            agent.agent = Some("claude-code".to_string());
        }

        assert!(stats.agent_usage.is_some());
        assert_eq!(stats.halv_agent_savings(), None);

        let sidecar_stats = stats.to_sidecar_stats(DEFAULT_PORT, 0, &HashMap::new());
        assert_eq!(sidecar_stats.compression_tokens_saved, 464_887);
        assert_eq!(sidecar_stats.tokens_saved, 464_887);
        assert_f64_close(sidecar_stats.cost_saved_usd, 4.386_785);
    }

    #[test]
    fn to_sidecar_stats_counts_every_exact_layer_and_keeps_estimates_separate() {
        let stats: HeadroomStats = serde_json::from_str(
            r#"{
                "agent_usage": {
                    "agents": [{"agent": "codex", "requests": 2, "tokens_saved": 100}]
                },
                "tokens": {
                    "proxy_compression_saved": 100,
                    "cli_filtering_saved": 37,
                    "rtk_saved": 37,
                    "lean_ctx_saved": 0,
                    "all_layers_saved": 137,
                    "output_saved": 13
                },
                "savings": {
                    "by_layer": {
                        "tool_search": {"tokens_saved": 11}
                    }
                },
                "summary": {
                    "mcp": {"tokens_removed": 7},
                    "codex_ws": {"tokens_saved": 200}
                },
                "persistent_savings": {
                    "lifetime": {
                        "tokens_saved": 100,
                        "compression_savings_usd": 0.5
                    }
                }
            }"#,
        )
        .expect("RTK savings fixture should parse");

        let mut sidecar_stats = stats.to_sidecar_stats(DEFAULT_PORT, 0, &HashMap::new());
        assert_eq!(sidecar_stats.tokens_saved, 148);
        assert_eq!(sidecar_stats.proxy_compression_tokens_saved, 100);
        assert_eq!(sidecar_stats.compression_tokens_saved, 100);
        assert_eq!(sidecar_stats.cli_filtering_tokens_saved, 37);
        assert_eq!(sidecar_stats.rtk_tokens_saved, 37);
        assert_eq!(sidecar_stats.lean_ctx_tokens_saved, 0);
        assert_eq!(sidecar_stats.tool_search_tokens_saved, 11);
        assert_eq!(sidecar_stats.estimated_output_tokens_saved, 13);
        assert_eq!(sidecar_stats.mcp_tokens_saved, 7);
        assert_eq!(sidecar_stats.live_ws_tokens_saved, 200);
        assert_f64_close(sidecar_stats.cost_saved_usd, 0.5);

        sidecar_stats.replace_rtk_tokens_saved(42);
        assert_eq!(sidecar_stats.tokens_saved, 153);
        assert_eq!(sidecar_stats.cli_filtering_tokens_saved, 42);
        assert_eq!(sidecar_stats.rtk_tokens_saved, 42);
        assert_f64_close(sidecar_stats.cost_saved_usd, 0.5);
    }

    #[test]
    fn to_sidecar_stats_counts_non_rtk_cli_filters_from_all_layers() {
        let stats: HeadroomStats = serde_json::from_str(
            r#"{
                "agent_usage": {
                    "agents": [{"agent": "codex", "requests": 1, "tokens_saved": 100}]
                },
                "tokens": {
                    "proxy_compression_saved": 100,
                    "cli_filtering_saved": 9,
                    "rtk_saved": 0,
                    "lean_ctx_saved": 9,
                    "all_layers_saved": 109
                }
            }"#,
        )
        .expect("Lean Context savings fixture should parse");

        let sidecar_stats = stats.to_sidecar_stats(DEFAULT_PORT, 0, &HashMap::new());
        assert_eq!(sidecar_stats.tokens_saved, 109);
        assert_eq!(sidecar_stats.cli_filtering_tokens_saved, 9);
        assert_eq!(sidecar_stats.rtk_tokens_saved, 0);
        assert_eq!(sidecar_stats.lean_ctx_tokens_saved, 9);
    }

    #[test]
    fn to_sidecar_stats_preserves_openai_and_anthropic_request_counts() {
        let stats: HeadroomStats = serde_json::from_str(
            r#"{
                "requests": {
                    "total": 10,
                    "by_provider": {
                        "openai": 7,
                        "anthropic": 3
                    }
                }
            }"#,
        )
        .expect("provider usage fixture should parse");
        let sidecar_stats = stats.to_sidecar_stats(DEFAULT_PORT, 0, &HashMap::new());

        assert_eq!(sidecar_stats.requests_by_provider.get("openai"), Some(&7));
        assert_eq!(
            sidecar_stats.requests_by_provider.get("anthropic"),
            Some(&3)
        );
    }

    #[test]
    fn to_sidecar_stats_maps_codex_plan_windows() {
        let stats = captured_stats();
        let sidecar_stats = stats.to_sidecar_stats(DEFAULT_PORT, 0, &HashMap::new());
        let windows = sidecar_stats
            .plan_windows
            .expect("captured fixture should include plan windows");

        assert_eq!(windows.len(), 1);
        assert_eq!(
            windows[0],
            PlanWindowStats {
                label: "7d".to_string(),
                used_percent: 81.0,
                resets_at: Some(1_783_388_592),
            }
        );
    }

    #[test]
    fn to_sidecar_stats_uses_plan_window_fallbacks_for_absent_fields() {
        let stats: HeadroomStats = serde_json::from_str(
            r#"{
                "summary": {"mode": "token"},
                "codex_rate_limits": {
                    "primary": {"used_percent": 42.4},
                    "secondary": {"used_percent": 91.2}
                }
            }"#,
        )
        .expect("plan-window fallback fixture should parse");
        let windows = stats
            .to_sidecar_stats(DEFAULT_PORT, 0, &HashMap::new())
            .plan_windows
            .expect("fallback fixture should include usable plan windows");

        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0].label, "7d");
        assert_f64_close(windows[0].used_percent, 91.2);
        assert_eq!(windows[0].resets_at, None);
    }

    #[test]
    fn to_sidecar_stats_uses_primary_when_it_is_the_weekly_window() {
        let stats: HeadroomStats = serde_json::from_str(
            r#"{
                "summary": {"mode": "token"},
                "codex_rate_limits": {
                    "primary": {
                        "used_percent": 1.0,
                        "window_minutes": 10080,
                        "window_label": "168h",
                        "resets_at": 1784489086
                    },
                    "secondary": {
                        "used_percent": 0.0,
                        "window_minutes": 0,
                        "window_label": "0m"
                    }
                }
            }"#,
        )
        .expect("current weekly-only fixture should parse");
        let windows = stats
            .to_sidecar_stats(DEFAULT_PORT, 0, &HashMap::new())
            .plan_windows
            .expect("current fixture should include the weekly window");

        assert_eq!(
            windows,
            vec![PlanWindowStats {
                label: "7d".to_string(),
                used_percent: 1.0,
                resets_at: Some(1_784_489_086),
            }]
        );
    }

    #[test]
    fn halv_agent_savings_sums_case_insensitive_related_codex_rows() {
        let stats: HeadroomStats = serde_json::from_str(
            r#"{
                "agent_usage": {
                    "agents": [
                        {"agent": "codex", "requests": 2, "tokens_saved": 5},
                        {"agent": "Codex", "requests": 3, "tokens_saved": 7},
                        {"agent": "codex-cli", "requests": 4, "tokens_saved": 11},
                        {"agent": "Halv-Codex", "requests": 5, "tokens_saved": 13},
                        {"agent": "halv_codex_terminal", "requests": 6, "tokens_saved": 17},
                        {"agent": "codexchange", "requests": 100, "tokens_saved": 1000},
                        {"agent": "claude-code", "requests": 100, "tokens_saved": 1000}
                    ]
                }
            }"#,
        )
        .expect("agent usage fixture should parse");

        assert_eq!(stats.halv_agent_savings(), Some((53, 20)));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn managed_child_exit_message_includes_recent_stderr_tail() {
        let root = temp_dir("stderr-exit-tail");
        let binary = root.join("headroom");
        write_executable(
            &binary,
            r#"#!/bin/sh
i=1
while [ "$i" -le 7 ]; do
  printf 'stderr-tail-%s\n' "$i" >&2
  i=$((i + 1))
done
exit 42
"#,
        );

        let manager = SidecarManager::new(DEFAULT_PORT);
        record_stderr_line(&manager.inner.stderr_lines, "stale-before-spawn");
        let child = manager
            .spawn_headroom_proxy(&binary, DEFAULT_PORT)
            .expect("stderr-tail proxy should spawn");
        manager.replace_child(Some(child));
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            if !manager.managed_child_is_running() {
                break;
            }
            if Instant::now() >= deadline {
                manager.shutdown();
                cleanup_dir(&root);
                panic!("stderr-tail proxy did not exit");
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let message = match manager.refresh_status().await {
            SidecarStatus::Error { message, port } => {
                assert_eq!(port, DEFAULT_PORT);
                message
            }
            status => panic!("exited proxy should produce an error status, got {status:?}"),
        };

        assert!(message.contains("exit status: 42"));
        assert!(message.contains("Last output:"));
        assert!(!message.contains("stale-before-spawn"));
        assert!(!message.contains("stderr-tail-1"));
        assert!(!message.contains("stderr-tail-2"));
        let mut previous_position = 0;
        for line_number in 3..=7 {
            let expected = format!("stderr-tail-{line_number}");
            let position = message
                .find(&expected)
                .unwrap_or_else(|| panic!("exit message should contain {expected}: {message}"));
            assert!(position >= previous_position);
            previous_position = position;
        }
        assert!(message.chars().count() <= CHILD_OUTPUT_MAX_CHARS);
        cleanup_dir(&root);
    }

    #[cfg(unix)]
    #[test]
    fn managed_child_startup_message_reports_selected_port_without_speculation() {
        use std::os::unix::process::ExitStatusExt;

        let exit = ManagedChildExit {
            status: ExitStatus::from_raw(42 << 8),
            stderr_lines: vec!["bind failed for configured address".to_string()],
        };

        let message = exit.startup_message(DEFAULT_PORT, "connection refused");

        assert!(message.contains(
            "managed proxy exited (exit status: 42) before becoming ready on selected port 8787"
        ));
        assert!(message.contains("Last readiness probe: connection refused"));
        assert!(message.contains("Last output: bind failed for configured address"));
        assert!(!message.contains("port may already be in use"));
    }

    #[test]
    fn stderr_ring_caps_line_length_and_line_count() {
        let stderr_lines = Mutex::new(VecDeque::new());
        for index in 0..HEADROOM_STDERR_RING_LINES + 3 {
            record_stderr_line(&stderr_lines, &format!("line-{index}"));
        }
        record_stderr_line(
            &stderr_lines,
            &"é".repeat(HEADROOM_STDERR_LINE_MAX_CHARS + 20),
        );

        let ring = stderr_lines
            .lock()
            .expect("stderr ring should not be poisoned");
        assert_eq!(ring.len(), HEADROOM_STDERR_RING_LINES);
        assert_eq!(ring.front().map(String::as_str), Some("line-4"));
        let final_line = ring.back().expect("long line should be retained");
        assert_eq!(final_line.chars().count(), HEADROOM_STDERR_LINE_MAX_CHARS);
        assert!(final_line.ends_with("..."));
        drop(ring);

        let message = message_with_stderr_tail(
            "x".repeat(CHILD_OUTPUT_MAX_CHARS * 2),
            &stderr_tail_snapshot(&stderr_lines),
        );
        assert!(message.contains(LAST_OUTPUT_MARKER));
        assert!(message.contains("line-42"));
        assert!(message.chars().count() <= CHILD_OUTPUT_MAX_CHARS);
    }

    #[test]
    fn headroom_proxy_log_rotates_at_configured_size_cap() {
        let app_data_dir = temp_dir("stderr-log-rotation");
        let logs_dir = app_data_dir.join("logs");
        let old_log_path = logs_dir.join("headroom-proxy.log.old");
        write_file(&old_log_path, "stale-old-log\n");
        let stderr_lines = Arc::new(Mutex::new(VecDeque::new()));
        let stderr_generation = Arc::new(AtomicU64::new(1));

        drain_child_stderr(
            Cursor::new(b"first-line-is-longer-than-cap\nfresh-line\n"),
            stderr_lines,
            stderr_generation,
            1,
            Arc::new(RwLock::new(Some(app_data_dir.clone()))),
            None,
            false,
            16,
        );

        assert_eq!(
            fs::read_to_string(&old_log_path).expect("rotated log should read"),
            "first-line-is-longer-than-cap\n"
        );
        assert_eq!(
            fs::read_to_string(logs_dir.join("headroom-proxy.log")).expect("fresh log should read"),
            "fresh-line\n"
        );
        cleanup_dir(&app_data_dir);
    }

    #[cfg(unix)]
    #[test]
    fn managed_proxy_cannot_stall_on_a_full_stderr_pipe() {
        let root = temp_dir("stderr-pressure");
        let binary = root.join("headroom");
        write_executable(
            &binary,
            r#"#!/bin/sh
i=0
while [ "$i" -lt 50000 ]; do
  printf '0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef\n' >&2
  i=$((i + 1))
done
exit 23
"#,
        );

        let manager = SidecarManager::new(DEFAULT_PORT);
        let app_data_dir = root.join("app-data");
        manager.set_app_data_dir(app_data_dir.clone());
        let child = manager
            .spawn_headroom_proxy(&binary, DEFAULT_PORT)
            .expect("stderr-pressure proxy should spawn");
        manager.replace_child(Some(child));
        let deadline = Instant::now() + Duration::from_secs(5);
        let exit = loop {
            if let Some(exit) = manager.take_child_exit() {
                break exit;
            }
            if Instant::now() >= deadline {
                manager.shutdown();
                cleanup_dir(&root);
                panic!("managed proxy stalled while writing stderr");
            }
            thread::sleep(Duration::from_millis(10));
        };

        assert_eq!(exit.status.code(), Some(23));
        assert_eq!(exit.stderr_lines.len(), HEADROOM_STDERR_MESSAGE_LINES);
        assert!(app_data_dir.join("logs/headroom-proxy.log").is_file());
        assert!(app_data_dir.join("logs/headroom-proxy.log.old").is_file());
        cleanup_dir(&root);
    }

    #[cfg(unix)]
    #[test]
    fn version_probe_is_bounded_and_killable() {
        let root = temp_dir("version-probe");
        let working = root.join("working-headroom");
        write_executable(&working, "#!/bin/sh\nprintf 'headroom 0.28.0\\n' >&2\n");
        assert_eq!(
            read_headroom_version_with_timeout(&working, Duration::from_secs(3)).as_deref(),
            Some("0.28.0")
        );

        let hung = root.join("hung-headroom");
        write_executable(&hung, "#!/bin/sh\nsleep 30\n");
        let started = Instant::now();
        assert_eq!(
            read_headroom_version_with_timeout(&hung, Duration::from_millis(100)),
            None
        );
        assert!(started.elapsed() < Duration::from_secs(3));
        cleanup_dir(&root);
    }

    #[test]
    fn timed_out_stats_probe_classifies_as_busy_and_not_unhealthy() {
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("bind test listener");
        let port = listener.local_addr().expect("listener address").port();
        // Accept connections and stay silent, like a worker busy with a
        // large request.
        std::thread::spawn(move || {
            let mut held = Vec::new();
            while let Ok((stream, _)) = listener.accept() {
                held.push(stream);
            }
        });
        let client = reqwest::Client::builder()
            .timeout(Duration::from_millis(200))
            .build()
            .expect("test client");
        let error = tauri::async_runtime::block_on(async {
            client
                .get(stats_url(port))
                .send()
                .await
                .expect_err("silent listener should time the probe out")
        });

        assert!(FetchStatsError::from_request_error(port, error).busy);
        assert!(!FetchStatsError::unhealthy("connection refused".to_string()).busy);
    }

    #[test]
    fn random_default_port_is_free_and_never_zero() {
        let port = random_default_port();
        assert_ne!(port, 0);
        // The reservation listener is closed, so the port binds again.
        std::net::TcpListener::bind(("127.0.0.1", port))
            .expect("the selected random port should be bindable");
    }

    #[test]
    fn saved_sidecar_port_file_round_trips() {
        let app_data_dir = temp_dir("saved-port");
        assert_eq!(
            read_saved_sidecar_port(&app_data_dir).expect("missing port file should load"),
            None
        );

        write_saved_sidecar_port(&app_data_dir, 43127).expect("saved port should write");

        assert_eq!(
            read_saved_sidecar_port(&app_data_dir).expect("saved port should load"),
            Some(43127)
        );
        cleanup_dir(&app_data_dir);
    }

    #[test]
    fn port_choice_prefers_a_free_port_and_falls_back_when_busy() {
        let listener = match TcpListener::bind(("127.0.0.1", 0)) {
            Ok(listener) => listener,
            Err(error) => {
                eprintln!("skipping local-port test; cannot bind localhost: {error}");
                return;
            }
        };
        let busy_port = listener
            .local_addr()
            .expect("listener should have a local address")
            .port();
        let fallback = choose_available_loopback_port(Some(busy_port))
            .expect("busy preferred port should fall back");
        assert!(!fallback.reused_preferred);
        assert_ne!(fallback.port, busy_port);
        drop(listener);

        let preferred = choose_available_loopback_port(Some(busy_port))
            .expect("free preferred port should be reused");
        assert_eq!(preferred.port, busy_port);
        assert!(preferred.reused_preferred);
    }

    #[test]
    fn occupied_requested_port_selects_unused_loopback_port_and_updates_manager() {
        let Some(fake) = FakeStatsServer::start(None) else {
            return;
        };
        let manager = SidecarManager::new(fake.port);

        let selected = manager
            .select_start_port(manager.requested_port())
            .expect("occupied preferred port should fall back");

        assert_ne!(selected, fake.port);
        assert_eq!(manager.port(), selected);
        let probe = TcpListener::bind(("127.0.0.1", selected))
            .expect("selected port should still be unused before child spawn");
        drop(probe);
        assert_eq!(fake.request_count(), 0);
    }

    #[test]
    fn free_requested_port_is_preserved() {
        const ATTEMPTS: usize = 5;
        let mut preserved = false;

        for _ in 0..ATTEMPTS {
            let listener = match TcpListener::bind(("127.0.0.1", 0)) {
                Ok(listener) => listener,
                Err(error) => {
                    eprintln!("skipping local-port test; cannot bind localhost: {error}");
                    return;
                }
            };
            let requested_port = listener
                .local_addr()
                .expect("listener should have a local address")
                .port();
            drop(listener);

            let manager = SidecarManager::new(requested_port);
            let selected_port = manager
                .select_start_port(manager.requested_port())
                .expect("an available loopback port should be selected");

            assert_eq!(manager.port(), selected_port);
            if selected_port == requested_port {
                preserved = true;
                break;
            }

            assert!(
                TcpListener::bind(("127.0.0.1", requested_port)).is_err(),
                "allocator replaced available requested port {requested_port} with {selected_port}"
            );
        }

        assert!(
            preserved,
            "requested port collided on all {ATTEMPTS} attempts"
        );
    }

    #[test]
    fn stats_probe_rejects_non_headroom_json_body() {
        let stats: HeadroomStats =
            serde_json::from_str(r#"{"status":"ok"}"#).expect("generic JSON should parse");
        let error = validate_headroom_stats(stats, DEFAULT_PORT)
            .expect_err("generic JSON should not pass headroom validation");

        assert!(error
            .to_string()
            .contains("does not look like a managed proxy"));
    }

    #[tokio::test]
    async fn refresh_and_live_stats_refuse_unmanaged_fake_listener_without_probing_it() {
        let Some(fake) = FakeStatsServer::start(None) else {
            return;
        };
        let manager = SidecarManager::new(fake.port);
        manager.set_status(SidecarStatus::Running {
            spawned_by_us: false,
            managed_by_halv: false,
            optimization_paused: false,
            version: Some("v-test".to_string()),
            pinned_version: pinned_headroom_version().to_string(),
            port: fake.port,
        });

        match manager.refresh_status().await {
            SidecarStatus::Error { message, port } => {
                assert_eq!(port, fake.port);
                assert!(message.contains("refusing to use an unmanaged listener"));
            }
            status => panic!("expected unmanaged refresh to fail closed, got {status:?}"),
        }
        let error = manager
            .live_stats(&HashMap::new())
            .await
            .expect_err("unmanaged fake listener must not supply stats");
        assert!(error.contains("refusing to use an unmanaged listener"));
        assert_eq!(fake.request_count(), 0);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn refresh_exited_managed_child_records_automatic_heal_attempt() {
        let root = temp_dir("auto-heal-exited-child");
        let app_data_dir = root.join("app-data");
        let managed_binary = app_data_dir.join("bin").join(binary_name());
        write_file(&managed_binary, "managed binary fixture");

        let manager = SidecarManager::new(DEFAULT_PORT);
        manager.set_app_data_dir(app_data_dir);
        manager.set_binary_path(Some(managed_binary));
        let mut child = Command::new("/bin/sh")
            .args(["-c", "exit 47"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("short-lived managed child should spawn");
        let exit = child.wait().expect("managed child should exit");
        assert_eq!(exit.code(), Some(47));
        manager.replace_child(Some(ManagedChild {
            process: child,
            stderr_drain: None,
            stderr_done: None,
        }));

        let op_guard = manager.inner.op_lock.lock().await;
        let status = manager.refresh_status_locked().await;

        assert!(matches!(status, SidecarStatus::Starting { .. }));
        assert_eq!(
            manager
                .inner
                .auto_heal_attempts
                .lock()
                .expect("auto-heal attempts lock should not be poisoned")
                .len(),
            1
        );
        assert!(manager.inner.auto_heal_in_flight.load(Ordering::SeqCst));

        manager.shutdown();
        assert!(!manager.inner.auto_heal_in_flight.load(Ordering::SeqCst));
        drop(op_guard);
        tokio::task::yield_now().await;
        cleanup_dir(&root);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn readiness_rejects_fake_stats_when_managed_child_exits() {
        let root = temp_dir("fake-listener-readiness");
        let marker = root.join("exit-child");
        let Some(fake) = FakeStatsServer::start(Some(marker.clone())) else {
            cleanup_dir(&root);
            return;
        };
        let manager = SidecarManager::new(fake.port);
        let child = Command::new("/bin/sh")
            .args([
                "-c",
                "while [ ! -f \"$1\" ]; do :; done; exit 98",
                "halv-readiness-test",
            ])
            .arg(&marker)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("short-lived managed child should spawn");
        manager.replace_child(Some(ManagedChild {
            process: child,
            stderr_drain: None,
            stderr_done: None,
        }));

        let error = manager
            .wait_for_ready(Duration::from_secs(3))
            .await
            .expect_err("fake /stats must not mask the managed child's exit");

        assert!(fake.request_count() >= 1);
        assert!(error.contains("before becoming ready on selected port"));
        assert!(error.contains("managed child exited"));
        cleanup_dir(&root);
    }
}
