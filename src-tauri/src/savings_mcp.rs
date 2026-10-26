use chrono::{Duration, Utc};
use futures_util::StreamExt;
use rusqlite::{params, Connection, OpenFlags, OptionalExtension, TransactionBehavior};
use serde::Serialize;
use serde_json::{json, Map, Value};
use std::{
    collections::BTreeSet,
    ffi::OsString,
    io::{self, BufRead, Write},
    path::{Path, PathBuf},
    sync::OnceLock,
};

use crate::{
    preferences::WorkerLanePreference,
    storage::{
        session_token::{self, SessionTokenVault},
        TaskRoute, TaskRouteAgent, TaskRouteReceipt, TaskRouteWorkflow,
    },
    task_hierarchy::Graph,
    task_routing::{
        self, Catalog, LadderTier, Lane, LaneState, LaneStates, RoutingInput, RoutingReason,
        RoutingResult, TierAgentAssignments, WorkerLane, WorkflowRole, WorkflowRoutingInput,
        DEFAULT_RESERVE_PERCENT,
    },
};

mod workflow_receipts;

const MCP_PROTOCOL_VERSION: &str = "2024-11-05";
const SAVINGS_RESOURCE_URI: &str = "savings://current";
const ROUTE_REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
const QUOTA_PROBE_ENVIRONMENT_VARIABLE: &str = "HALV_ROUTING_QUOTA_PROBE";
const SIGN_IN_MESSAGE: &str = "Sign in to your Halv account to route tasks.";
/// The reason and the provider of a route when the user has no Halv Cloud
/// access (ADR 0021).
const HALV_CLOUD_REQUIRED: &str = "halv_cloud_required";
const HALV_CLOUD_REQUIRED_MESSAGE: &str =
    "Task routing needs Halv Cloud. The coordinator does this task itself.";
/// The reason and the provider of a route when Halv has no valid session:
/// the user is signed out, the session expired, or the token is unreadable.
const SIGN_IN_REQUIRED: &str = "sign_in_required";
const SIGN_IN_REQUIRED_MESSAGE: &str =
    "Sign in to your Halv account to route tasks. The coordinator does this task itself.";
/// The reason and the provider of a route when the user did not allow task
/// briefs to leave the device (ADR 0023). Halv sent nothing.
const BRIEF_SHARING_OFF: &str = "brief_sharing_off";
const BRIEF_SHARING_OFF_MESSAGE: &str =
    "Task brief sharing is off in Halv settings, so Halv sent nothing. The coordinator does this task itself.";
const EXECUTE_IN_PARENT: &str = "execute_in_parent";
const STORAGE_NOT_READY_MESSAGE: &str = "Halv task routing storage is not ready. Restart Halv.";
const MAX_TASK_TITLE_CHARS: usize = 120;
const WORKFLOW_ROUTING_OFF_MESSAGE: &str =
    "Workflow routing is off in Halv settings. Run the workflow without routed options.";
const TASK_ROUTING_OFF_MESSAGE: &str =
    "Task routing is off for this session. Do the task yourself.";
const MAX_WORKFLOW_TASKS: usize = 64;
const MAX_WORKFLOW_BRIEF_CHARS: usize = 4_000;
const MAX_WORKFLOW_BATCH_BRIEF_CHARS: usize = 64_000;
const MAX_WORKFLOW_KEY_CHARS: usize = 48;
const MAX_WORKFLOW_NAME_CHARS: usize = 120;
const MAX_COORDINATOR_MODEL_CHARS: usize = 80;
const MAX_WORKFLOW_INSTANCES: i64 = 200;
const MAX_REPORT_RESULTS: usize = 128;
const MAX_REPORTED_AGENTS: i64 = 1_000;
/// JEV answers several briefs at once. The server has no rate limit on
/// `/v1/route`, and eight requests keep a 64-entry batch near ten seconds.
const WORKFLOW_DECISION_CONCURRENCY: usize = 8;

/// Why JEV gave no route decision.
#[derive(Debug, Clone, PartialEq, Eq)]
enum DecisionError {
    /// Halv has no valid session: the user is signed out, the token is
    /// unreadable, or the server refused the session with HTTP 401.
    SignInRequired,
    /// The server refused the account with HTTP 402 or 403, or the cached
    /// entitlements show no Halv Cloud access. Task routing is part of Halv
    /// Cloud (ADR 0021).
    CloudRequired,
    /// The user did not allow task briefs to leave the device (ADR 0023).
    BriefSharingOff,
    Failed(String),
}

impl DecisionError {
    fn message(&self) -> String {
        match self {
            Self::SignInRequired => SIGN_IN_REQUIRED_MESSAGE.to_string(),
            Self::CloudRequired => HALV_CLOUD_REQUIRED_MESSAGE.to_string(),
            Self::BriefSharingOff => BRIEF_SHARING_OFF_MESSAGE.to_string(),
            Self::Failed(message) => message.clone(),
        }
    }

    /// The route reason when the coordinator does the task itself.
    fn parent_reason(&self) -> Option<&'static str> {
        match self {
            Self::SignInRequired => Some(RoutingReason::SignInRequired.as_str()),
            Self::CloudRequired => Some(RoutingReason::CloudRequired.as_str()),
            Self::BriefSharingOff => Some(BRIEF_SHARING_OFF),
            Self::Failed(_) => None,
        }
    }
}

/// Halv Cloud access from the cached `/v1/me` entitlements. It follows
/// `cloudAccess` in `src/halvCloud.ts`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CachedCloudAccess {
    Granted,
    Denied,
    /// The app has no entitlements yet. The server decides.
    Unknown,
}

fn cached_cloud_access(entitlements_json: Option<&str>, plan: Option<&str>) -> CachedCloudAccess {
    let Some(entitlements) = entitlements_json
        .and_then(|value| serde_json::from_str::<crate::auth::Entitlements>(value).ok())
    else {
        return CachedCloudAccess::Unknown;
    };
    let payment_issue = entitlements
        .payment_issue
        .as_deref()
        .filter(|issue| !issue.trim().is_empty());
    // An unpaid subscription has no access, also when the server is late.
    if payment_issue == Some("unpaid") {
        return CachedCloudAccess::Denied;
    }
    if entitlements.can_use_app {
        return CachedCloudAccess::Granted;
    }
    // Legacy beta accounts keep Halv Cloud while billing is required.
    let plan = Some(entitlements.plan.as_str())
        .filter(|plan| !plan.is_empty())
        .or(plan);
    if plan == Some("beta_free") && payment_issue.is_none() {
        CachedCloudAccess::Granted
    } else {
        CachedCloudAccess::Denied
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SavingsMcpLaunchConfig {
    pub command: String,
    pub args: Vec<String>,
    /// Variables that the agent CLI must give to the server process.
    pub env: std::collections::BTreeMap<String, String>,
}

/// The variables that the keychain needs in the MCP server. On Linux the
/// keyring reads the Secret Service over the D-Bus session bus. Codex and
/// Claude Code can start an MCP server with a filtered environment, so the
/// launch configuration names these variables (issue #50).
const KEYCHAIN_ENVIRONMENT: &[&str] = if cfg!(target_os = "linux") {
    &["DBUS_SESSION_BUS_ADDRESS", "XDG_RUNTIME_DIR"]
} else {
    &[]
};

/// The named variables that have a value.
fn keychain_environment(
    names: &[&str],
    value_of: impl Fn(&str) -> Option<String>,
) -> std::collections::BTreeMap<String, String> {
    names
        .iter()
        .filter_map(|name| {
            value_of(name)
                .filter(|value| !value.is_empty())
                .map(|value| (name.to_string(), value))
        })
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SavingsMcpArgs {
    database_path: PathBuf,
    session_id: String,
    rtk_database_path: PathBuf,
    coordinator_model: Option<String>,
    conversation_id: Option<String>,
    worker_lane: WorkerLane,
    tier_agents: TierAgentAssignments,
    /// The routing tools are on (ADR 0023). Without them, the server lists
    /// only the savings tools, which send nothing.
    task_routing: bool,
    /// `route_tasks` routes Claude Code Workflow agents (ADR 0014).
    workflow_routing: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
struct SessionSavings {
    session_id: String,
    tokens_saved: u64,
    source: &'static str,
    known: bool,
    rtk_tokens_saved: u64,
    rtk_included_in_total: bool,
    rtk_scope: &'static str,
    pricing_source: Option<String>,
    /// Dollars saved at API prices (ADR 0015), with the routing low
    /// estimate of the session (ADR 0016).
    dollars_usd: f64,
    /// The routing part of `dollars_usd`.
    routing_usd: f64,
    /// True when `dollars_usd` holds a routing estimate.
    approximate: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
struct SavingsTotals {
    session_tokens_saved: u64,
    session_source: &'static str,
    session_known: bool,
    this_week_tokens_saved: u64,
    period: &'static str,
    rtk_tokens_saved: u64,
    rtk_included_in_total: bool,
    crux_tokens_saved: u64,
    crux_included_in_total: bool,
    pricing_source: Option<String>,
    session_dollars_usd: f64,
    session_approximate: bool,
    this_week_dollars_usd: f64,
    /// The routing part of `this_week_dollars_usd`.
    this_week_routing_usd: f64,
    this_week_approximate: bool,
}

/// The dollar text of the agent-facing tools: "$0.42 at API prices", with
/// "≈" when the amount holds a routing estimate.
fn dollars_text(usd: f64, approximate: bool) -> String {
    let amount = crate::tray::format_usd_compact(Some(usd));
    if approximate {
        format!("\u{2248} {amount} at API prices")
    } else {
        format!("{amount} at API prices")
    }
}

/// The routing totals of one scope. An older database without the routing
/// table gives none.
fn routing_totals_or_none(
    connection: &Connection,
    scope: crate::storage::routing_savings::RoutingScope<'_>,
) -> crate::savings::RoutingTotals {
    crate::storage::routing_savings::routing_totals(connection, scope).unwrap_or_default()
}

/// `task_routing` is the effective value of the launch (ADR 0023). Without
/// it, the server lists only the savings tools, which send nothing.
#[allow(clippy::too_many_arguments)]
pub(crate) fn launch_config(
    database_path: &Path,
    session_id: &str,
    coordinator_model: Option<&str>,
    conversation_id: Option<&str>,
    worker_lane: WorkerLanePreference,
    tier_agents: TierAgentAssignments,
    task_routing: bool,
    workflow_routing: bool,
) -> Result<SavingsMcpLaunchConfig, String> {
    let executable = std::env::current_exe()
        .map_err(|_| "failed to locate the Halv executable for savings MCP".to_string())?;
    let mut config = launch_config_with_executable(
        &executable,
        database_path,
        session_id,
        // The session-scoped ledger keeps the CLI-side report consistent
        // with the sidebar meter.
        &crate::exec::rtk_session_database_path(session_id),
        coordinator_model,
        conversation_id,
        worker_lane,
        tier_agents,
        task_routing,
        workflow_routing,
    )?;
    config.env = keychain_environment(KEYCHAIN_ENVIRONMENT, |name| std::env::var(name).ok());
    Ok(config)
}

#[allow(clippy::too_many_arguments)]
fn launch_config_with_executable(
    executable: &Path,
    database_path: &Path,
    session_id: &str,
    rtk_database_path: &Path,
    coordinator_model: Option<&str>,
    conversation_id: Option<&str>,
    worker_lane: WorkerLanePreference,
    tier_agents: TierAgentAssignments,
    task_routing: bool,
    workflow_routing: bool,
) -> Result<SavingsMcpLaunchConfig, String> {
    let path_text = |path: &Path, label: &str| {
        path.to_str()
            .map(str::to_owned)
            .ok_or_else(|| format!("{label} must use Unicode for savings MCP"))
    };
    if session_id.trim().is_empty() {
        return Err("the savings MCP session ID cannot be empty".to_string());
    }
    let mut args = vec![
        "--mcp-savings".to_string(),
        "--database".to_string(),
        path_text(database_path, "the Halv database path")?,
        "--session".to_string(),
        session_id.to_string(),
        "--rtk-database".to_string(),
        path_text(rtk_database_path, "the RTK database path")?,
    ];
    if let Some(model) = coordinator_model
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        args.push("--coordinator-model".to_string());
        args.push(model.to_string());
    }
    if let Some(conversation_id) = conversation_id
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        args.push("--conversation".to_string());
        args.push(conversation_id.to_string());
    }
    if worker_lane != WorkerLanePreference::Auto {
        args.push("--worker-lane".to_string());
        args.push(worker_lane.as_str().to_string());
    }
    args.push("--tier-agents".to_string());
    args.push(
        serde_json::to_string(&tier_agents)
            .map_err(|error| format!("failed to encode routing tier agents: {error}"))?,
    );
    // Only "off" goes on the command line, so the default argument set
    // stays the same as in older apps. Workflow routing sends briefs too,
    // so it is off without task routing (ADR 0023).
    if !task_routing {
        args.push("--task-routing".to_string());
        args.push("off".to_string());
    }
    if !(workflow_routing && task_routing) {
        args.push("--workflow-routing".to_string());
        args.push("off".to_string());
    }
    Ok(SavingsMcpLaunchConfig {
        command: path_text(executable, "the Halv executable path")?,
        args,
        env: Default::default(),
    })
}

pub fn run_from_args(args: impl IntoIterator<Item = OsString>) -> i32 {
    let args = match parse_args(args) {
        Ok(args) => args,
        Err(message) => {
            eprintln!("halv-savings MCP configuration error: {message}");
            return 2;
        }
    };
    let reader = match SavingsReader::open(args) {
        Ok(reader) => reader,
        Err(()) => {
            eprintln!("halv-savings MCP could not open its read-only savings data");
            return 1;
        }
    };
    match serve(io::stdin().lock(), io::stdout().lock(), &reader) {
        Ok(()) => 0,
        Err(()) => {
            eprintln!("halv-savings MCP stopped after a protocol I/O error");
            1
        }
    }
}

fn parse_args(args: impl IntoIterator<Item = OsString>) -> Result<SavingsMcpArgs, String> {
    let mut database_path = None;
    let mut session_id = None;
    let mut rtk_database_path = None;
    let mut coordinator_model = None;
    let mut conversation_id = None;
    let mut worker_lane = None;
    let mut tier_agents = None;
    let mut task_routing = None;
    let mut workflow_routing = None;
    let mut args = args.into_iter();
    while let Some(flag) = args.next() {
        let slot = match flag.to_str() {
            Some("--database") => &mut database_path,
            Some("--session") => &mut session_id,
            Some("--rtk-database") => &mut rtk_database_path,
            Some("--coordinator-model") => &mut coordinator_model,
            Some("--conversation") => &mut conversation_id,
            Some("--worker-lane") => &mut worker_lane,
            Some("--tier-agents") => &mut tier_agents,
            Some("--task-routing") => &mut task_routing,
            Some("--workflow-routing") => &mut workflow_routing,
            _ => return Err("an unsupported argument was provided".to_string()),
        };
        let value = args
            .next()
            .ok_or_else(|| "a required argument value is missing".to_string())?;
        if slot.replace(value).is_some() {
            return Err("a savings MCP argument was provided more than once".to_string());
        }
    }
    let database_path = database_path
        .map(PathBuf::from)
        .ok_or_else(|| "the Halv database path is required".to_string())?;
    let session_id = session_id
        .and_then(|value| value.into_string().ok())
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| "the savings MCP session ID is required".to_string())?;
    let rtk_database_path = rtk_database_path
        .map(PathBuf::from)
        .ok_or_else(|| "the RTK database path is required".to_string())?;
    let optional_text = |value: Option<OsString>| {
        value
            .and_then(|value| value.into_string().ok())
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
    };
    let worker_lane = match optional_text(worker_lane) {
        Some(value) => WorkerLane::parse(value.as_str())
            .ok_or_else(|| "the worker lane must be auto, claude, or codex".to_string())?,
        None => WorkerLane::Auto,
    };
    let tier_agents = optional_text(tier_agents)
        .map(|value| {
            serde_json::from_str::<TierAgentAssignments>(&value)
                .map_err(|_| "the routing tier agents must be valid JSON".to_string())
        })
        .transpose()?
        .unwrap_or_default();
    tier_agents.validate_default_catalog()?;
    // Older apps do not send the flags. Routing is on for them.
    let task_routing = match optional_text(task_routing).as_deref() {
        None | Some("on") => true,
        Some("off") => false,
        Some(_) => return Err("the task routing flag must be on or off".to_string()),
    };
    let workflow_routing = match optional_text(workflow_routing).as_deref() {
        None | Some("on") => task_routing,
        Some("off") => false,
        Some(_) => return Err("the workflow routing flag must be on or off".to_string()),
    };
    Ok(SavingsMcpArgs {
        database_path,
        session_id,
        rtk_database_path,
        coordinator_model: optional_text(coordinator_model),
        conversation_id: optional_text(conversation_id),
        worker_lane,
        tier_agents,
        task_routing,
        workflow_routing,
    })
}

struct SavingsReader {
    connection: Connection,
    session_id: String,
    rtk_database_path: PathBuf,
    database_path: PathBuf,
    coordinator_model: Option<String>,
    conversation_id: Option<String>,
    worker_lane: WorkerLane,
    tier_agents: TierAgentAssignments,
    task_routing: bool,
    workflow_routing: bool,
    /// The Claude Code projects folder that holds the transcripts.
    claude_projects_root: Option<PathBuf>,
    runtime: OnceLock<Option<tokio::runtime::Runtime>>,
    /// The keychain item of the Halv session token (issue #50).
    session_token: SessionTokenVault,
    /// The app preferences file. The server reads the task brief answer
    /// from it before each brief, so a "no" in Settings stops a running
    /// session at once (ADR 0023).
    preferences_path: PathBuf,
}

impl SavingsReader {
    fn open(args: SavingsMcpArgs) -> Result<Self, ()> {
        // The app database uses WAL. A read-only open works because the
        // same user owns the -wal and -shm files.
        let connection = Connection::open_with_flags(
            &args.database_path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(|_| ())?;
        connection
            .busy_timeout(std::time::Duration::from_secs(2))
            .map_err(|_| ())?;
        // The database and the preferences file share the app data folder.
        let preferences_path = crate::preferences::preferences_path(
            args.database_path.parent().unwrap_or_else(|| Path::new("")),
        );
        Ok(Self {
            preferences_path,
            connection,
            session_id: args.session_id,
            rtk_database_path: args.rtk_database_path,
            database_path: args.database_path,
            coordinator_model: args.coordinator_model,
            conversation_id: args.conversation_id,
            worker_lane: args.worker_lane,
            tier_agents: args.tier_agents,
            task_routing: args.task_routing,
            workflow_routing: args.workflow_routing,
            claude_projects_root: workflow_receipts::claude_projects_root(),
            runtime: OnceLock::new(),
            session_token: SessionTokenVault::new(session_token::default_store()),
        })
    }

    fn session_savings(&self) -> Result<SessionSavings, ()> {
        let sql = crate::savings::mcp_session_savings_sql();
        let (tokens_saved, samples, pricing_source, context_usd) = self
            .connection
            .query_row(&sql, params![&self.session_id], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, f64>(3)?,
                ))
            })
            .map_err(|_| ())?;
        let routing = routing_totals_or_none(
            &self.connection,
            crate::storage::routing_savings::RoutingScope::Session(&self.session_id),
        );
        Ok(SessionSavings {
            session_id: self.session_id.clone(),
            tokens_saved: nonnegative_u64(tokens_saved),
            source: if samples > 0 { "measured" } else { "estimated" },
            known: samples > 0,
            rtk_tokens_saved: crate::exec::rtk_tokens_saved_at(&self.rtk_database_path)
                .unwrap_or_default(),
            rtk_included_in_total: true,
            rtk_scope: "session",
            pricing_source,
            dollars_usd: context_usd + routing.low_usd,
            routing_usd: routing.low_usd,
            approximate: routing.chains > 0,
        })
    }

    fn savings_totals(&self) -> Result<SavingsTotals, ()> {
        let session = self.session_savings()?;
        let cutoff = trailing_week_cutoff();
        let sql = crate::savings::mcp_week_savings_sql();
        let (tokens_saved, crux_tokens_saved, pricing_source, week_usd) = self
            .connection
            .query_row(
                &sql,
                params![crate::storage::savings_usd_for_tokens(1), cutoff],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, f64>(3)?,
                    ))
                },
            )
            .map_err(|_| ())?;
        let week_routing = routing_totals_or_none(
            &self.connection,
            crate::storage::routing_savings::RoutingScope::Since(&cutoff),
        );
        Ok(SavingsTotals {
            session_tokens_saved: session.tokens_saved,
            session_source: session.source,
            session_known: session.known,
            this_week_tokens_saved: nonnegative_u64(tokens_saved),
            period: "rolling_7x24h",
            rtk_tokens_saved: session.rtk_tokens_saved,
            rtk_included_in_total: true,
            crux_tokens_saved: nonnegative_u64(crux_tokens_saved),
            crux_included_in_total: true,
            pricing_source,
            session_dollars_usd: session.dollars_usd,
            session_approximate: session.approximate,
            this_week_dollars_usd: week_usd,
            this_week_routing_usd: week_routing.low_usd,
            this_week_approximate: week_routing.chains > 0,
        })
    }
}

/// The routing half of the MCP server. It writes through its own connection
/// because the savings reader stays read-only.
impl SavingsReader {
    fn runtime(&self) -> Result<&tokio::runtime::Runtime, String> {
        self.runtime
            .get_or_init(|| {
                tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .ok()
            })
            .as_ref()
            .ok_or_else(|| "Halv could not start the task routing runtime.".to_string())
    }

    /// Opens the write connection the routing tools use for `task_routes`.
    fn routing_connection(&self) -> Result<Connection, String> {
        let connection =
            Connection::open(&self.database_path).map_err(|_| STORAGE_NOT_READY_MESSAGE)?;
        connection
            .busy_timeout(std::time::Duration::from_secs(2))
            .map_err(|_| STORAGE_NOT_READY_MESSAGE)?;
        let table_exists: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'task_routes'",
                [],
                |row| row.get(0),
            )
            .map_err(|_| STORAGE_NOT_READY_MESSAGE)?;
        if table_exists == 0 {
            return Err(STORAGE_NOT_READY_MESSAGE.to_string());
        }
        Ok(connection)
    }

    /// The server URL and the session token. The token is in the keychain
    /// item that `token_account` names, and the item also gives the URL. The
    /// URL column does not count for such a token: any process of the same
    /// user can change it, also in a copy of the database for this server
    /// (issue #50). The column `token` holds the token only when the app
    /// could not use the keychain. Then the URL column counts, because such a
    /// process can also read the token in that column.
    fn halv_session(&self) -> Result<(String, String), String> {
        let (server_url, token, token_account) = self
            .connection
            .query_row(
                "SELECT server_url, token, token_account FROM auth_session WHERE id = 1",
                [],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, Option<String>>(2)?,
                    ))
                },
            )
            .map_err(|_| SIGN_IN_MESSAGE.to_string())?;
        if let Some(token) = token.filter(|token| !token.trim().is_empty()) {
            let server_url = server_url.trim().trim_end_matches('/').to_string();
            if server_url.is_empty() {
                return Err(SIGN_IN_MESSAGE.to_string());
            }
            return Ok((server_url, token));
        }
        let bound = token_account
            .as_deref()
            .and_then(|account| self.session_token.bound_token(account))
            .ok_or_else(|| SIGN_IN_MESSAGE.to_string())?;
        Ok((bound.server_url, bound.token))
    }

    /// The lane and the cap of the coordinator that launched this server.
    fn coordinator(&self) -> (Lane, LadderTier) {
        match self.coordinator_model.as_deref() {
            Some(model) => {
                let lane = task_routing::lane_for_model(model);
                (lane, task_routing::tier_for_model(lane, model))
            }
            None => (Lane::Claude, LadderTier::Strong),
        }
    }

    fn lane_states(
        &self,
        coordinator_lane: Lane,
        worker_lane: WorkerLane,
        tier_agents: &TierAgentAssignments,
    ) -> LaneStates {
        let (claude_remaining, codex_remaining) = self.lane_quotas();
        let pinned = worker_lane.fixed();
        let lane_state = |lane: Lane, remaining: Option<f64>| LaneState {
            cap: LadderTier::Strong,
            available: remaining.is_some()
                || lane == coordinator_lane
                || pinned == Some(lane)
                || tier_agents.uses_lane(lane),
            remaining_percent: remaining,
            reserve_percent: DEFAULT_RESERVE_PERCENT,
        };
        let mut states = LaneStates {
            claude: lane_state(Lane::Claude, claude_remaining),
            codex: lane_state(Lane::Codex, codex_remaining),
        };
        let (_, cap) = self.coordinator();
        match coordinator_lane {
            Lane::Claude => states.claude.cap = cap,
            Lane::Codex => states.codex.cap = cap,
        }
        states
    }

    fn routing_catalog(&self) -> Catalog {
        load_routing_catalog(&self.database_path)
    }

    /// Reads both lane quotas, best effort. A probe failure counts as unknown.
    fn lane_quotas(&self) -> (Option<f64>, Option<f64>) {
        if std::env::var(QUOTA_PROBE_ENVIRONMENT_VARIABLE)
            .map(|value| value.trim().eq_ignore_ascii_case("off"))
            .unwrap_or(false)
        {
            return (None, None);
        }
        let Ok(runtime) = self.runtime() else {
            return (None, None);
        };
        runtime.block_on(async {
            (
                crate::agent_sessions::claude_lane_remaining_percent().await,
                crate::agent_sessions::codex_lane_remaining_percent().await,
            )
        })
    }

    /// The task brief answer in the preferences file. A missing or damaged
    /// file reads `Unset`.
    fn brief_sharing(&self) -> crate::preferences::BriefSharing {
        crate::preferences::brief_sharing_in_file(&self.preferences_path)
    }

    /// Halv Cloud access from the entitlements that the app cached.
    fn cached_cloud_access(&self) -> CachedCloudAccess {
        self.connection
            .query_row(
                "SELECT entitlements_json, plan FROM auth_session WHERE id = 1",
                [],
                |row| {
                    Ok((
                        row.get::<_, Option<String>>(0)?,
                        row.get::<_, Option<String>>(1)?,
                    ))
                },
            )
            .map(|(entitlements, plan)| {
                cached_cloud_access(entitlements.as_deref(), plan.as_deref())
            })
            .unwrap_or(CachedCloudAccess::Unknown)
    }

    /// The server URL and the token for a brief, or the reason that no
    /// brief may leave the device. It reads only local data, so a refusal
    /// sends nothing (ADR 0023):
    /// - no valid session: the user must sign in first;
    /// - cached entitlements without Halv Cloud: the server would refuse;
    /// - no "yes" to the task brief question: the user did not allow it.
    ///
    /// Unknown entitlements do not refuse. The server decides for them.
    fn brief_session(&self) -> Result<(String, String), DecisionError> {
        let session = self
            .halv_session()
            .map_err(|_| DecisionError::SignInRequired)?;
        if self.cached_cloud_access() == CachedCloudAccess::Denied {
            return Err(DecisionError::CloudRequired);
        }
        if self.brief_sharing() != crate::preferences::BriefSharing::Allowed {
            return Err(DecisionError::BriefSharingOff);
        }
        Ok(session)
    }

    /// Without a valid session the user must sign in first.
    fn fetch_decision(&self, brief: &str) -> Result<crate::auth::RouteDecision, DecisionError> {
        let (server_url, token) = self.brief_session()?;
        let runtime = self.runtime().map_err(DecisionError::Failed)?;
        runtime.block_on(request_decision(&server_url, &token, brief))
    }

    /// Fetches one decision for each brief, eight at a time, in input order.
    /// A failure of one brief does not stop the others.
    fn fetch_decisions(
        &self,
        briefs: &[&str],
    ) -> Vec<Result<crate::auth::RouteDecision, DecisionError>> {
        let session = self.brief_session().and_then(|session| {
            self.runtime()
                .map(|runtime| (session, runtime))
                .map_err(DecisionError::Failed)
        });
        let ((server_url, token), runtime) = match session {
            Ok(session) => session,
            Err(error) => return briefs.iter().map(|_| Err(error.clone())).collect(),
        };
        runtime.block_on(
            futures_util::stream::iter(
                briefs
                    .iter()
                    .map(|brief| request_decision(&server_url, &token, brief)),
            )
            .buffered(WORKFLOW_DECISION_CONCURRENCY)
            .collect::<Vec<_>>(),
        )
    }

    /// The Claude Code session id of this Halv session, when Halv knows it.
    fn cli_session_id(&self) -> Option<String> {
        self.connection
            .query_row(
                "SELECT cli_session_id FROM agent_sessions WHERE id = ?1",
                params![self.session_id],
                |row| row.get::<_, Option<String>>(0),
            )
            .ok()
            .flatten()
    }

    /// The model and the effort that a worker inherits without JEV.
    ///
    /// The order is: the last model in the coordinator transcript, the model
    /// that the coordinator names in the call, the launch hint, nothing.
    fn route_default(&self, coordinator_model: Option<&str>) -> RouteDefault {
        let is_claude = |model: &str| task_routing::lane_for_model(model) == Lane::Claude;
        let observed = self
            .claude_projects_root
            .as_deref()
            .and_then(|root| workflow_receipts::observe_coordinator(root, &self.cli_session_id()?));
        if let Some((model, effort)) = observed.filter(|(model, _)| is_claude(model)) {
            return RouteDefault {
                model: Some(model),
                effort,
                source: "observed",
            };
        }
        if let Some(model) = coordinator_model
            .map(str::trim)
            .filter(|model| !model.is_empty() && is_claude(model))
        {
            return RouteDefault {
                model: Some(model.to_string()),
                effort: None,
                source: "coordinator",
            };
        }
        match self.coordinator_model.as_deref() {
            Some(model) => RouteDefault {
                model: Some(model.to_string()),
                effort: None,
                source: "hint",
            },
            None => RouteDefault {
                model: None,
                effort: None,
                source: "none",
            },
        }
    }

    /// Resolves the workflow and fallback lane.
    /// A saved tier assignment can override this lane inside the router.
    fn effective_worker_lane(&self, requested: Option<&str>) -> WorkerLane {
        if self.worker_lane != WorkerLane::Auto {
            return self.worker_lane;
        }
        requested
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .and_then(WorkerLane::parse)
            .unwrap_or(WorkerLane::Auto)
    }

    fn route_task(
        &self,
        brief: &str,
        title: Option<&str>,
        worker_lane: Option<&str>,
    ) -> Result<Value, String> {
        let brief = brief.trim();
        if brief.is_empty() {
            return Err("The task brief is empty.".to_string());
        }
        let connection = self.routing_connection()?;
        let worker_lane = self.effective_worker_lane(worker_lane);
        let (route, decision) = self.compute_task_route(brief, title, worker_lane)?;
        let route = self.route_default(None).apply(route);
        crate::storage::insert_task_route_row(&connection, &route)
            .map_err(|_| STORAGE_NOT_READY_MESSAGE.to_string())?;
        Ok(route_tool_result(&route, &decision, None))
    }

    fn compute_task_route(
        &self,
        brief: &str,
        title: Option<&str>,
        worker_lane: WorkerLane,
    ) -> Result<(TaskRoute, crate::auth::RouteDecision), String> {
        let decision = match self.fetch_decision(brief) {
            Ok(decision) => decision,
            Err(DecisionError::Failed(message)) => return Err(message),
            Err(error) => {
                let reason = error.parent_reason().unwrap_or(HALV_CLOUD_REQUIRED);
                return Ok(self.cloud_required_route(brief, title, worker_lane, reason));
            }
        };
        let (coordinator_lane, _) = self.coordinator();
        let tier_agents = self.tier_agents.clone();
        let input = RoutingInput {
            decision: &decision,
            coordinator_lane,
            lanes: self.lane_states(coordinator_lane, worker_lane, &tier_agents),
            catalog: Some(self.routing_catalog()),
            worker_lane,
            tier_agents: Some(tier_agents),
        };
        let result = task_routing::collapse_coordinator_model(
            task_routing::choose_route(&input),
            coordinator_lane,
            self.coordinator_model.as_deref(),
        );
        let route = self.new_task_route(&decision, &result, brief, title, None, worker_lane);
        Ok((route, decision))
    }

    /// The route without Halv Cloud access, without a valid session, or
    /// without a "yes" to the task brief question (ADR 0023): the
    /// coordinator does the task itself at its own tier. The answer is a
    /// normal route, never an error, so the agent keeps working (ADR 0021).
    /// The route stays on this device.
    fn cloud_required_route(
        &self,
        brief: &str,
        title: Option<&str>,
        worker_lane: WorkerLane,
        reason: &'static str,
    ) -> (TaskRoute, crate::auth::RouteDecision) {
        let decision = parent_route_decision(reason);
        let (coordinator_lane, _) = self.coordinator();
        let result = RoutingResult {
            tier: LadderTier::Max,
            // A hierarchy route must stay in the lane of its parent.
            lane: worker_lane.fixed().unwrap_or(coordinator_lane),
            model: None,
            effort: None,
            reasons: Vec::new(),
        };
        let route = self.new_task_route(&decision, &result, brief, title, None, worker_lane);
        // The reason names the refusal. `brief_sharing_off` is no routing
        // rule, so it is not a `RoutingReason`.
        let route = TaskRoute {
            reasons: vec![reason.to_string()],
            ..route
        };
        (route, decision)
    }

    fn report_task_result(&self, task_id: &str, passed: bool) -> Result<Value, String> {
        let task_id = task_id.trim();
        if task_id.is_empty() {
            return Err("The task id is empty.".to_string());
        }
        let connection = self.routing_connection()?;
        let previous = crate::storage::task_route_row(&connection, task_id)
            .map_err(|_| STORAGE_NOT_READY_MESSAGE.to_string())?
            .ok_or_else(|| "Halv does not know this task id.".to_string())?;
        if passed {
            crate::storage::update_task_route_status_row(&connection, task_id, "passed")
                .map_err(|_| STORAGE_NOT_READY_MESSAGE.to_string())?;
            return Ok(tool_text_result(
                serde_json::to_string(&json!({ "taskId": task_id, "status": "passed" }))
                    .unwrap_or_default(),
            ));
        }
        crate::storage::update_task_route_status_row(&connection, task_id, "failed")
            .map_err(|_| STORAGE_NOT_READY_MESSAGE.to_string())?;

        let (route, decision) = self.compute_escalated_route(&previous);
        crate::storage::insert_task_route_row(&connection, &route)
            .map_err(|_| STORAGE_NOT_READY_MESSAGE.to_string())?;
        Ok(route_tool_result(
            &route,
            &decision,
            Some(previous.id.as_str()),
        ))
    }

    fn compute_escalated_route(
        &self,
        previous: &TaskRoute,
    ) -> (TaskRoute, crate::auth::RouteDecision) {
        let decision = decision_from_route(previous);
        let (coordinator_lane, _) = self.coordinator();
        let worker_lane =
            WorkerLane::parse(previous.worker_lane.as_str()).unwrap_or(WorkerLane::Auto);
        let catalog = self.routing_catalog();
        let tier_agents = self.tier_agents.clone();
        let input = RoutingInput {
            decision: &decision,
            coordinator_lane,
            lanes: self.lane_states(coordinator_lane, worker_lane, &tier_agents),
            catalog: Some(catalog),
            worker_lane,
            tier_agents: Some(tier_agents),
        };
        let current = result_from_route(previous);
        let next = task_routing::collapse_coordinator_model(
            task_routing::escalate(&current, &input),
            coordinator_lane,
            self.coordinator_model.as_deref(),
        );
        let route = self.new_task_route(
            &decision,
            &next,
            "",
            Some(previous.title.as_str()),
            Some(previous.id.as_str()),
            worker_lane,
        );
        let route = TaskRoute {
            brief_chars: previous.brief_chars,
            default_model: previous.default_model.clone(),
            default_effort: previous.default_effort.clone(),
            default_source: previous.default_source.clone(),
            ..route
        };
        (route, decision)
    }

    fn hierarchy_graph(&self, connection: &Connection) -> Result<Graph, String> {
        let stored: Option<String> = connection
            .query_row(
                "SELECT graph_json FROM task_hierarchies WHERE session_id = ?1",
                params![self.session_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(|_| STORAGE_NOT_READY_MESSAGE.to_string())?;
        stored
            .map(|json| {
                let graph: Graph = serde_json::from_str(&json)
                    .map_err(|_| "The saved JEV hierarchy is invalid.".to_string())?;
                graph.validate()?;
                Ok(graph)
            })
            .unwrap_or_else(|| Ok(Graph::default()))
    }

    fn save_hierarchy(&self, connection: &Connection, graph: &Graph) -> Result<(), String> {
        let json = serde_json::to_string(graph).map_err(|error| error.to_string())?;
        connection.execute(
            "INSERT INTO task_hierarchies (session_id, graph_json, updated_at) VALUES (?1, ?2, ?3)
             ON CONFLICT(session_id) DO UPDATE SET graph_json = excluded.graph_json, updated_at = excluded.updated_at",
            params![self.session_id, json, Utc::now().to_rfc3339()],
        ).map_err(|_| STORAGE_NOT_READY_MESSAGE.to_string())?;
        Ok(())
    }

    fn hierarchy_route(&self, args: &Value) -> Result<Value, String> {
        let requested_lane = args.get("workerLane").and_then(Value::as_str);
        if self.effective_worker_lane(requested_lane) != WorkerLane::Fixed(Lane::Codex) {
            return Err("The investigation/L1/L2 hierarchy requires the Codex worker lane.".into());
        }
        let mut connection = self.routing_connection()?;
        let prepared = self.hierarchy_graph(&connection)?.prepare(args)?;
        let brief = prepared.effective_brief.as_str();
        if brief.chars().count() > 16_000 {
            return Err(
                "The task brief and investigation evidence exceed 16000 characters.".into(),
            );
        }
        let title = args.get("title").and_then(Value::as_str);
        let (route, decision) = if prepared.stage == "investigation" {
            let decision = crate::auth::RouteDecision {
                schema: crate::task_hierarchy::VERSION.into(),
                provider: "jev-role-policy".into(),
                model: "investigation-role".into(),
                complexity: crate::auth::RouteChoice {
                    choice: "standard".into(),
                    confidence: 1.0,
                    probabilities: std::collections::BTreeMap::new(),
                },
                needs_reasoning: 0.0,
                risky: 0.0,
                usage: crate::auth::RouteUsage {
                    input_tokens: None,
                    output_tokens: None,
                },
                latency_ms: 0,
            };
            let investigator = self.routing_catalog().codex.cheapest;
            if investigator.effort.as_deref() != Some("medium") {
                return Err(
                    "The available Luna model does not support the investigation's medium effort."
                        .into(),
                );
            }
            let result = RoutingResult {
                tier: LadderTier::Cheapest,
                lane: Lane::Codex,
                model: Some(investigator.model),
                effort: Some("medium".into()),
                reasons: Vec::new(),
            };
            let title = format!("Investigation: {}", task_title(brief, title));
            (
                self.new_task_route(
                    &decision,
                    &result,
                    brief,
                    Some(&title),
                    None,
                    WorkerLane::Fixed(Lane::Codex),
                ),
                decision,
            )
        } else {
            self.compute_task_route(brief, title, WorkerLane::Fixed(Lane::Codex))?
        };
        let route = self.route_default(None).apply(route);
        // The classifier runs before the transaction. Recheck the graph before storing its route.
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|_| STORAGE_NOT_READY_MESSAGE.to_string())?;
        let mut graph = self.hierarchy_graph(&transaction)?;
        let prepared = graph.prepare(args)?;
        let payload = route_tool_result(&route, &decision, None)["structuredContent"].clone();
        let result = graph.accept(&prepared, payload)?;
        crate::storage::insert_task_route_row(&transaction, &route)
            .map_err(|_| STORAGE_NOT_READY_MESSAGE.to_string())?;
        self.save_hierarchy(&transaction, &graph)?;
        transaction
            .commit()
            .map_err(|_| STORAGE_NOT_READY_MESSAGE.to_string())?;
        Ok(hierarchy_tool_result(result))
    }

    fn report_routed_task(&self, args: &Value) -> Result<Value, String> {
        let task_id = args
            .get("taskId")
            .and_then(Value::as_str)
            .filter(|id| !id.trim().is_empty())
            .ok_or("A taskId is required.")?;
        let passed = args
            .get("passed")
            .and_then(Value::as_bool)
            .ok_or("A boolean passed result is required.")?;
        let mut connection = self.routing_connection()?;
        let graph = self.hierarchy_graph(&connection)?;
        let previous = crate::storage::task_route_row(&connection, task_id)
            .map_err(|_| STORAGE_NOT_READY_MESSAGE.to_string())?
            .ok_or("Halv does not know this task id.")?;
        if previous.session_id != self.session_id {
            return Err("The task belongs to a different routing session.".into());
        }
        if !graph.routes.contains_key(task_id) {
            return self.report_task_result(task_id, passed);
        }
        let prepared = graph.prepare_report(args)?;
        let next = if !passed && prepared.stage != "investigation" && !prepared.already_reported {
            Some(self.compute_escalated_route(&previous))
        } else {
            None
        };
        let upstream = next.as_ref().map(|(route, decision)| {
            route_tool_result(route, decision, Some(task_id))["structuredContent"].clone()
        });
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|_| STORAGE_NOT_READY_MESSAGE.to_string())?;
        let mut graph = self.hierarchy_graph(&transaction)?;
        let prepared = graph.prepare_report(args)?;
        let upstream = if prepared.stage == "investigation" {
            None
        } else {
            Some(upstream.unwrap_or_else(
                || json!({"taskId":task_id,"status":if passed {"passed"} else {"failed"}}),
            ))
        };
        let result = graph.accept_report(&prepared, upstream)?;
        crate::storage::update_task_route_status_row(
            &transaction,
            task_id,
            if passed { "passed" } else { "failed" },
        )
        .map_err(|_| STORAGE_NOT_READY_MESSAGE.to_string())?;
        if !prepared.already_reported {
            if let Some((route, _)) = next {
                crate::storage::insert_task_route_row(&transaction, &route)
                    .map_err(|_| STORAGE_NOT_READY_MESSAGE.to_string())?;
            }
        }
        self.save_hierarchy(&transaction, &graph)?;
        transaction
            .commit()
            .map_err(|_| STORAGE_NOT_READY_MESSAGE.to_string())?;
        Ok(hierarchy_tool_result(result))
    }

    fn route_request(&self, args: &Value) -> Result<Value, String> {
        if args.get("stage").is_some() {
            return self.hierarchy_route(args);
        }
        if ["parentTaskId", "files", "checks"]
            .iter()
            .any(|key| args.get(key).is_some())
        {
            return Err("Hierarchy routes require a stage.".into());
        }
        let requested_lane = args.get("workerLane").and_then(Value::as_str);
        if self.coordinator_model.is_some()
            && self.effective_worker_lane(requested_lane) == WorkerLane::Fixed(Lane::Codex)
        {
            return Err("Codex routing requires stage=investigation, l1, or l2.".into());
        }
        let connection = self.routing_connection()?;
        if self.hierarchy_graph(&connection)?.has_active_root() {
            return Err(
                "An active hierarchy requires investigation, l1, or l2 stage routing.".into(),
            );
        }
        self.route_task(
            args.get("brief")
                .and_then(Value::as_str)
                .unwrap_or_default(),
            args.get("title").and_then(Value::as_str),
            args.get("workerLane").and_then(Value::as_str),
        )
    }

    /// Routes every agent stage of one Workflow script (ADR 0014). A JEV
    /// failure gives an inherit route for that stage, so the workflow never
    /// waits for JEV.
    fn route_tasks(&self, args: &Value) -> Result<Value, String> {
        if !self.workflow_routing {
            return Err(WORKFLOW_ROUTING_OFF_MESSAGE.to_string());
        }
        let request = WorkflowRequest::parse(args)?;
        let mut connection = self.routing_connection()?;
        require_workflow_columns(&connection)?;
        let default = self.route_default(request.coordinator_model.as_deref());
        let coordinator_cap = default
            .model
            .as_deref()
            .filter(|model| task_routing::lane_for_model(model) == Lane::Claude)
            .map_or(LadderTier::Strong, |model| {
                task_routing::tier_for_model(Lane::Claude, model)
            });
        let catalog = self.routing_catalog();
        let default_effort = claude_cheapest_default_effort(&self.database_path);
        let briefs = request
            .tasks
            .iter()
            .map(|task| task.brief.as_str())
            .collect::<Vec<_>>();
        let decisions = self.fetch_decisions(&briefs);
        let batch_id = crate::storage::new_uuid_v4();
        let mut planned = Vec::new();
        let mut fallback = Map::new();
        let mut parent_reason: Option<&'static str> = None;
        for (task, decision) in request.tasks.iter().zip(decisions) {
            let (decision, fell_back) = match decision {
                Ok(decision) => (decision, false),
                Err(error) => {
                    parent_reason = parent_reason.or(error.parent_reason());
                    fallback.insert(task.key.clone(), Value::String(error.message()));
                    (fallback_decision(), true)
                }
            };
            let input = WorkflowRoutingInput {
                decision: &decision,
                role: task.role,
                coordinator_cap,
                catalog: Some(catalog.clone()),
                worker_lane: self.worker_lane,
                tier_agents: Some(self.tier_agents.clone()),
                default_effort: default_effort.clone(),
            };
            let result = if fell_back {
                task_routing::workflow_fallback_route()
            } else {
                task_routing::choose_workflow_route(&input)
            };
            let retry = task_routing::workflow_escalation(&result, &input);
            let route = TaskRoute {
                workflow: Some(TaskRouteWorkflow {
                    batch_id: batch_id.clone(),
                    name: request.workflow.clone(),
                    key: task.key.clone(),
                    role: task.role.as_str().to_string(),
                    instances: task.instances,
                }),
                ..default.apply(self.new_task_route(
                    &decision,
                    &result,
                    &task.brief,
                    None,
                    None,
                    WorkerLane::Fixed(Lane::Claude),
                ))
            };
            planned.push((route, result, retry));
        }
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|_| STORAGE_NOT_READY_MESSAGE.to_string())?;
        for (route, _, _) in &planned {
            crate::storage::insert_task_route_row(&transaction, route)
                .map_err(|_| STORAGE_NOT_READY_MESSAGE.to_string())?;
        }
        transaction
            .commit()
            .map_err(|_| STORAGE_NOT_READY_MESSAGE.to_string())?;
        let mut result = route_tasks_result(&batch_id, &default, &planned, &fallback);
        if let Some(reason) = parent_reason {
            result["structuredContent"]["reason"] = Value::String(reason.to_string());
        }
        Ok(result)
    }

    /// Records the outcome and the receipt of each route of one finished
    /// Workflow run. A retry on the `retry` route becomes an escalated route
    /// that points to the first attempt.
    fn report_task_results(&self, args: &Value) -> Result<Value, String> {
        let reports = parse_task_reports(args)?;
        let mut connection = self.routing_connection()?;
        require_workflow_columns(&connection)?;
        let mut routes = Vec::new();
        for report in &reports {
            let route = crate::storage::task_route_row(&connection, &report.task_id)
                .map_err(|_| STORAGE_NOT_READY_MESSAGE.to_string())?
                .ok_or_else(|| format!("Halv does not know task id {}.", report.task_id))?;
            if route.session_id != self.session_id {
                return Err("The task belongs to a different routing session.".into());
            }
            if route.workflow.is_none() {
                return Err(format!(
                    "Task {} is not a workflow route. Report it with report_task_result.",
                    report.task_id
                ));
            }
            let retry_rows = crate::storage::list_task_routes_rows(
                &connection,
                "escalated_from_id = ?1",
                &report.task_id,
            )
            .map_err(|_| STORAGE_NOT_READY_MESSAGE.to_string())?;
            routes.push((route, retry_rows.into_iter().next()));
        }
        let measured = match (self.claude_projects_root.as_deref(), self.cli_session_id()) {
            (Some(root), Some(cli_session_id)) => {
                workflow_receipts::measure_session_agents(root, &cli_session_id)
            }
            _ => Vec::new(),
        };
        let observed_at = Utc::now().to_rfc3339();
        let mut writes = Vec::new();
        let mut answers = Vec::new();
        for (report, (route, existing_retry)) in reports.iter().zip(routes) {
            let workflow = route.workflow.clone().expect("checked above");
            let agents_of = |retry: bool| {
                measured
                    .iter()
                    .filter(|agent| {
                        agent.route.short_id == workflow_receipts::short_id(&route.id)
                            && agent.route.key == workflow.key
                            && agent.route.retry == retry
                    })
                    .collect::<Vec<_>>()
            };
            let first_agents = agents_of(false);
            let retry_agents = agents_of(true);
            let retried = report.retried.is_some() || !retry_agents.is_empty();
            let outcome = match (retried, report.first.passed) {
                (true, _) => "escalated",
                (false, true) => "passed",
                (false, false) => "failed",
            };
            let allowed = route.status == "routed"
                || route.status == outcome
                || (route.status == "failed" && outcome == "escalated");
            if !allowed {
                return Err(format!(
                    "Task {} was already reported as {}.",
                    route.id, route.status
                ));
            }
            let (receipt, agents) =
                build_receipt(&route, outcome, &report.first, &first_agents, &observed_at);
            let mut answer = json!({
                "taskId": route.id,
                "outcome": outcome,
                "usage": receipt.usage_source,
            });
            let retry_write = if retried {
                // The report of the retry. Without one, the retry agents ran
                // and the stage result is the result of the retry.
                let attempt = report.retried.clone().unwrap_or(ReportedAttempt {
                    passed: report.first.passed,
                    ..ReportedAttempt::default()
                });
                let retry_outcome = if attempt.passed { "passed" } else { "failed" };
                let retry_route = match existing_retry {
                    Some(existing) => existing,
                    None => self
                        .workflow_retry_route(&route, &workflow)
                        .ok_or_else(|| {
                            format!("Task {} has no retry route to record.", route.id)
                        })?,
                };
                let (retry_receipt, retry_agent_rows) = build_receipt(
                    &retry_route,
                    retry_outcome,
                    &attempt,
                    &retry_agents,
                    &observed_at,
                );
                answer["retryTaskId"] = Value::String(retry_route.id.clone());
                answer["retryOutcome"] = Value::String(retry_outcome.to_string());
                Some((retry_route, retry_outcome, retry_receipt, retry_agent_rows))
            } else {
                None
            };
            answers.push(answer);
            writes.push((route, outcome, receipt, agents, retry_write));
        }
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|_| STORAGE_NOT_READY_MESSAGE.to_string())?;
        let storage_error = |_| STORAGE_NOT_READY_MESSAGE.to_string();
        for (route, outcome, receipt, agents, retry) in writes {
            crate::storage::update_task_route_status_row(&transaction, &route.id, outcome)
                .map_err(storage_error)?;
            crate::storage::upsert_task_route_receipt_row(&transaction, &receipt)
                .map_err(storage_error)?;
            crate::storage::replace_task_route_agent_rows(&transaction, &route.id, &agents)
                .map_err(storage_error)?;
            if let Some((retry_route, retry_outcome, retry_receipt, retry_agents)) = retry {
                if crate::storage::task_route_row(&transaction, &retry_route.id)
                    .map_err(storage_error)?
                    .is_none()
                {
                    crate::storage::insert_task_route_row(&transaction, &retry_route)
                        .map_err(storage_error)?;
                }
                crate::storage::update_task_route_status_row(
                    &transaction,
                    &retry_route.id,
                    retry_outcome,
                )
                .map_err(storage_error)?;
                crate::storage::upsert_task_route_receipt_row(&transaction, &retry_receipt)
                    .map_err(storage_error)?;
                crate::storage::replace_task_route_agent_rows(
                    &transaction,
                    &retry_route.id,
                    &retry_agents,
                )
                .map_err(storage_error)?;
            }
        }
        transaction.commit().map_err(storage_error)?;
        let payload = json!({ "results": answers });
        let mut result = tool_text_result(format!(
            "{}\nHalv recorded {} workflow receipts.",
            payload,
            reports.len()
        ));
        result["structuredContent"] = payload;
        Ok(result)
    }

    /// The escalated route of a workflow route, from its stored decision.
    fn workflow_retry_route(
        &self,
        route: &TaskRoute,
        workflow: &TaskRouteWorkflow,
    ) -> Option<TaskRoute> {
        let role = WorkflowRole::parse(&workflow.role)?;
        let decision = decision_from_route(route);
        let coordinator_cap = route
            .default_model
            .as_deref()
            .filter(|model| task_routing::lane_for_model(model) == Lane::Claude)
            .map_or(LadderTier::Strong, |model| {
                task_routing::tier_for_model(Lane::Claude, model)
            });
        let input = WorkflowRoutingInput {
            decision: &decision,
            role,
            coordinator_cap,
            catalog: Some(self.routing_catalog()),
            worker_lane: self.worker_lane,
            tier_agents: Some(self.tier_agents.clone()),
            default_effort: claude_cheapest_default_effort(&self.database_path),
        };
        let next = task_routing::workflow_escalation(&result_from_route(route), &input)?;
        let retry = self.new_task_route(
            &decision,
            &next,
            "",
            Some(route.title.as_str()),
            Some(route.id.as_str()),
            WorkerLane::Fixed(Lane::Claude),
        );
        Some(TaskRoute {
            brief_chars: route.brief_chars,
            default_model: route.default_model.clone(),
            default_effort: route.default_effort.clone(),
            default_source: route.default_source.clone(),
            workflow: Some(workflow.clone()),
            ..retry
        })
    }

    fn new_task_route(
        &self,
        decision: &crate::auth::RouteDecision,
        result: &RoutingResult,
        brief: &str,
        title: Option<&str>,
        escalated_from_id: Option<&str>,
        worker_lane: WorkerLane,
    ) -> TaskRoute {
        let now = Utc::now().to_rfc3339();
        TaskRoute {
            id: crate::storage::new_uuid_v4(),
            session_id: self.session_id.clone(),
            conversation_id: self.conversation_id.clone(),
            title: task_title(brief, title),
            brief_chars: i64::try_from(brief.chars().count()).unwrap_or(i64::MAX),
            complexity: decision.complexity.choice.clone(),
            complexity_confidence: decision.complexity.confidence,
            needs_reasoning: decision.needs_reasoning,
            risky: decision.risky,
            tier: result.tier.as_str().to_string(),
            lane: result.lane.as_str().to_string(),
            model: result.model.clone(),
            effort: result.effort.clone(),
            reasons: result
                .reasons
                .iter()
                .map(|reason| reason.as_str().to_string())
                .collect(),
            worker_lane: worker_lane.as_str().to_string(),
            status: "routed".to_string(),
            escalated_from_id: escalated_from_id.map(str::to_owned),
            provider: decision.provider.clone(),
            provider_model: decision.model.clone(),
            latency_ms: i64::try_from(decision.latency_ms).unwrap_or(i64::MAX),
            created_at: now.clone(),
            updated_at: now,
            default_model: None,
            default_effort: None,
            default_source: None,
            closed_unreported: false,
            workflow: None,
            receipt: None,
        }
    }
}

fn load_cli_models(database_path: &Path) -> Vec<crate::agent_sessions::model_catalog::CliModel> {
    database_path
        .parent()
        .map(|directory| directory.join("cli-models.json"))
        .and_then(|path| std::fs::read(path).ok())
        .and_then(|bytes| {
            serde_json::from_slice::<Vec<crate::agent_sessions::model_catalog::CliModel>>(&bytes)
                .ok()
        })
        .unwrap_or_default()
}

fn load_routing_catalog(database_path: &Path) -> Catalog {
    task_routing::catalog_from_available_models(&load_cli_models(database_path))
}

/// The CLI default effort of Haiku, the cheapest Claude tier. The catalog
/// gives that tier no effort, and a workflow route always sets one.
fn claude_cheapest_default_effort(database_path: &Path) -> Option<String> {
    load_cli_models(database_path)
        .into_iter()
        .find(|model| model.provider == "claude_cli" && model.id.contains("haiku"))
        .map(|model| model.default_effort)
        .filter(|effort| !effort.is_empty())
}

async fn request_decision(
    server_url: &str,
    token: &str,
    brief: &str,
) -> Result<crate::auth::RouteDecision, DecisionError> {
    let response = crate::auth::api_client()
        .post(crate::auth::api_url(server_url, "/v1/route"))
        .bearer_auth(token)
        .json(&json!({ "brief": brief }))
        .timeout(ROUTE_REQUEST_TIMEOUT)
        .send()
        .await
        .map_err(|error| DecisionError::Failed(format!("Could not route task: {error}")))?;
    let status = response.status();
    // The server refuses an expired session (401). The user must sign in
    // again. It refuses an account without a plan that includes Halv Cloud
    // with 402 or 403.
    if status.as_u16() == 401 {
        return Err(DecisionError::SignInRequired);
    }
    if matches!(status.as_u16(), 402 | 403) {
        return Err(DecisionError::CloudRequired);
    }
    let body = response.text().await.map_err(|error| {
        DecisionError::Failed(format!("Could not read the route response: {error}"))
    })?;
    if !status.is_success() {
        return Err(DecisionError::Failed(format!(
            "Could not route task (HTTP {status})."
        )));
    }
    serde_json::from_str::<crate::auth::RouteDecision>(&body).map_err(|error| {
        DecisionError::Failed(format!("The route response was not valid JSON: {error}"))
    })
}

/// The model and the effort that a worker inherits without JEV, and their
/// source.
#[derive(Debug, Clone, PartialEq, Eq)]
struct RouteDefault {
    model: Option<String>,
    effort: Option<String>,
    source: &'static str,
}

impl RouteDefault {
    fn apply(&self, route: TaskRoute) -> TaskRoute {
        TaskRoute {
            default_model: self.model.clone(),
            default_effort: self.effort.clone(),
            default_source: Some(self.source.to_string()),
            ..route
        }
    }
}

fn require_workflow_columns(connection: &Connection) -> Result<(), String> {
    match crate::storage::task_route_workflow_columns_exist(connection) {
        Ok(true) => Ok(()),
        _ => Err(STORAGE_NOT_READY_MESSAGE.to_string()),
    }
}

/// The stored decision of a route that the coordinator does itself: no Halv
/// Cloud access (`halv_cloud_required`), no valid session
/// (`sign_in_required`), or no "yes" to the task brief question
/// (`brief_sharing_off`).
fn parent_route_decision(reason: &str) -> crate::auth::RouteDecision {
    crate::auth::RouteDecision {
        provider: reason.to_string(),
        ..fallback_decision()
    }
}

/// The stored decision of a route that JEV did not answer.
fn fallback_decision() -> crate::auth::RouteDecision {
    crate::auth::RouteDecision {
        schema: "task-tier-v1".to_string(),
        provider: "fallback".to_string(),
        model: "none".to_string(),
        complexity: crate::auth::RouteChoice {
            choice: "standard".to_string(),
            confidence: 0.0,
            probabilities: std::collections::BTreeMap::new(),
        },
        needs_reasoning: 0.0,
        risky: 0.0,
        usage: crate::auth::RouteUsage {
            input_tokens: None,
            output_tokens: None,
        },
        latency_ms: 0,
    }
}

struct WorkflowTask {
    key: String,
    brief: String,
    role: WorkflowRole,
    instances: i64,
}

struct WorkflowRequest {
    workflow: Option<String>,
    coordinator_model: Option<String>,
    tasks: Vec<WorkflowTask>,
}

fn optional_bounded_text(
    args: &Value,
    key: &str,
    max_chars: usize,
) -> Result<Option<String>, String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text)) => {
            let text = text.trim();
            if text.chars().count() > max_chars {
                return Err(format!("{key} exceeds {max_chars} characters."));
            }
            Ok((!text.is_empty()).then(|| text.to_string()))
        }
        Some(_) => Err(format!("{key} must be a string.")),
    }
}

fn valid_workflow_key(key: &str) -> bool {
    !key.is_empty()
        && key.chars().count() <= MAX_WORKFLOW_KEY_CHARS
        && key.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '_' | '.' | '-')
        })
}

impl WorkflowRequest {
    fn parse(args: &Value) -> Result<Self, String> {
        let workflow = optional_bounded_text(args, "workflow", MAX_WORKFLOW_NAME_CHARS)?;
        let coordinator_model =
            optional_bounded_text(args, "coordinatorModel", MAX_COORDINATOR_MODEL_CHARS)?;
        let entries = args
            .get("tasks")
            .and_then(Value::as_array)
            .ok_or("tasks must be an array.")?;
        if entries.is_empty() || entries.len() > MAX_WORKFLOW_TASKS {
            return Err(format!("Send 1 to {MAX_WORKFLOW_TASKS} tasks."));
        }
        let mut keys = BTreeSet::new();
        let mut total_chars = 0;
        let mut tasks = Vec::new();
        for entry in entries {
            let key = entry
                .get("key")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|key| valid_workflow_key(key))
                .ok_or("Each task needs a key of 1 to 48 letters, digits, '_', '.', or '-'.")?;
            if !keys.insert(key.to_string()) {
                return Err(format!("The task key {key} is not unique."));
            }
            let brief = entry
                .get("brief")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|brief| !brief.is_empty())
                .ok_or_else(|| format!("The brief of task {key} is empty."))?;
            let chars = brief.chars().count();
            if chars > MAX_WORKFLOW_BRIEF_CHARS {
                return Err(format!(
                    "The brief of task {key} exceeds {MAX_WORKFLOW_BRIEF_CHARS} characters."
                ));
            }
            total_chars += chars;
            let role = entry
                .get("role")
                .and_then(Value::as_str)
                .and_then(WorkflowRole::parse)
                .ok_or_else(|| {
                    format!("The role of task {key} must be read, implement, or verify.")
                })?;
            let instances = match entry.get("instances") {
                None | Some(Value::Null) => 1,
                Some(value) => value
                    .as_i64()
                    .filter(|count| (1..=MAX_WORKFLOW_INSTANCES).contains(count))
                    .ok_or_else(|| {
                        format!(
                            "The instances of task {key} must be 1 to {MAX_WORKFLOW_INSTANCES}."
                        )
                    })?,
            };
            tasks.push(WorkflowTask {
                key: key.to_string(),
                brief: brief.to_string(),
                role,
                instances,
            });
        }
        if total_chars > MAX_WORKFLOW_BATCH_BRIEF_CHARS {
            return Err(format!(
                "The briefs exceed {MAX_WORKFLOW_BATCH_BRIEF_CHARS} characters in total."
            ));
        }
        Ok(Self {
            workflow,
            coordinator_model,
            tasks,
        })
    }
}

/// The `agent()` options of one route, with its label.
fn agent_options(result: &RoutingResult, label: String) -> Value {
    let mut options =
        serde_json::to_value(task_routing::agent_opts(result)).unwrap_or_else(|_| json!({}));
    options["label"] = Value::String(label);
    options
}

/// The compact text holds what the script needs. The structured content
/// adds the tiers, roles, and reasons.
fn route_tasks_result(
    batch_id: &str,
    default: &RouteDefault,
    planned: &[(TaskRoute, RoutingResult, Option<RoutingResult>)],
    fallback: &Map<String, Value>,
) -> Value {
    let mut routes = Map::new();
    let mut details = Map::new();
    for (route, result, retry) in planned {
        let key = route
            .workflow
            .as_ref()
            .map(|workflow| workflow.key.clone())
            .unwrap_or_default();
        let opts = agent_options(result, workflow_receipts::route_label(&route.id, &key));
        let retry = retry.as_ref().map_or(Value::Null, |retry| {
            agent_options(retry, workflow_receipts::retry_label(&route.id, &key))
        });
        routes.insert(
            key.clone(),
            json!({ "taskId": route.id, "opts": opts, "retry": retry }),
        );
        details.insert(
            key,
            json!({
                "taskId": route.id,
                "tier": route.tier,
                "role": route.workflow.as_ref().map(|workflow| workflow.role.clone()),
                "instances": route.workflow.as_ref().map(|workflow| workflow.instances),
                "opts": opts,
                "retry": retry,
                "reasons": route.reasons,
            }),
        );
    }
    let compact = json!({ "batchId": batch_id, "routes": routes });
    let mut text = format!(
        "{compact}\nPass routes to the Workflow tool as args.jev. \
         Spread args.jev[key].opts into each routed agent() call. \
         For several agents on one route, append ':' and an item name to opts.label. \
         Options without model and effort inherit your model and effort. \
         After a failed check, retry once with args.jev[key].retry. \
         After the workflow, call report_task_results once with every taskId."
    );
    if !fallback.is_empty() {
        let keys = fallback.keys().cloned().collect::<Vec<_>>().join(", ");
        text.push_str(&format!(
            "\nJEV did not answer for {keys}. These routes inherit your model and effort."
        ));
    }
    let mut result = tool_text_result(text);
    result["structuredContent"] = json!({
        "batchId": batch_id,
        "defaultModel": default.model,
        "defaultEffort": default.effort,
        "defaultSource": default.source,
        "routes": details,
        "fallback": fallback,
    });
    result
}

/// What the coordinator reports about one attempt of a route.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct ReportedAttempt {
    passed: bool,
    agents: Option<i64>,
    failed_agents: Option<i64>,
    duration_ms: Option<i64>,
    /// Input, output, cache read, and cache creation tokens.
    usage: Option<[i64; 4]>,
}

struct TaskReport {
    task_id: String,
    first: ReportedAttempt,
    retried: Option<ReportedAttempt>,
}

fn reported_count(value: &Value, key: &str, max: i64) -> Result<Option<i64>, String> {
    match value.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(number) => number
            .as_i64()
            .filter(|count| (0..=max).contains(count))
            .map(Some)
            .ok_or_else(|| format!("{key} must be an integer from 0 to {max}.")),
    }
}

fn parse_attempt(value: &Value) -> Result<ReportedAttempt, String> {
    let passed = value
        .get("passed")
        .and_then(Value::as_bool)
        .ok_or("Each result needs a boolean passed.")?;
    let usage = match value.get("usage") {
        None | Some(Value::Null) => None,
        Some(usage) => {
            let mut tokens = [0_i64; 4];
            for (slot, key) in tokens.iter_mut().zip([
                "inputTokens",
                "outputTokens",
                "cacheReadTokens",
                "cacheCreationTokens",
            ]) {
                *slot = reported_count(usage, key, i64::MAX)?.unwrap_or_default();
            }
            Some(tokens)
        }
    };
    Ok(ReportedAttempt {
        passed,
        agents: reported_count(value, "agents", MAX_REPORTED_AGENTS)?,
        failed_agents: reported_count(value, "failedAgents", MAX_REPORTED_AGENTS)?,
        duration_ms: reported_count(value, "durationMs", i64::MAX)?,
        usage,
    })
}

fn parse_task_reports(args: &Value) -> Result<Vec<TaskReport>, String> {
    let entries = args
        .get("results")
        .and_then(Value::as_array)
        .ok_or("results must be an array.")?;
    if entries.is_empty() || entries.len() > MAX_REPORT_RESULTS {
        return Err(format!("Send 1 to {MAX_REPORT_RESULTS} results."));
    }
    let mut seen = BTreeSet::new();
    let mut reports = Vec::new();
    for entry in entries {
        let task_id = entry
            .get("taskId")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .ok_or("Each result needs a taskId.")?;
        if !seen.insert(task_id.to_string()) {
            return Err(format!("Task {task_id} appears more than once."));
        }
        let retried = match entry.get("retried") {
            None | Some(Value::Null) => None,
            Some(retried) => Some(parse_attempt(retried)?),
        };
        reports.push(TaskReport {
            task_id: task_id.to_string(),
            first: parse_attempt(entry)?,
            retried,
        });
    }
    Ok(reports)
}

/// The receipt of one attempt. Measured transcripts win over reported numbers.
fn build_receipt(
    route: &TaskRoute,
    outcome: &str,
    reported: &ReportedAttempt,
    measured: &[&workflow_receipts::MeasuredAgent],
    observed_at: &str,
) -> (TaskRouteReceipt, Vec<TaskRouteAgent>) {
    let instances = route
        .workflow
        .as_ref()
        .map_or(1, |workflow| workflow.instances);
    let mut agents = Vec::new();
    let mut totals = [0_i64; 4];
    for agent in measured {
        for usage in &agent.usage {
            totals[0] = totals[0].saturating_add(usage.input_tokens);
            totals[1] = totals[1].saturating_add(usage.output_tokens);
            totals[2] = totals[2].saturating_add(usage.cache_read_tokens);
            totals[3] = totals[3]
                .saturating_add(usage.cache_write_5m_tokens)
                .saturating_add(usage.cache_write_1h_tokens);
            agents.push(TaskRouteAgent {
                route_id: route.id.clone(),
                agent_id: agent.agent_id.clone(),
                observed_model: usage.model.clone(),
                workflow_run_id: agent.workflow_run_id.clone(),
                label: agent.label.clone(),
                observed_effort: agent.effort.clone(),
                input_tokens: usage.input_tokens,
                output_tokens: usage.output_tokens,
                cache_read_tokens: usage.cache_read_tokens,
                cache_write_5m_tokens: usage.cache_write_5m_tokens,
                cache_write_1h_tokens: usage.cache_write_1h_tokens,
                actual_usd: Some(usage.actual_usd),
                price_version: Some(crate::pricing::current().price_version.clone()),
                started_at: agent.started_at.map(|time| time.to_rfc3339()),
                ended_at: agent.ended_at.map(|time| time.to_rfc3339()),
                observed_at: observed_at.to_string(),
            });
        }
    }
    let started = measured.iter().filter_map(|agent| agent.started_at).min();
    let ended = measured.iter().filter_map(|agent| agent.ended_at).max();
    let measured_duration = started
        .zip(ended)
        .map(|(started, ended)| (ended - started).num_milliseconds().max(0));
    let (usage_source, tokens) = if !measured.is_empty() {
        ("measured", Some(totals))
    } else if let Some(tokens) = reported.usage {
        ("reported", Some(tokens))
    } else {
        ("none", None)
    };
    let receipt = TaskRouteReceipt {
        route_id: route.id.clone(),
        session_id: route.session_id.clone(),
        outcome: outcome.to_string(),
        agents: if measured.is_empty() {
            reported.agents.unwrap_or(instances)
        } else {
            i64::try_from(measured.len()).unwrap_or(i64::MAX)
        },
        failed_agents: reported.failed_agents.unwrap_or_default(),
        input_tokens: tokens.map(|tokens| tokens[0]),
        output_tokens: tokens.map(|tokens| tokens[1]),
        cache_read_tokens: tokens.map(|tokens| tokens[2]),
        cache_creation_tokens: tokens.map(|tokens| tokens[3]),
        duration_ms: measured_duration.or(reported.duration_ms),
        usage_source: usage_source.to_string(),
        reported_at: observed_at.to_string(),
        models: Vec::new(),
    };
    (receipt, agents)
}

/// Measures the agents of workflow routes that a session end closed before
/// any report (ADR 0016). The app calls it before it settles their chains,
/// so the low estimate can count their cost as a failure. Each entry pairs a
/// route with the Claude Code session id of its Halv session. A retry agent
/// of the route counts with the route, because no retry route exists.
pub(crate) fn measure_unreported_route_agents(
    routes: &[(TaskRoute, String)],
) -> Vec<(String, Vec<TaskRouteAgent>)> {
    let Some(root) = workflow_receipts::claude_projects_root() else {
        return Vec::new();
    };
    let observed_at = Utc::now().to_rfc3339();
    let mut by_session = std::collections::BTreeMap::<&str, Vec<&TaskRoute>>::new();
    for (route, cli_session_id) in routes {
        by_session
            .entry(cli_session_id.as_str())
            .or_default()
            .push(route);
    }
    let mut measured_routes = Vec::new();
    for (cli_session_id, routes) in by_session {
        let measured = workflow_receipts::measure_session_agents(&root, cli_session_id);
        for route in routes {
            let Some(workflow) = route.workflow.as_ref() else {
                continue;
            };
            let agents = measured
                .iter()
                .filter(|agent| {
                    agent.route.short_id == workflow_receipts::short_id(&route.id)
                        && agent.route.key == workflow.key
                })
                .collect::<Vec<_>>();
            let reported = ReportedAttempt {
                passed: false,
                ..ReportedAttempt::default()
            };
            let (_, rows) = build_receipt(route, "failed", &reported, &agents, &observed_at);
            measured_routes.push((route.id.clone(), rows));
        }
    }
    measured_routes
}

/// Builds the title of a task from the first line of its brief.
fn task_title(brief: &str, title: Option<&str>) -> String {
    let explicit = title.map(str::trim).filter(|value| !value.is_empty());
    let first_line = brief.lines().next().unwrap_or_default().trim();
    let text = explicit.unwrap_or(first_line);
    text.chars().take(MAX_TASK_TITLE_CHARS).collect()
}

fn decision_from_route(route: &TaskRoute) -> crate::auth::RouteDecision {
    crate::auth::RouteDecision {
        schema: "task-tier-v1".to_string(),
        provider: route.provider.clone(),
        model: route.provider_model.clone(),
        complexity: crate::auth::RouteChoice {
            choice: route.complexity.clone(),
            confidence: route.complexity_confidence,
            probabilities: std::collections::BTreeMap::new(),
        },
        needs_reasoning: route.needs_reasoning,
        risky: route.risky,
        usage: crate::auth::RouteUsage {
            input_tokens: None,
            output_tokens: None,
        },
        latency_ms: u64::try_from(route.latency_ms).unwrap_or_default(),
    }
}

fn result_from_route(route: &TaskRoute) -> RoutingResult {
    RoutingResult {
        tier: LadderTier::parse(&route.tier).unwrap_or(LadderTier::Cheap),
        lane: if route.lane == "codex" {
            Lane::Codex
        } else {
            Lane::Claude
        },
        model: route.model.clone(),
        effort: route.effort.clone(),
        reasons: route
            .reasons
            .iter()
            .filter_map(|reason| serde_json::from_value(Value::String(reason.clone())).ok())
            .collect(),
    }
}

/// The compact answer of a routing tool. The field order follows the spec.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RouteToolPayload<'a> {
    task_id: &'a str,
    tier: &'a str,
    lane: &'a str,
    model: Option<&'a str>,
    effort: Option<&'a str>,
    reasons: &'a [String],
    complexity: &'a str,
    confidence: f64,
    needs_reasoning: f64,
    risky: f64,
    provider: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    escalated_from: Option<&'a str>,
    /// Only on a route without Halv Cloud access: `execute_in_parent`.
    #[serde(skip_serializing_if = "Option::is_none")]
    action: Option<&'a str>,
    /// Only on a route that runs in the parent: `halv_cloud_required`,
    /// `sign_in_required`, or `brief_sharing_off`.
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<&'a str>,
}

/// Build the MCP answer with one JSON line and the worker instruction.
fn route_tool_result(
    route: &TaskRoute,
    decision: &crate::auth::RouteDecision,
    escalated_from: Option<&str>,
) -> Value {
    // A route without Halv Cloud access, without a valid session, or
    // without a "yes" to the task brief question runs in the parent. The
    // reason tells the agent which one.
    let parent_reason = route.reasons.iter().map(String::as_str).find(|reason| {
        matches!(
            *reason,
            HALV_CLOUD_REQUIRED | SIGN_IN_REQUIRED | BRIEF_SHARING_OFF
        )
    });
    let cloud_required = parent_reason.is_some();
    let parent_message = match parent_reason {
        Some(SIGN_IN_REQUIRED) => SIGN_IN_REQUIRED_MESSAGE,
        Some(BRIEF_SHARING_OFF) => BRIEF_SHARING_OFF_MESSAGE,
        _ => HALV_CLOUD_REQUIRED_MESSAGE,
    };
    let payload = RouteToolPayload {
        task_id: &route.id,
        tier: &route.tier,
        lane: &route.lane,
        model: route.model.as_deref(),
        effort: route.effort.as_deref(),
        reasons: &route.reasons,
        complexity: &route.complexity,
        confidence: route.complexity_confidence,
        needs_reasoning: route.needs_reasoning,
        risky: route.risky,
        provider: &decision.provider,
        escalated_from,
        action: cloud_required.then_some(EXECUTE_IN_PARENT),
        reason: parent_reason,
    };
    let json_line = serde_json::to_string(&payload).unwrap_or_default();
    let instruction = match (route.tier.as_str(), route.lane.as_str(), route.model.as_deref(), route.effort.as_deref()) {
        ("max", _, _, _) if cloud_required => format!(
            "{parent_message} Do this task yourself at the coordinator tier. \
             Do not spawn a worker for it."
        ),
        ("max", _, _, _) => "Do this task yourself at the coordinator tier.".to_string(),
        (_, "codex", Some(model), Some(effort)) => format!(
            "Call native spawn_agent now for this brief with model {model} and reasoning_effort {effort}. \
             Start the worker before editing its files. \
             Do not implement this brief yourself. \
             Report the worker result with report_task_result and taskId {}.",
            route.id
        ),
        (_, lane, Some(model), Some(effort)) => format!(
            "Start the assigned {lane} worker now with model {model} and effort {effort}. \
             Do not implement this brief yourself. \
             Report the worker result with report_task_result and taskId {}.",
            route.id
        ),
        (_, lane, Some(model), None) => format!(
            "Start the assigned {lane} worker now with model {model}. \
             Do not implement this brief yourself. \
             Report the worker result with report_task_result and taskId {}.",
            route.id
        ),
        _ => "Do this task yourself at the coordinator tier.".to_string(),
    };
    let mut result = tool_text_result(format!("{json_line}\n{instruction}"));
    result["structuredContent"] = serde_json::to_value(payload).unwrap_or(Value::Null);
    result
}

fn hierarchy_tool_result(value: Value) -> Value {
    let instruction = match value["action"].as_str() {
        Some("spawn") => "Spawn the returned lane and taskName with the returned model and effort. Use fork_turns=none and pass taskId, stage, evidence, files, and checks.",
        Some("execute_in_parent") => "The current owner must complete this work. Do not spawn this route or lower its tier. Report its result with taskId.",
        _ => "Use this reported result for the next hierarchy stage.",
    };
    json!({"content":[{"type":"text","text":format!("{}\n{}", value, instruction)}],
        "structuredContent":value,"isError":false})
}

fn tool_text_result(text: String) -> Value {
    json!({
        "content": [{ "type": "text", "text": text }],
        "isError": false,
    })
}

fn tool_error_result(text: &str) -> Value {
    json!({
        "content": [{ "type": "text", "text": text }],
        "isError": true,
    })
}

fn trailing_week_cutoff() -> String {
    (Utc::now() - Duration::days(7)).to_rfc3339()
}

fn nonnegative_u64(value: i64) -> u64 {
    u64::try_from(value.max(0)).unwrap_or(u64::MAX)
}

fn serve(input: impl BufRead, mut output: impl Write, reader: &SavingsReader) -> Result<(), ()> {
    for line in input.lines() {
        let line = line.map_err(|_| ())?;
        let request = match serde_json::from_str::<Value>(&line) {
            Ok(request) => request,
            Err(_) => {
                write_message(&mut output, rpc_error(Value::Null, -32700, "parse error"))?;
                continue;
            }
        };
        let Some(method) = request.get("method").and_then(Value::as_str) else {
            continue;
        };
        if method == "exit" {
            break;
        }
        let Some(id) = request.get("id").cloned() else {
            continue;
        };
        let response = match method {
            "initialize" => rpc_result(
                id,
                initialize_result(&request, reader.task_routing, reader.workflow_routing),
            ),
            "ping" | "shutdown" => rpc_result(id, json!({})),
            "tools/list" => {
                rpc_result(id, tools_list(reader.task_routing, reader.workflow_routing))
            }
            "tools/call" => tool_call_response(id, &request, reader),
            "resources/list" => rpc_result(id, resources_list()),
            "resources/templates/list" => rpc_result(id, json!({ "resourceTemplates": [] })),
            "resources/read" => resource_read_response(id, &request, reader),
            _ => rpc_error(id, -32601, "method not found"),
        };
        write_message(&mut output, response)?;
    }
    Ok(())
}

/// The server instructions reach the coordinator and every subagent, so they
/// stay short. The tool descriptions carry the details on demand.
fn server_instructions(task_routing: bool, workflow_routing: bool) -> String {
    if !task_routing {
        return "Halv savings. Read the token and dollar savings of this session. Task routing is off for this session.".to_string();
    }
    let mut text = String::from(
        "Halv JEV routing. Only the root coordinator routes. A worker or workflow agent calls a routing or report tool only when its prompt says so. Codex coordinators use investigate-l1-l2-v1: route investigation before repository discovery, report evidence, then route l1. Only L1 routes l2, and L2 cannot delegate.",
    );
    if workflow_routing {
        text.push_str(
            " Before a Claude Code Workflow script, route its agent stages with route_tasks, and report them once with report_task_results.",
        );
    }
    text
}

fn initialize_result(request: &Value, task_routing: bool, workflow_routing: bool) -> Value {
    let protocol_version = request
        .pointer("/params/protocolVersion")
        .and_then(Value::as_str)
        .unwrap_or(MCP_PROTOCOL_VERSION);
    json!({
        "protocolVersion": protocol_version,
        "capabilities": {
            "tools": {},
            "resources": {},
        },
        "serverInfo": {
            "name": "halv-savings",
            "version": env!("CARGO_PKG_VERSION"),
        },
        "instructions": server_instructions(task_routing, workflow_routing),
    })
}

fn workflow_tools() -> Vec<Value> {
    vec![
        json!({
            "name": "route_tasks",
            "description": "Route every agent stage of one Claude Code Workflow script in one call, after scouting and before you write the script. Halv's router sets the model and effort of each workflow agent. This is an explicit instruction to use them. Pass the returned routes to the Workflow tool as args.jev. In the script, spread a route into each routed agent: agent(prompt, {...args.jev[key].opts, phase, schema}). For several agents on one route, keep the label prefix: label: args.jev[key].opts.label + ':' + item. Options without model and effort inherit your model and effort. After a failed check, retry once with args.jev[key].retry in the same way. It is null when no stronger route exists. Do not route a synthesis or final judge agent. On a resume, pass the same routes. After the workflow, call report_task_results once.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "workflow": {
                        "type": "string",
                        "maxLength": MAX_WORKFLOW_NAME_CHARS,
                        "description": "A short workflow name. Halv groups the routes under it.",
                    },
                    "coordinatorModel": {
                        "type": "string",
                        "maxLength": MAX_COORDINATOR_MODEL_CHARS,
                        "description": "Your exact model id from your system prompt, for example claude-opus-5-5.",
                    },
                    "tasks": {
                        "type": "array",
                        "minItems": 1,
                        "maxItems": MAX_WORKFLOW_TASKS,
                        "items": {
                            "type": "object",
                            "properties": {
                                "key": {
                                    "type": "string",
                                    "pattern": "^[A-Za-z0-9_.-]{1,48}$",
                                    "description": "Unique in this call. The script reads args.jev[key].",
                                },
                                "brief": {
                                    "type": "string",
                                    "maxLength": MAX_WORKFLOW_BRIEF_CHARS,
                                    "description": "Two to five lines: the goal, the scope, and the completion check. Not the full agent prompt.",
                                },
                                "role": {
                                    "type": "string",
                                    "enum": ["read", "implement", "verify"],
                                    "description": "read: scout, find, map, or reproduce without edits. implement: change files. verify: review, challenge, or check other agents' work.",
                                },
                                "instances": {
                                    "type": "integer",
                                    "minimum": 1,
                                    "maximum": MAX_WORKFLOW_INSTANCES,
                                    "description": "The number of similar agents that share this route.",
                                },
                            },
                            "required": ["key", "brief", "role"],
                            "additionalProperties": false,
                        },
                    },
                },
                "required": ["tasks"],
                "additionalProperties": false,
            },
            "annotations": {
                "readOnlyHint": false,
                "destructiveHint": false,
                "idempotentHint": false,
                "openWorldHint": true,
            },
        }),
        json!({
            "name": "report_task_results",
            "description": "Report the routes of one finished Workflow run in one call. passed is the result of the first attempt of a route. Add retried when the script ran the retry options for that route, with the result of the retry. Halv measures the tokens of each labelled agent from its transcript. Add usage or durationMs only with exact numbers from a tool.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "results": {
                        "type": "array",
                        "minItems": 1,
                        "maxItems": MAX_REPORT_RESULTS,
                        "items": {
                            "type": "object",
                            "properties": {
                                "taskId": {"type": "string", "description": "A taskId that route_tasks returned."},
                                "passed": {"type": "boolean", "description": "True when the first attempt of the route passed its check."},
                                "agents": {"type": "integer", "minimum": 0, "maximum": MAX_REPORTED_AGENTS},
                                "failedAgents": {"type": "integer", "minimum": 0, "maximum": MAX_REPORTED_AGENTS},
                                "durationMs": {"type": "integer", "minimum": 0},
                                "usage": {
                                    "type": "object",
                                    "properties": {
                                        "inputTokens": {"type": "integer", "minimum": 0},
                                        "outputTokens": {"type": "integer", "minimum": 0},
                                        "cacheReadTokens": {"type": "integer", "minimum": 0},
                                        "cacheCreationTokens": {"type": "integer", "minimum": 0},
                                    },
                                    "additionalProperties": false,
                                },
                                "retried": {
                                    "type": "object",
                                    "description": "The retry on the retry options.",
                                    "properties": {
                                        "passed": {"type": "boolean"},
                                        "agents": {"type": "integer", "minimum": 0, "maximum": MAX_REPORTED_AGENTS},
                                        "failedAgents": {"type": "integer", "minimum": 0, "maximum": MAX_REPORTED_AGENTS},
                                    },
                                    "required": ["passed"],
                                    "additionalProperties": false,
                                },
                            },
                            "required": ["taskId", "passed"],
                            "additionalProperties": false,
                        },
                    },
                },
                "required": ["results"],
                "additionalProperties": false,
            },
            "annotations": {
                "readOnlyHint": false,
                "destructiveHint": false,
                "idempotentHint": false,
                "openWorldHint": false,
            },
        }),
    ]
}

fn tools_list(task_routing: bool, workflow_routing: bool) -> Value {
    let mut tools = legacy_tools();
    // Without task routing, only the savings tools stay. They read local
    // data and send nothing (ADR 0023).
    if !task_routing {
        tools.retain(|tool| {
            matches!(
                tool["name"].as_str(),
                Some("get_session_savings" | "get_savings_totals")
            )
        });
    } else if workflow_routing {
        tools.extend(workflow_tools());
    }
    json!({ "tools": tools })
}

fn legacy_tools() -> Vec<Value> {
    let tools = json!({
        "tools": [
            {
                "name": "get_session_savings",
                "description": "Return measured savings for the current Halv agent session. The shared RTK total is separate.",
                "inputSchema": {
                    "type": "object",
                    "properties": {},
                    "additionalProperties": false,
                },
                "annotations": {
                    "readOnlyHint": true,
                    "destructiveHint": false,
                    "idempotentHint": true,
                    "openWorldHint": false,
                },
            },
            {
                "name": "get_savings_totals",
                "description": "Return current session savings and the trailing seven-day Halv savings total.",
                "inputSchema": {
                    "type": "object",
                    "properties": {},
                    "additionalProperties": false,
                },
                "annotations": {
                    "readOnlyHint": true,
                    "destructiveHint": false,
                    "idempotentHint": true,
                    "openWorldHint": false,
                },
            },
            {
                "name": "route_task",
                "description": "Route a task. workerLane selects the workflow and fallback lane. A saved tier assignment can return another lane. For the Codex workflow, first use stage=investigation, then l1 with its completed parentTaskId. Only L1 requests l2 with files and checks. Spawn only action=spawn. Use the returned lane, taskName, model, and effort. L2 must be cheaper than L1 and cannot delegate. Omit stage for ordinary Claude routing.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "brief": {
                            "type": "string",
                            "description": "The self-contained task brief.",
                            "maxLength": 16000,
                        },
                        "title": {
                            "type": "string",
                            "description": "A short task title. Halv uses the first line of the brief when this is absent.",
                        },
                        "stage": {"type": "string", "enum": ["investigation", "l1", "l2"]},
                        "parentTaskId": {"type": "string", "description": "The investigation taskId for L1, or the active L1 taskId for L2."},
                        "files": {"type": "array", "items": {"type": "string"}, "description": "Specific non-overlapping relative paths owned by L2."},
                        "checks": {"type": "array", "items": {"type": "string"}, "description": "Independent validation commands for L2."},
                        "workerLane": {
                            "type": "string",
                            "enum": ["auto", "claude", "codex"],
                            "description": "Selects the routing flow and fallback lane. Use codex inside Codex and claude inside Claude unless a standing instruction selects another lane. A saved tier assignment can override it. Always use the returned lane, model, and effort. Use auto only for explicit automatic cross-vendor selection.",
                        },
                    },
                    "required": ["brief"],
                    "additionalProperties": false,
                },
                "annotations": {
                    "readOnlyHint": false,
                    "destructiveHint": false,
                    "idempotentHint": false,
                    "openWorldHint": true,
                },
            },
            {
                "name": "report_task_result",
                "description": "Report a routed result. Investigation success requires read-only evidence, not implementation tests. L1 and L2 report their checks. Complete all L2 reports before L1. Follow action on an escalated route.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "taskId": {
                            "type": "string",
                            "description": "The task id that route_task returned.",
                        },
                        "passed": {
                            "type": "boolean",
                            "description": "True when investigation evidence is complete or implementation checks passed.",
                        },
                        "evidence": {"type": "string", "maxLength": 6000, "description": "Required findings and independent decomposition for a successful investigation."},
                    },
                    "required": ["taskId", "passed"],
                    "additionalProperties": false,
                },
                "annotations": {
                    "readOnlyHint": false,
                    "destructiveHint": false,
                    "idempotentHint": false,
                    "openWorldHint": true,
                },
            },
        ],
    });
    tools["tools"].as_array().cloned().unwrap_or_default()
}

fn resources_list() -> Value {
    json!({
        "resources": [{
            "uri": SAVINGS_RESOURCE_URI,
            "name": "Current Halv session savings",
            "description": "Measured savings for the current Halv agent session.",
            "mimeType": "application/json",
        }],
    })
}

fn tool_call_response(id: Value, request: &Value, reader: &SavingsReader) -> Value {
    let name = request
        .pointer("/params/name")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let result = match name {
        "get_session_savings" => reader.session_savings().map(|value| {
            let routing = if value.approximate {
                format!(
                    " Routing: {} (low estimate).",
                    crate::tray::format_usd_compact(Some(value.routing_usd))
                )
            } else {
                String::new()
            };
            let text = if value.known {
                format!(
                    "Current Halv session savings: {} ({} measured tokens). Shared RTK savings: {} tokens.{routing}",
                    dollars_text(value.dollars_usd, value.approximate),
                    value.tokens_saved,
                    value.rtk_tokens_saved
                )
            } else if value.approximate {
                format!(
                    "Current Halv session savings: {}. Token savings are not measured yet. Shared RTK savings: {} tokens.{routing}",
                    dollars_text(value.dollars_usd, value.approximate),
                    value.rtk_tokens_saved
                )
            } else {
                format!(
                    "Current Halv session savings are not measured yet. The session value is 0. Shared RTK savings: {} tokens.",
                    value.rtk_tokens_saved
                )
            };
            tool_result(text, &value)
        }),
        "get_savings_totals" => reader.savings_totals().map(|value| {
            tool_result(
                format!(
                    "Current session: {} ({} tokens). Trailing seven days: {} ({} tokens).",
                    dollars_text(value.session_dollars_usd, value.session_approximate),
                    value.session_tokens_saved,
                    dollars_text(value.this_week_dollars_usd, value.this_week_approximate),
                    value.this_week_tokens_saved
                ),
                &value,
            )
        }),
        "route_task" | "report_task_result" | "route_tasks" | "report_task_results" => {
            if !reader.task_routing {
                return rpc_result(id, tool_error_result(TASK_ROUTING_OFF_MESSAGE));
            }
            let arguments = request.pointer("/params/arguments").unwrap_or(&Value::Null);
            let result = match name {
                "route_task" => reader.route_request(arguments),
                "report_task_result" => reader.report_routed_task(arguments),
                "route_tasks" => reader.route_tasks(arguments),
                _ => reader.report_task_results(arguments),
            };
            return match result {
                Ok(result) => rpc_result(id, result),
                Err(message) => rpc_result(id, tool_error_result(&message)),
            };
        }
        _ => return rpc_error(id, -32602, "unknown savings tool"),
    };
    match result {
        Ok(result) => rpc_result(id, result),
        Err(()) => rpc_result(
            id,
            json!({
                "content": [{
                    "type": "text",
                    "text": "Halv savings data is temporarily unavailable.",
                }],
                "isError": true,
            }),
        ),
    }
}

fn resource_read_response(id: Value, request: &Value, reader: &SavingsReader) -> Value {
    let uri = request
        .pointer("/params/uri")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if uri != SAVINGS_RESOURCE_URI {
        return rpc_error(id, -32602, "unknown savings resource");
    }
    match reader.session_savings() {
        Ok(value) => match serde_json::to_string(&value) {
            Ok(text) => rpc_result(
                id,
                json!({
                    "contents": [{
                        "uri": SAVINGS_RESOURCE_URI,
                        "mimeType": "application/json",
                        "text": text,
                    }],
                }),
            ),
            Err(_) => rpc_error(id, -32603, "savings data encoding failed"),
        },
        Err(()) => rpc_error(id, -32603, "savings data is unavailable"),
    }
}

fn tool_result(text: String, value: &impl Serialize) -> Value {
    json!({
        "content": [{ "type": "text", "text": text }],
        "structuredContent": value,
        "isError": false,
    })
}

fn rpc_result(id: Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn rpc_error(id: Value, code: i64, message: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": code, "message": message },
    })
}

fn write_message(output: &mut impl Write, message: Value) -> Result<(), ()> {
    serde_json::to_writer(&mut *output, &message).map_err(|_| ())?;
    output.write_all(b"\n").map_err(|_| ())?;
    output.flush().map_err(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::Storage;
    use chrono::Local;
    use std::io::Cursor;

    fn fixture_reader() -> (SavingsReader, PathBuf) {
        fixture_reader_with_worker_lane(WorkerLane::Auto)
    }

    fn fixture_reader_with_worker_lane(worker_lane: WorkerLane) -> (SavingsReader, PathBuf) {
        let path = std::env::temp_dir().join(format!(
            "halv-savings-mcp-{}.sqlite3",
            crate::storage::new_uuid_v4()
        ));
        let storage = Storage::open(&path).expect("fixture migrations should apply");
        let conversation_id = storage
            .ensure_conversation(None, "Covered GUI turn", "gpt-5", None)
            .expect("fixture conversation should insert");
        let assistant = storage
            .insert_message(crate::storage::NewMessage {
                conversation_id: &conversation_id,
                role: "assistant",
                content: "Covered response",
                model: Some("gpt-5"),
                input_tokens: Some(0),
                output_tokens: Some(0),
                error: false,
                mentions: None,
            })
            .expect("fixture assistant should insert");
        storage
            .insert_savings_snapshot_with_crux(
                &assistant.id,
                Some(13),
                Some(0.013),
                Some(crate::pricing::PricingSource::Headroom),
                4,
            )
            .expect("fixture snapshot should insert");
        drop(storage);
        let connection = Connection::open(&path).expect("fixture database should reopen");
        connection
            .execute(
                "INSERT INTO agent_sessions (
                    id, agent_kind, lane, mode, project_id, cwd, cli_session_id,
                    status, created_at, last_active_at, exit_code
                 ) VALUES (?1, 'codex', 'codex_chatgpt', 'terminal_embedded', NULL,
                    '/tmp', NULL, 'running', ?2, ?2, NULL)",
                params!["session-123", Local::now().to_rfc3339()],
            )
            .expect("fixture session should insert");
        connection
            .execute(
                "INSERT INTO agent_session_turns (
                    session_id, turn_index, input_tokens, output_tokens,
                    cache_read_tokens, cache_creation_tokens, cost_usd,
                    savings_tokens, pricing_source, crux_tokens_saved,
                    savings_source, created_at
                 ) VALUES (?1, 0, 0, 0, 0, 0, NULL, 55, 'model_table', 20,
                           'measured', ?2)",
                params!["session-123", Local::now().to_rfc3339()],
            )
            .expect("fixture savings should insert");
        let created_at = Local::now().to_rfc3339();
        connection
            .execute(
                "INSERT INTO agent_sessions (
                   id, agent_kind, lane, mode, project_id, cwd, cli_session_id,
                   status, created_at, last_active_at, exit_code
                 ) VALUES
                   ('gui-standalone', 'codex', 'codex_chatgpt', 'gui', NULL,
                    '/tmp', NULL, 'running', ?1, ?1, NULL),
                   ('gui-covered', 'codex', 'codex_chatgpt', 'gui', NULL,
                    '/tmp', NULL, 'running', ?1, ?1, NULL)",
                params![&created_at],
            )
            .expect("GUI fixture sessions should insert");
        connection
            .execute(
                "INSERT INTO agent_session_turns (
                   session_id, turn_index, input_tokens, output_tokens,
                   cache_read_tokens, cache_creation_tokens, cost_usd,
                   savings_tokens, savings_usd, crux_tokens_saved,
                   savings_source, has_message_snapshot, created_at
                 ) VALUES
                   ('gui-standalone', 0, 0, 0, 0, 0, NULL, 13, 0.013, 3,
                    'measured', 0, ?1),
                   ('gui-covered', 0, 0, 0, 0, 0, NULL, 17, 0.017, 4,
                    'measured', 1, ?1)",
                params![&created_at],
            )
            .expect("GUI fixture savings should insert");
        drop(connection);
        let mut reader = SavingsReader::open(SavingsMcpArgs {
            database_path: path.clone(),
            session_id: "session-123".to_string(),
            rtk_database_path: path.with_extension("rtk"),
            coordinator_model: None,
            conversation_id: Some("conversation-1".to_string()),
            worker_lane,
            tier_agents: TierAgentAssignments::default(),
            task_routing: true,
            workflow_routing: true,
        })
        .expect("fixture reader should open");
        // The temporary folder is shared, so each fixture has its own
        // preferences file. The routing tests start with a "yes".
        reader.preferences_path = path.with_extension("preferences.json");
        set_fixture_brief_sharing(&reader, Some("allowed"));
        (reader, path)
    }

    /// Writes the task brief answer of a fixture. `None` removes the file.
    fn set_fixture_brief_sharing(reader: &SavingsReader, answer: Option<&str>) {
        match answer {
            Some(answer) => std::fs::write(
                &reader.preferences_path,
                json!({ "taskRouting": true, "briefSharing": answer }).to_string(),
            )
            .expect("fixture preferences should write"),
            None => {
                let _ = std::fs::remove_file(&reader.preferences_path);
            }
        }
    }

    #[test]
    fn keychain_environment_keeps_the_variables_that_have_a_value() {
        let environment = keychain_environment(
            &[
                "DBUS_SESSION_BUS_ADDRESS",
                "XDG_RUNTIME_DIR",
                "UNSET_VARIABLE",
            ],
            |name| match name {
                "DBUS_SESSION_BUS_ADDRESS" => Some("unix:path=/run/user/1000/bus".to_string()),
                "XDG_RUNTIME_DIR" => Some(String::new()),
                _ => None,
            },
        );
        assert_eq!(
            environment,
            std::collections::BTreeMap::from([(
                "DBUS_SESSION_BUS_ADDRESS".to_string(),
                "unix:path=/run/user/1000/bus".to_string()
            )])
        );
        // Only the Linux keyring uses the D-Bus session bus.
        assert_eq!(
            KEYCHAIN_ENVIRONMENT.contains(&"DBUS_SESSION_BUS_ADDRESS"),
            cfg!(target_os = "linux")
        );
    }

    #[test]
    fn launch_config_uses_the_halv_executable_and_session_paths() {
        let config = launch_config_with_executable(
            Path::new("/opt/Halv/halv"),
            Path::new("/tmp/halv.sqlite3"),
            "session-123",
            Path::new("/tmp/rtk.sqlite3"),
            Some("sonnet"),
            Some("conversation-1"),
            WorkerLanePreference::Codex,
            TierAgentAssignments::default(),
            true,
            true,
        )
        .expect("launch configuration should build");
        assert_eq!(config.command, "/opt/Halv/halv");
        assert_eq!(config.args[0], "--mcp-savings");
        assert!(config
            .args
            .windows(2)
            .any(|pair| pair == ["--session", "session-123"]));
        assert!(config
            .args
            .windows(2)
            .any(|pair| pair == ["--coordinator-model", "sonnet"]));
        assert!(config
            .args
            .windows(2)
            .any(|pair| pair == ["--conversation", "conversation-1"]));
        assert!(config
            .args
            .windows(2)
            .any(|pair| pair == ["--worker-lane", "codex"]));

        let without_routing = launch_config_with_executable(
            Path::new("/opt/Halv/halv"),
            Path::new("/tmp/halv.sqlite3"),
            "session-123",
            Path::new("/tmp/rtk.sqlite3"),
            None,
            None,
            WorkerLanePreference::Auto,
            TierAgentAssignments::default(),
            true,
            true,
        )
        .expect("launch configuration should build");
        assert!(!without_routing
            .args
            .iter()
            .any(|argument| argument == "--coordinator-model"));
        assert!(!without_routing
            .args
            .iter()
            .any(|argument| argument == "--worker-lane"));
    }

    #[test]
    fn week_totals_include_only_uncovered_gui_turn_rows() {
        let (reader, path) = fixture_reader();
        let session = reader
            .session_savings()
            .expect("session savings should load");
        let totals = reader.savings_totals().expect("savings totals should load");
        assert_eq!(session.tokens_saved, 55);
        assert_eq!(session.source, "measured");
        assert!(session.known);
        assert!(session.rtk_included_in_total);
        assert_eq!(session.pricing_source.as_deref(), Some("model_table"));
        assert_eq!(totals.session_tokens_saved, 55);
        assert_eq!(totals.this_week_tokens_saved, 85);
        assert_eq!(totals.period, "rolling_7x24h");
        assert!(totals.rtk_included_in_total);
        assert_eq!(totals.crux_tokens_saved, 27);
        assert!(totals.crux_included_in_total);
        assert_eq!(totals.pricing_source.as_deref(), Some("model_table"));
        let payload = serde_json::to_value(&totals).expect("MCP totals should encode");
        assert_eq!(payload["rtk_included_in_total"], true);
        assert_eq!(payload["crux_included_in_total"], true);
        drop(reader);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn dollar_text_marks_a_routing_estimate() {
        assert_eq!(dollars_text(0.42, false), "$0.42 at API prices");
        assert_eq!(dollars_text(0.42, true), "\u{2248} $0.42 at API prices");
        assert_eq!(dollars_text(-0.35, true), "\u{2248} -$0.35 at API prices");
    }

    #[test]
    fn stdio_server_lists_and_calls_read_only_savings_tools() {
        let (reader, path) = fixture_reader();
        let input = [
            json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": { "protocolVersion": MCP_PROTOCOL_VERSION },
            }),
            json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }),
            json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }),
            json!({
                "jsonrpc": "2.0",
                "id": 3,
                "method": "tools/call",
                "params": { "name": "get_session_savings", "arguments": {} },
            }),
            json!({
                "jsonrpc": "2.0",
                "id": 4,
                "method": "resources/read",
                "params": { "uri": SAVINGS_RESOURCE_URI },
            }),
        ]
        .into_iter()
        .map(|message| message.to_string())
        .collect::<Vec<_>>()
        .join("\n");
        let mut output = Vec::new();
        serve(Cursor::new(input), &mut output, &reader).expect("server should complete");
        let responses = String::from_utf8(output)
            .expect("responses should use UTF-8")
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).expect("response should be JSON"))
            .collect::<Vec<_>>();
        assert_eq!(responses.len(), 4);
        assert_eq!(
            responses[1]["result"]["tools"].as_array().map(Vec::len),
            Some(6)
        );
        assert!(responses[0]["result"]["instructions"]
            .as_str()
            .unwrap()
            .contains("Only the root coordinator routes"));
        assert_eq!(
            responses[2]["result"]["structuredContent"]["tokens_saved"],
            55
        );
        let text = responses[2]["result"]["content"][0]["text"]
            .as_str()
            .expect("the tool should answer with text");
        assert!(
            text.starts_with("Current Halv session savings: $"),
            "{text}"
        );
        assert!(
            text.contains("at API prices (55 measured tokens)"),
            "{text}"
        );
        assert_eq!(
            responses[2]["result"]["structuredContent"]["rtk_included_in_total"],
            true
        );
        assert_eq!(
            responses[3]["result"]["contents"][0]["uri"],
            SAVINGS_RESOURCE_URI
        );
        drop(reader);
        let _ = std::fs::remove_file(path);
    }
    /// Serves one fixed route decision over HTTP so the routing tests never
    /// reach the network.
    fn spawn_route_server() -> String {
        use std::io::Read;
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").expect("mock route server should bind");
        let address = listener
            .local_addr()
            .expect("mock route server should report its address");
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { break };
                let mut buffer = [0_u8; 4096];
                let _ = stream.read(&mut buffer);
                let body = json!({
                    "schema": "task-tier-v1",
                    "provider": "jev",
                    "model": "typesafe/jev-1.13-20260917",
                    "complexity": {
                        "choice": "standard",
                        "confidence": 0.78,
                        "probabilities": { "trivial": 0.1, "standard": 0.8, "hard": 0.1 },
                    },
                    "needs_reasoning": 0.12,
                    "risky": 0.03,
                    "usage": { "input_tokens": 392, "output_tokens": 65 },
                    "latency_ms": 143,
                })
                .to_string();
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes());
                let _ = stream.flush();
            }
        });
        format!("http://127.0.0.1:{}", address.port())
    }

    fn routing_fixture() -> (SavingsReader, PathBuf) {
        routing_fixture_with_worker_lane(WorkerLane::Auto)
    }

    fn routing_fixture_with_worker_lane(worker_lane: WorkerLane) -> (SavingsReader, PathBuf) {
        std::env::set_var(QUOTA_PROBE_ENVIRONMENT_VARIABLE, "off");
        let (reader, path) = fixture_reader_with_worker_lane(worker_lane);
        let server_url = spawn_route_server();
        let connection = Connection::open(&path).expect("fixture database should reopen");
        connection
            .execute(
                "INSERT INTO auth_session (id, server_url, token) VALUES (1, ?1, ?2)",
                params![server_url, "test-token"],
            )
            .expect("auth session should insert");
        drop(connection);
        (reader, path)
    }

    #[test]
    fn halv_session_reads_the_token_from_the_keychain_once() {
        let (mut reader, path) = fixture_reader();
        let store = std::sync::Arc::new(session_token::MemorySessionTokenStore::default());
        reader.session_token = SessionTokenVault::new(store.clone());
        let storage = Storage::open_with_session_token_store(&path, store.clone())
            .expect("fixture database should reopen");
        storage
            .save_auth_session(&crate::storage::AuthSession {
                server_url: "https://api.halv.test/".to_string(),
                token: Some("keychain-token".to_string()),
                email: Some("user@example.test".to_string()),
                plan: Some("pro".to_string()),
                entitlements_json: None,
                synced_at: None,
            })
            .expect("session should save");
        let reads_before = store.reads();

        for _ in 0..2 {
            assert_eq!(
                reader.halv_session(),
                Ok((
                    "https://api.halv.test".to_string(),
                    "keychain-token".to_string()
                ))
            );
        }
        assert_eq!(store.reads(), reads_before + 1);

        assert!(storage
            .clear_auth_session_if_current("https://api.halv.test/", Some("keychain-token"))
            .expect("sign-out should clear the session"));
        assert_eq!(reader.halv_session(), Err(SIGN_IN_MESSAGE.to_string()));

        drop(storage);
        drop(reader);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn a_changed_database_url_does_not_redirect_the_route_request() {
        let (mut reader, path) = fixture_reader();
        let store = std::sync::Arc::new(session_token::MemorySessionTokenStore::default());
        reader.session_token = SessionTokenVault::new(store.clone());
        let (halv_url, halv_requests) =
            spawn_workflow_route_server(std::time::Duration::from_millis(0));
        let (attacker_url, attacker_requests) =
            spawn_workflow_route_server(std::time::Duration::from_millis(0));
        let storage = Storage::open_with_session_token_store(&path, store.clone())
            .expect("fixture database should reopen");
        storage
            .save_auth_session(&crate::storage::AuthSession {
                server_url: halv_url.clone(),
                token: Some("keychain-token".to_string()),
                email: Some("user@example.test".to_string()),
                plan: Some("pro".to_string()),
                entitlements_json: None,
                synced_at: None,
            })
            .expect("session should save");
        drop(storage);
        // A same-user process points the row of the database at its host.
        Connection::open(&path)
            .expect("fixture database should reopen")
            .execute(
                "UPDATE auth_session SET server_url = ?1 WHERE id = 1",
                params![&attacker_url],
            )
            .expect("server URL should update");

        assert_eq!(
            reader.halv_session(),
            Ok((halv_url, "keychain-token".to_string()))
        );
        reader
            .fetch_decision("Rename one variable.")
            .expect("the route request should reach the bound server");
        let count = |requests: &std::sync::Arc<std::sync::atomic::AtomicUsize>| {
            requests.load(std::sync::atomic::Ordering::SeqCst)
        };
        assert_eq!(count(&halv_requests), 1);
        assert_eq!(count(&attacker_requests), 0);

        drop(reader);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn halv_session_does_not_use_a_bare_token_item() {
        use crate::storage::session_token::SessionTokenStore;

        let (mut reader, path) = fixture_reader();
        let store = std::sync::Arc::new(session_token::MemorySessionTokenStore::default());
        reader.session_token = SessionTokenVault::new(store.clone());
        // An earlier build of this change wrote the bare token. Only the app
        // binds it to a URL.
        store
            .write("halv-session-legacy", "bare-token")
            .expect("legacy item should write");
        Connection::open(&path)
            .expect("fixture database should reopen")
            .execute(
                "INSERT INTO auth_session (id, server_url, token_account, email, plan)
                 VALUES (1, 'https://attacker.test', 'halv-session-legacy', 'user@example.test', 'pro')",
                [],
            )
            .expect("auth session should insert");

        assert_eq!(reader.halv_session(), Err(SIGN_IN_MESSAGE.to_string()));
        assert_eq!(
            store.items().get("halv-session-legacy").map(String::as_str),
            Some("bare-token")
        );

        drop(reader);
        let _ = std::fs::remove_file(path);
    }

    fn call_tool(reader: &SavingsReader, id: i64, name: &str, arguments: Value) -> Value {
        let input = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "tools/call",
            "params": { "name": name, "arguments": arguments },
        })
        .to_string();
        let mut output = Vec::new();
        serve(Cursor::new(input), &mut output, reader).expect("server should complete");
        let text = String::from_utf8(output).expect("responses should use UTF-8");
        serde_json::from_str::<Value>(text.lines().next().expect("one response"))
            .expect("response should be JSON")
    }

    fn stored_routes(path: &Path) -> Vec<crate::storage::TaskRoute> {
        let connection = Connection::open(path).expect("fixture database should reopen");
        crate::storage::list_task_routes_rows(&connection, "session_id = ?1", "session-123")
            .expect("task routes should read")
    }

    #[test]
    fn worker_lane_preserves_explicit_overrides() {
        let (mut reader, path) = fixture_reader_with_worker_lane(WorkerLane::Auto);
        for model in ["gpt-6-astra", "claude-opus-5"] {
            reader.coordinator_model = Some(model.to_string());
            assert_eq!(reader.effective_worker_lane(None), WorkerLane::Auto);
            assert_eq!(reader.effective_worker_lane(Some(" ")), WorkerLane::Auto);
            assert_eq!(reader.effective_worker_lane(Some("auto")), WorkerLane::Auto);
            assert_eq!(
                reader.effective_worker_lane(Some("codex")),
                WorkerLane::Fixed(Lane::Codex)
            );
            assert_eq!(
                reader.effective_worker_lane(Some("claude")),
                WorkerLane::Fixed(Lane::Claude)
            );
        }
        reader.worker_lane = WorkerLane::Fixed(Lane::Codex);
        assert_eq!(
            reader.effective_worker_lane(Some("claude")),
            WorkerLane::Fixed(Lane::Codex)
        );
        drop(reader);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn routing_catalog_reads_cli_models_next_to_the_database() {
        let directory = std::env::temp_dir().join(format!(
            "halv-routing-catalog-{}",
            crate::storage::new_uuid_v4()
        ));
        std::fs::create_dir_all(&directory).expect("catalog fixture directory should exist");
        let models = vec![
            crate::agent_sessions::model_catalog::CliModel {
                id: "gpt-6-sol".to_string(),
                label: "GPT-6-Sol".to_string(),
                provider: "codex_cli".to_string(),
                reasoning_efforts: vec!["medium".to_string(), "xhigh".to_string()],
                default_effort: "medium".to_string(),
            },
            crate::agent_sessions::model_catalog::CliModel {
                id: "gpt-6.1-sol".to_string(),
                label: "GPT-6.1-Sol".to_string(),
                provider: "codex_cli".to_string(),
                reasoning_efforts: vec!["medium".to_string(), "xhigh".to_string()],
                default_effort: "medium".to_string(),
            },
        ];
        std::fs::write(
            directory.join("cli-models.json"),
            serde_json::to_vec(&models).expect("catalog fixture should encode"),
        )
        .expect("catalog fixture should write");

        let catalog = load_routing_catalog(&directory.join("halv.sqlite3"));
        assert_eq!(catalog.codex.medium.model, "gpt-6.1-sol");
        assert_eq!(catalog.codex.strong.model, "gpt-6.1-sol");
        assert_eq!(catalog.claude.cheap.model, "sonnet");

        std::fs::remove_dir_all(directory).expect("catalog fixture should clean up");
    }

    #[test]
    fn hierarchy_mcp_persists_evidence_and_enforces_lifecycle() {
        let (mut reader, path) = routing_fixture_with_worker_lane(WorkerLane::Fixed(Lane::Codex));
        reader.coordinator_model = Some("gpt-6-astra".into());
        let investigation = call_tool(
            &reader,
            1,
            "route_task",
            json!({
                "stage":"investigation", "brief":"Fix the parser", "workerLane":"codex"
            }),
        );
        assert_eq!(investigation["result"]["isError"], false, "{investigation}");
        let root_id = investigation["result"]["structuredContent"]["taskId"]
            .as_str()
            .unwrap()
            .to_owned();
        assert_eq!(
            investigation["result"]["structuredContent"]["provider"],
            "jev-role-policy"
        );
        assert_eq!(
            investigation["result"]["structuredContent"]["effort"],
            "medium"
        );
        let l1_args = json!({"stage":"l1","parentTaskId":root_id,"brief":"Fix the parser","workerLane":"codex"});
        assert_eq!(
            call_tool(&reader, 2, "route_task", l1_args.clone())["result"]["isError"],
            true
        );
        assert_eq!(
            call_tool(
                &reader,
                3,
                "report_task_result",
                json!({"taskId":root_id,"passed":true})
            )["result"]["isError"],
            true
        );
        let report = call_tool(
            &reader,
            4,
            "report_task_result",
            json!({
                "taskId":root_id,"passed":true,"evidence":"Parser owns src/parser.rs. Check cargo test parser."
            }),
        );
        assert_eq!(report["result"]["isError"], false, "{report}");
        assert_eq!(
            call_tool(
                &reader,
                5,
                "route_task",
                json!({"brief":"bypass","workerLane":"codex"})
            )["result"]["isError"],
            true
        );
        let args = SavingsMcpArgs {
            database_path: path.clone(),
            session_id: reader.session_id.clone(),
            rtk_database_path: reader.rtk_database_path.clone(),
            coordinator_model: reader.coordinator_model.clone(),
            conversation_id: reader.conversation_id.clone(),
            worker_lane: reader.worker_lane,
            tier_agents: reader.tier_agents.clone(),
            task_routing: reader.task_routing,
            workflow_routing: reader.workflow_routing,
        };
        let preferences_path = reader.preferences_path.clone();
        drop(reader);
        let mut reader = SavingsReader::open(args).unwrap();
        reader.preferences_path = preferences_path;
        // Completed research persisted by an older build must not block a new
        // user task, and its evidence must remain available for a later L1.
        let next = call_tool(
            &reader,
            20,
            "route_task",
            json!({"stage":"investigation", "brief":"Independent research", "workerLane":"codex"}),
        );
        assert_eq!(next["result"]["isError"], false, "{next}");
        let next_id = next["result"]["structuredContent"]["taskId"]
            .as_str()
            .unwrap();
        assert_ne!(next_id, root_id);
        let finished = call_tool(
            &reader,
            21,
            "report_task_result",
            json!({"taskId":next_id, "passed":true, "evidence":"Research complete; no implementation needed."}),
        );
        assert_eq!(finished["result"]["isError"], false, "{finished}");
        let l1 = call_tool(&reader, 6, "route_task", l1_args);
        assert_eq!(l1["result"]["isError"], false, "{l1}");
        let l1_id = l1["result"]["structuredContent"]["taskId"]
            .as_str()
            .unwrap()
            .to_owned();
        assert_eq!(l1["result"]["structuredContent"]["action"], "spawn");
        let l2_args = json!({"stage":"l2","parentTaskId":l1_id,"brief":"Fix a parser detail",
            "workerLane":"codex","files":["src/parser.rs"],"checks":["cargo test parser"]});
        let l2 = call_tool(&reader, 7, "route_task", l2_args.clone());
        assert_eq!(l2["result"]["isError"], false, "{l2}");
        let l2_id = l2["result"]["structuredContent"]["taskId"]
            .as_str()
            .unwrap()
            .to_owned();
        assert_eq!(
            l2["result"]["structuredContent"]["action"],
            "execute_in_parent"
        );
        let count_before = stored_routes(&path).len();
        let blocked = call_tool(
            &reader,
            22,
            "route_task",
            json!({"stage":"investigation", "brief":"Cannot interrupt live work", "workerLane":"codex"}),
        );
        assert_eq!(blocked["result"]["isError"], true, "{blocked}");
        assert_eq!(
            call_tool(&reader, 8, "route_task", l2_args)["result"]["isError"],
            true
        );
        assert_eq!(stored_routes(&path).len(), count_before);
        assert_eq!(
            call_tool(
                &reader,
                9,
                "report_task_result",
                json!({"taskId":l1_id,"passed":true})
            )["result"]["isError"],
            true
        );
        assert_eq!(
            call_tool(
                &reader,
                10,
                "report_task_result",
                json!({"taskId":l2_id,"passed":true})
            )["result"]["isError"],
            false
        );
        assert_eq!(
            call_tool(
                &reader,
                11,
                "report_task_result",
                json!({"taskId":l1_id,"passed":true})
            )["result"]["isError"],
            false
        );
        let second = call_tool(
            &reader,
            12,
            "route_task",
            json!({"stage":"investigation","brief":"Next user task","workerLane":"codex"}),
        );
        assert_eq!(second["result"]["isError"], false, "{second}");
        assert_ne!(second["result"]["structuredContent"]["taskId"], root_id);
        let graph = reader
            .hierarchy_graph(&reader.routing_connection().unwrap())
            .unwrap();
        assert!(graph.routes.contains_key(&root_id));
        drop(reader);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn invalid_hierarchy_graph_blocks_legacy_routing() {
        let (reader, path) = routing_fixture();
        let connection = Connection::open(&path).expect("fixture database should reopen");
        connection
            .execute(
                "INSERT INTO task_hierarchies (session_id, graph_json, updated_at) VALUES (?1, ?2, ?3)",
                params![reader.session_id, r#"{"version":"future","routes":{}}"#, Utc::now().to_rfc3339()],
            )
            .expect("invalid hierarchy fixture should insert");
        drop(connection);

        let response = call_tool(
            &reader,
            13,
            "route_task",
            json!({ "brief": "Do not bypass the invalid hierarchy" }),
        );
        assert_eq!(response["result"]["isError"], true);
        assert!(stored_routes(&path).is_empty());

        drop(reader);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn hierarchy_report_rejects_missing_boolean_without_mutating_routes() {
        let (reader, path) = routing_fixture_with_worker_lane(WorkerLane::Fixed(Lane::Codex));
        let route = call_tool(
            &reader,
            1,
            "route_task",
            json!({"stage":"investigation","brief":"Inspect parser","workerLane":"codex"}),
        );
        let id = route["result"]["structuredContent"]["taskId"]
            .as_str()
            .unwrap();
        assert_eq!(
            call_tool(&reader, 2, "report_task_result", json!({"taskId":id}))["result"]["isError"],
            true
        );
        assert_eq!(stored_routes(&path)[0].status, "routed");
        drop(reader);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn route_task_records_the_route_and_answers_with_json() {
        let (reader, path) = routing_fixture();
        let response = call_tool(
            &reader,
            5,
            "route_task",
            json!({ "brief": "Fix the login button\nSecond line" }),
        );
        let text = response["result"]["content"][0]["text"]
            .as_str()
            .expect("the tool should answer with text");
        assert!(text.starts_with("{\"taskId\""), "unexpected answer: {text}");
        assert_eq!(response["result"]["isError"], false);

        let routes = stored_routes(&path);
        assert_eq!(routes.len(), 1);
        assert_eq!(routes[0].tier, "cheap");
        assert_eq!(routes[0].lane, "claude");
        assert_eq!(routes[0].model.as_deref(), Some("sonnet"));
        assert_eq!(routes[0].effort.as_deref(), Some("high"));
        assert_eq!(routes[0].status, "routed");
        assert_eq!(routes[0].title, "Fix the login button");
        assert_eq!(routes[0].conversation_id.as_deref(), Some("conversation-1"));
        assert_eq!(routes[0].worker_lane, "auto");
        drop(reader);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn route_task_uses_the_saved_agent_for_the_selected_tier() {
        let (mut reader, path) = routing_fixture();
        reader.tier_agents = TierAgentAssignments {
            cheap: crate::task_routing::TierAgentAssignment {
                lane: WorkerLane::Fixed(Lane::Codex),
                model: Some("gpt-6.1-sol".to_string()),
                effort: Some("xhigh".to_string()),
            },
            ..TierAgentAssignments::default()
        };
        let response = call_tool(
            &reader,
            6,
            "route_task",
            json!({ "brief": "Add a retry to the webhook", "workerLane": "claude" }),
        );
        assert_eq!(response["result"]["isError"], false);

        let routes = stored_routes(&path);
        assert_eq!(routes.len(), 1);
        assert_eq!(routes[0].tier, "cheap");
        assert_eq!(routes[0].lane, "codex");
        assert_eq!(routes[0].model.as_deref(), Some("gpt-6.1-sol"));
        assert_eq!(routes[0].worker_lane, "claude");
        drop(reader);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn route_task_honors_a_fixed_codex_worker_lane() {
        let (reader, path) = routing_fixture();
        let response = call_tool(
            &reader,
            8,
            "route_task",
            json!({ "brief": "Add a retry to the webhook", "workerLane": "codex" }),
        );
        assert_eq!(response["result"]["isError"], false);
        let text = response["result"]["content"][0]["text"]
            .as_str()
            .expect("the route answer should be text");
        assert!(text.contains("Call native spawn_agent now"));
        assert!(text.contains("model gpt-6-luna and reasoning_effort high"));

        let routes = stored_routes(&path);
        assert_eq!(routes.len(), 1);
        assert_eq!(routes[0].lane, "codex");
        assert_eq!(routes[0].model.as_deref(), Some("gpt-6-luna"));
        assert_eq!(routes[0].effort.as_deref(), Some("high"));
        assert_eq!(routes[0].worker_lane, "codex");
        assert!(routes[0].reasons.iter().any(|reason| reason == "lane"));
        drop(reader);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn the_worker_lane_launch_argument_beats_the_tool_argument() {
        let (reader, path) = routing_fixture_with_worker_lane(WorkerLane::Fixed(Lane::Claude));
        let response = call_tool(
            &reader,
            9,
            "route_task",
            json!({ "brief": "Add a retry to the webhook", "workerLane": "codex" }),
        );
        assert_eq!(response["result"]["isError"], false);

        let routes = stored_routes(&path);
        assert_eq!(routes.len(), 1);
        assert_eq!(routes[0].lane, "claude");
        assert_eq!(routes[0].model.as_deref(), Some("sonnet"));
        assert_eq!(routes[0].effort.as_deref(), Some("high"));
        assert_eq!(routes[0].worker_lane, "claude");
        assert!(routes[0].reasons.iter().any(|reason| reason == "lane"));
        drop(reader);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn report_task_result_escalates_a_failed_task() {
        let (reader, path) = routing_fixture();
        let routed = call_tool(
            &reader,
            6,
            "route_task",
            json!({ "brief": "Rework the parser" }),
        );
        let text = routed["result"]["content"][0]["text"]
            .as_str()
            .expect("the tool should answer with text");
        let payload = serde_json::from_str::<Value>(text.lines().next().expect("one JSON line"))
            .expect("the first line should be JSON");
        let task_id = payload["taskId"].as_str().expect("a task id").to_string();

        let reported = call_tool(
            &reader,
            7,
            "report_task_result",
            json!({ "taskId": task_id, "passed": false }),
        );
        assert_eq!(reported["result"]["isError"], false);

        let routes = stored_routes(&path);
        assert_eq!(routes.len(), 2);
        let previous = routes
            .iter()
            .find(|route| route.id == task_id)
            .expect("the first route should stay");
        assert_eq!(previous.status, "failed");
        let escalated = routes
            .iter()
            .find(|route| route.id != task_id)
            .expect("the escalated route should exist");
        assert_eq!(
            escalated.escalated_from_id.as_deref(),
            Some(task_id.as_str())
        );
        assert_eq!(escalated.tier, "medium");
        assert_eq!(escalated.status, "routed");
        drop(reader);
        let _ = std::fs::remove_file(path);
    }

    // An agent keeps its MCP arguments across an app update. When it starts
    // the server again, the new bundle receives the arguments of the old app.
    // See ADR 0012.
    #[test]
    fn argument_sets_from_older_apps_still_parse() {
        let tier_agents = r#"{"cheapest":{"lane":"auto","model":null,"effort":null},"cheap":{"lane":"auto","model":null,"effort":null},"medium":{"lane":"auto","model":null,"effort":null},"strong":{"lane":"auto","model":null,"effort":null}}"#;
        let from_0_7_1 = [
            "--database",
            "/Users/demo/Library/Application Support/ai.halv.app/halv.sqlite3",
            "--session",
            "4b8f2a7e-0c1d-4e5f-9a6b-7c8d9e0f1a2b",
            "--rtk-database",
            "/Users/demo/Library/Application Support/ai.halv.app/rtk/4b8f2a7e/history.db",
            "--coordinator-model",
            "claude-sonnet-4-5",
            "--conversation",
            "conversation-1",
            "--worker-lane",
            "codex",
            "--tier-agents",
            tier_agents,
        ];
        let parsed = parse_args(from_0_7_1.iter().map(OsString::from))
            .expect("the arguments of Halv 0.7.1 should parse");
        assert_eq!(parsed.session_id, "4b8f2a7e-0c1d-4e5f-9a6b-7c8d9e0f1a2b");
        assert_eq!(
            parsed.coordinator_model.as_deref(),
            Some("claude-sonnet-4-5")
        );
        assert_eq!(parsed.conversation_id.as_deref(), Some("conversation-1"));
        assert_eq!(parsed.worker_lane, WorkerLane::Fixed(Lane::Codex));
        assert_eq!(parsed.tier_agents, TierAgentAssignments::default());

        // Halv 0.6.0 to 0.6.4 did not send the tier agents.
        let from_0_6 = [
            "--database",
            "/tmp/halv.sqlite3",
            "--session",
            "session-123",
            "--rtk-database",
            "/tmp/rtk.sqlite3",
            "--conversation",
            "conversation-1",
        ];
        let parsed = parse_args(from_0_6.iter().map(OsString::from))
            .expect("the arguments of Halv 0.6 should parse");
        assert_eq!(parsed.worker_lane, WorkerLane::Auto);
        assert_eq!(parsed.tier_agents, TierAgentAssignments::default());

        let current = launch_config_with_executable(
            Path::new("/opt/Halv/halv"),
            Path::new("/tmp/halv.sqlite3"),
            "session-123",
            Path::new("/tmp/rtk.sqlite3"),
            Some("sonnet"),
            Some("conversation-1"),
            WorkerLanePreference::Claude,
            TierAgentAssignments::default(),
            true,
            true,
        )
        .expect("launch configuration should build");
        // `main` removes the `--mcp-savings` flag before it parses.
        parse_args(current.args.iter().skip(1).map(OsString::from))
            .expect("the arguments of this app should parse");
    }

    // Workflow mode (ADR 0014). The mock server answers by keyword, so the
    // tests never reach the network: "[fail]" gives HTTP 502, "[hard]" gives a
    // hard brief that needs reasoning, and "[risky]" gives a risky brief.
    fn spawn_workflow_route_server(
        delay: std::time::Duration,
    ) -> (String, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
        use std::io::Read;
        use std::net::TcpListener;
        use std::sync::{atomic::AtomicUsize, atomic::Ordering, Arc};

        let listener = TcpListener::bind("127.0.0.1:0").expect("mock route server should bind");
        let address = listener.local_addr().expect("mock address");
        let requests = Arc::new(AtomicUsize::new(0));
        let counter = requests.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { break };
                let counter = counter.clone();
                std::thread::spawn(move || {
                    let mut data = Vec::new();
                    let mut buffer = [0_u8; 8192];
                    loop {
                        let read = stream.read(&mut buffer).unwrap_or(0);
                        if read == 0 {
                            break;
                        }
                        data.extend_from_slice(&buffer[..read]);
                        let text = String::from_utf8_lossy(&data).to_string();
                        if let Some(end) = text.find("\r\n\r\n") {
                            let length = text[..end]
                                .to_ascii_lowercase()
                                .lines()
                                .find_map(|line| {
                                    line.strip_prefix("content-length:")
                                        .map(str::trim)
                                        .and_then(|value| value.parse::<usize>().ok())
                                })
                                .unwrap_or(0);
                            if data.len() >= end + 4 + length {
                                break;
                            }
                        }
                    }
                    counter.fetch_add(1, Ordering::SeqCst);
                    std::thread::sleep(delay);
                    let text = String::from_utf8_lossy(&data).to_string();
                    let brief = text
                        .split("\r\n\r\n")
                        .nth(1)
                        .and_then(|body| serde_json::from_str::<Value>(body).ok())
                        .and_then(|body| body["brief"].as_str().map(str::to_owned))
                        .unwrap_or_default();
                    let response = if brief.contains("[fail]") {
                        "HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                            .to_string()
                    } else {
                        let (choice, needs_reasoning, risky) = if brief.contains("[hard]") {
                            ("hard", 0.9, 0.05)
                        } else if brief.contains("[risky]") {
                            ("standard", 0.1, 0.9)
                        } else {
                            ("standard", 0.1, 0.0)
                        };
                        let body = json!({
                            "schema": "task-tier-v1",
                            "provider": "jev",
                            "model": "typesafe/jev-1.13-20260917",
                            "complexity": {
                                "choice": choice,
                                "confidence": 0.95,
                                "probabilities": { "trivial": 0.0, "standard": 0.5, "hard": 0.5 },
                            },
                            "needs_reasoning": needs_reasoning,
                            "risky": risky,
                            "usage": { "input_tokens": 400, "output_tokens": 60 },
                            "latency_ms": 120,
                        })
                        .to_string();
                        format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                            body.len()
                        )
                    };
                    let _ = stream.write_all(response.as_bytes());
                    let _ = stream.flush();
                });
            }
        });
        (format!("http://127.0.0.1:{}", address.port()), requests)
    }

    struct WorkflowFixture {
        reader: SavingsReader,
        path: PathBuf,
        projects: PathBuf,
        requests: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    }

    impl Drop for WorkflowFixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.path);
            let _ = std::fs::remove_dir_all(&self.projects);
        }
    }

    fn workflow_fixture(delay_ms: u64) -> WorkflowFixture {
        let (mut reader, path) = fixture_reader();
        let (server_url, requests) =
            spawn_workflow_route_server(std::time::Duration::from_millis(delay_ms));
        let connection = Connection::open(&path).expect("fixture database should reopen");
        connection
            .execute(
                "INSERT INTO auth_session (id, server_url, token) VALUES (1, ?1, ?2)",
                params![server_url, "test-token"],
            )
            .expect("auth session should insert");
        connection
            .execute(
                "UPDATE agent_sessions SET cli_session_id = 'cli-1' WHERE id = 'session-123'",
                [],
            )
            .expect("CLI session id should update");
        drop(connection);
        let projects = std::env::temp_dir().join(format!(
            "halv-workflow-projects-{}",
            crate::storage::new_uuid_v4()
        ));
        std::fs::create_dir_all(projects.join("-tmp-project")).unwrap();
        reader.claude_projects_root = Some(projects.clone());
        // A wrong launch hint, as in old Claude panes.
        reader.coordinator_model = Some("gpt-6-astra".to_string());
        WorkflowFixture {
            reader,
            path,
            projects,
            requests,
        }
    }

    fn requests(fixture: &WorkflowFixture) -> usize {
        fixture.requests.load(std::sync::atomic::Ordering::SeqCst)
    }

    fn route_payload(response: &Value) -> Value {
        let text = response["result"]["content"][0]["text"]
            .as_str()
            .expect("the tool should answer with text");
        serde_json::from_str(text.lines().next().unwrap()).expect("the first line is JSON")
    }

    fn workflow_routes(path: &Path) -> Vec<TaskRoute> {
        stored_routes(path)
            .into_iter()
            .filter(|route| route.workflow.is_some())
            .collect()
    }

    #[test]
    fn route_tasks_routes_every_stage_in_one_batch_and_falls_back_per_stage() {
        let fixture = workflow_fixture(0);
        let response = call_tool(
            &fixture.reader,
            1,
            "route_tasks",
            json!({
                "workflow": "fix-findings",
                "coordinatorModel": "claude-opus-5-5",
                "tasks": [
                    {"key": "scout", "role": "read", "brief": "[hard] Map the parser.\nRead-only."},
                    {"key": "fix", "role": "implement", "brief": "Fix one finding.", "instances": 3},
                    {"key": "check", "role": "verify", "brief": "[fail] Review the fixes."},
                ],
            }),
        );
        assert_eq!(response["result"]["isError"], false, "{response}");
        assert_eq!(requests(&fixture), 3);
        let payload = route_payload(&response);
        let routes = &payload["routes"];
        let scout_id = routes["scout"]["taskId"].as_str().unwrap();
        assert_eq!(
            routes["scout"]["opts"],
            json!({"model": "sonnet", "effort": "xhigh", "label": format!("jev:{}:scout", &scout_id[..8])})
        );
        assert_eq!(
            routes["scout"]["retry"]["label"],
            format!("jev:{}:scout:retry", &scout_id[..8])
        );
        assert_eq!(routes["fix"]["opts"]["model"], "sonnet");
        assert_eq!(routes["fix"]["opts"]["effort"], "high");
        assert_eq!(routes["fix"]["retry"]["model"], "opus");
        assert_eq!(routes["fix"]["retry"]["effort"], "medium");
        // JEV failed for this stage, so the agent inherits.
        assert_eq!(routes["check"]["opts"].as_object().unwrap().len(), 1);
        assert!(routes["check"]["opts"]["label"].is_string());
        assert_eq!(routes["check"]["retry"], Value::Null);
        let text = response["result"]["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("JEV did not answer for check"), "{text}");
        let structured = &response["result"]["structuredContent"];
        assert_eq!(
            structured["routes"]["check"]["reasons"],
            json!(["fallback"])
        );
        assert_eq!(structured["defaultModel"], "claude-opus-5-5");
        assert_eq!(structured["defaultSource"], "coordinator");

        let rows = workflow_routes(&fixture.path);
        assert_eq!(rows.len(), 3);
        let batch = rows[0].workflow.as_ref().unwrap().batch_id.clone();
        assert!(rows
            .iter()
            .all(|row| row.workflow.as_ref().unwrap().batch_id == batch));
        let fix = rows
            .iter()
            .find(|row| row.workflow.as_ref().unwrap().key == "fix")
            .unwrap();
        assert_eq!(fix.workflow.as_ref().unwrap().instances, 3);
        assert_eq!(fix.workflow.as_ref().unwrap().role, "implement");
        assert_eq!(
            fix.workflow.as_ref().unwrap().name.as_deref(),
            Some("fix-findings")
        );
        assert_eq!(fix.lane, "claude");
        assert_eq!(fix.default_model.as_deref(), Some("claude-opus-5-5"));
        let check = rows
            .iter()
            .find(|row| row.workflow.as_ref().unwrap().key == "check")
            .unwrap();
        assert_eq!(check.tier, "max");
        assert_eq!(check.provider, "fallback");
        assert!(rows.iter().all(|row| row.status == "routed"));
    }

    #[test]
    fn route_tasks_asks_jev_for_eight_briefs_at_once() {
        let fixture = workflow_fixture(300);
        let tasks = (0..8)
            .map(|index| json!({"key": format!("item-{index}"), "role": "read", "brief": format!("Inspect item {index}.")}))
            .collect::<Vec<_>>();
        let started = std::time::Instant::now();
        let response = call_tool(&fixture.reader, 1, "route_tasks", json!({ "tasks": tasks }));
        assert_eq!(response["result"]["isError"], false, "{response}");
        assert!(
            started.elapsed() < std::time::Duration::from_millis(1_500),
            "{:?}",
            started.elapsed()
        );
        let keys = workflow_routes(&fixture.path)
            .iter()
            .map(|route| route.workflow.as_ref().unwrap().key.clone())
            .collect::<BTreeSet<_>>();
        assert_eq!(keys.len(), 8);
    }

    #[test]
    fn route_tasks_rejects_invalid_batches_without_writes_or_requests() {
        let fixture = workflow_fixture(0);
        let task = |key: &str, brief: &str| json!({"key": key, "role": "read", "brief": brief});
        let invalid = [
            json!({ "tasks": [] }),
            json!({ "tasks": [task("a", "One"), task("a", "Two")] }),
            json!({ "tasks": (0..65).map(|index| task(&format!("k{index}"), "Brief")).collect::<Vec<_>>() }),
            json!({ "tasks": [task("a", "  ")] }),
            json!({ "tasks": [task("a", &"x".repeat(4_001))] }),
            json!({ "tasks": [task("bad key", "Brief")] }),
            json!({ "tasks": [{"key": "a", "role": "synthesis", "brief": "Brief"}] }),
            json!({ "tasks": [{"key": "a", "role": "read", "brief": "Brief", "instances": 0}] }),
        ];
        for (index, args) in invalid.into_iter().enumerate() {
            let response = call_tool(&fixture.reader, index as i64, "route_tasks", args);
            assert_eq!(response["result"]["isError"], true, "{response}");
        }
        assert_eq!(requests(&fixture), 0);
        assert!(stored_routes(&fixture.path).is_empty());
    }

    #[test]
    fn route_tasks_reads_the_coordinator_model_from_its_transcript() {
        let fixture = workflow_fixture(0);
        let mut line = workflow_receipts::tests::assistant_line(
            "m1",
            "claude-sonnet-5-5",
            1,
            "2026-10-15T10:00:00Z",
        );
        line["effort"] = json!("high");
        std::fs::write(
            fixture.projects.join("-tmp-project").join("cli-1.jsonl"),
            line.to_string(),
        )
        .unwrap();
        let response = call_tool(
            &fixture.reader,
            1,
            "route_tasks",
            json!({
                "coordinatorModel": "claude-opus-5-5",
                "tasks": [{"key": "check", "role": "verify", "brief": "[risky] Review the change."}],
            }),
        );
        let payload = route_payload(&response);
        // A Sonnet coordinator caps a verify agent at Sonnet.
        assert_eq!(payload["routes"]["check"]["opts"]["model"], "sonnet");
        let route = &workflow_routes(&fixture.path)[0];
        assert_eq!(route.default_model.as_deref(), Some("claude-sonnet-5-5"));
        assert_eq!(route.default_effort.as_deref(), Some("high"));
        assert_eq!(route.default_source.as_deref(), Some("observed"));
        assert!(route.reasons.iter().any(|reason| reason == "cap"));
    }

    #[test]
    fn an_active_codex_hierarchy_does_not_block_a_workflow_batch() {
        let fixture = workflow_fixture(0);
        let mut graph = Graph::default();
        let prepared = graph
            .prepare(&json!({"stage": "investigation", "brief": "Inspect"}))
            .unwrap();
        graph
            .accept(
                &prepared,
                json!({"taskId": "inv", "tier": "cheapest", "lane": "codex", "model": "gpt-6-luna", "effort": "medium"}),
            )
            .unwrap();
        assert!(graph.has_active_root());
        let connection = Connection::open(&fixture.path).unwrap();
        fixture.reader.save_hierarchy(&connection, &graph).unwrap();
        drop(connection);
        let response = call_tool(
            &fixture.reader,
            1,
            "route_tasks",
            json!({ "tasks": [{"key": "scout", "role": "read", "brief": "Map it."}] }),
        );
        assert_eq!(response["result"]["isError"], false, "{response}");
    }

    #[test]
    fn workflow_routing_off_hides_and_refuses_the_workflow_tools() {
        let mut fixture = workflow_fixture(0);
        fixture.reader.workflow_routing = false;
        let response = call_tool(
            &fixture.reader,
            1,
            "route_tasks",
            json!({ "tasks": [{"key": "scout", "role": "read", "brief": "Map it."}] }),
        );
        assert_eq!(response["result"]["isError"], true);
        assert_eq!(
            tools_list(true, false)["tools"].as_array().unwrap().len(),
            4
        );
        assert!(!server_instructions(true, false).contains("route_tasks"));
        assert!(server_instructions(true, true).contains("route_tasks"));
        assert_eq!(requests(&fixture), 0);
    }

    fn route_batch(fixture: &WorkflowFixture) -> Value {
        let response = call_tool(
            &fixture.reader,
            1,
            "route_tasks",
            json!({
                "coordinatorModel": "claude-opus-5-5",
                "tasks": [
                    {"key": "scout", "role": "read", "brief": "Map the parser.", "instances": 2},
                    {"key": "fix", "role": "implement", "brief": "Fix the parser."},
                ],
            }),
        );
        route_payload(&response)
    }

    fn write_routed_agent(
        fixture: &WorkflowFixture,
        agent_id: &str,
        label: &str,
        model: &str,
        output: i64,
    ) {
        workflow_receipts::tests::write_agent(
            &fixture.projects,
            "cli-1",
            "wf_run1",
            agent_id,
            label,
            &[
                workflow_receipts::tests::assistant_line(
                    "m-start",
                    model,
                    1,
                    "2026-10-15T10:00:00Z",
                ),
                workflow_receipts::tests::assistant_line(
                    &format!("m-{agent_id}"),
                    model,
                    output,
                    "2026-10-15T10:00:30Z",
                ),
            ],
        );
    }

    #[test]
    fn report_task_results_records_receipts_and_links_escalations() {
        let fixture = workflow_fixture(0);
        let payload = route_batch(&fixture);
        let scout = payload["routes"]["scout"].clone();
        let fix = payload["routes"]["fix"].clone();
        let scout_label = scout["opts"]["label"].as_str().unwrap();
        let fix_label = fix["opts"]["label"].as_str().unwrap();
        let retry_label = fix["retry"]["label"].as_str().unwrap();
        write_routed_agent(
            &fixture,
            "s1",
            &format!("{scout_label}:a"),
            "claude-sonnet-5-5",
            10,
        );
        write_routed_agent(
            &fixture,
            "s2",
            &format!("{scout_label}:b"),
            "claude-sonnet-5-5",
            20,
        );
        write_routed_agent(&fixture, "f1", fix_label, "claude-sonnet-5-5", 30);
        write_routed_agent(&fixture, "r1", retry_label, "claude-opus-5-5", 40);
        let report = json!({"results": [
            {"taskId": scout["taskId"], "passed": true},
            {"taskId": fix["taskId"], "passed": false, "retried": {"passed": true}},
        ]});
        let response = call_tool(&fixture.reader, 2, "report_task_results", report.clone());
        assert_eq!(response["result"]["isError"], false, "{response}");
        let answers = &response["result"]["structuredContent"]["results"];
        assert_eq!(answers[0]["outcome"], "passed");
        assert_eq!(answers[0]["usage"], "measured");
        assert_eq!(answers[1]["outcome"], "escalated");
        assert_eq!(answers[1]["retryOutcome"], "passed");

        let routes = workflow_routes(&fixture.path);
        assert_eq!(routes.len(), 3);
        let find = |id: &str| routes.iter().find(|route| route.id == id).unwrap().clone();
        let scout_row = find(scout["taskId"].as_str().unwrap());
        assert_eq!(scout_row.status, "passed");
        let receipt = scout_row.receipt.unwrap();
        assert_eq!(receipt.agents, 2);
        // Each agent: two messages of output 1 and N, so 2 + 10 + 20 + 2.
        assert_eq!(receipt.output_tokens, Some(32));
        assert_eq!(receipt.cache_creation_tokens, Some(1_200));
        assert_eq!(receipt.duration_ms, Some(30_000));
        assert_eq!(receipt.models, vec!["claude-sonnet-5-5".to_string()]);
        let fix_row = find(fix["taskId"].as_str().unwrap());
        assert_eq!(fix_row.status, "escalated");
        assert_eq!(fix_row.receipt.as_ref().unwrap().outcome, "escalated");
        assert_eq!(fix_row.receipt.as_ref().unwrap().output_tokens, Some(31));
        let retry_row = routes
            .iter()
            .find(|route| route.escalated_from_id.as_deref() == Some(fix_row.id.as_str()))
            .unwrap();
        assert_eq!(retry_row.status, "passed");
        assert_eq!(retry_row.model.as_deref(), Some("opus"));
        assert_eq!(retry_row.effort.as_deref(), Some("medium"));
        assert_eq!(retry_row.default_model, fix_row.default_model);
        assert_eq!(retry_row.workflow.as_ref().unwrap().key, "fix");
        let retry_receipt = retry_row.receipt.as_ref().unwrap();
        assert_eq!(retry_receipt.output_tokens, Some(41));
        assert_eq!(retry_receipt.models, vec!["claude-opus-5-5".to_string()]);
        let connection = Connection::open(&fixture.path).unwrap();
        let agents =
            crate::storage::list_task_route_agent_rows(&connection, &scout_row.id).unwrap();
        assert_eq!(agents.len(), 2);
        assert_eq!(agents[0].cache_write_5m_tokens, 400);
        assert_eq!(agents[0].cache_write_1h_tokens, 200);
        drop(connection);

        // A repeated report changes nothing.
        let again = call_tool(&fixture.reader, 3, "report_task_results", report);
        assert_eq!(again["result"]["isError"], false, "{again}");
        assert_eq!(workflow_routes(&fixture.path).len(), 3);

        // A conflicting report fails and writes nothing.
        let conflict = call_tool(
            &fixture.reader,
            4,
            "report_task_results",
            json!({"results": [{"taskId": scout["taskId"], "passed": false}]}),
        );
        assert_eq!(conflict["result"]["isError"], true);
        assert_eq!(find(scout["taskId"].as_str().unwrap()).status, "passed");
    }

    #[test]
    fn report_task_results_without_transcripts_uses_reported_numbers() {
        let fixture = workflow_fixture(0);
        let payload = route_batch(&fixture);
        let fix_id = payload["routes"]["fix"]["taskId"]
            .as_str()
            .unwrap()
            .to_string();
        let scout_id = payload["routes"]["scout"]["taskId"]
            .as_str()
            .unwrap()
            .to_string();
        let failed = call_tool(
            &fixture.reader,
            2,
            "report_task_results",
            json!({"results": [
                {"taskId": fix_id, "passed": false, "agents": 1, "failedAgents": 1},
                {"taskId": scout_id, "passed": true, "durationMs": 5000,
                 "usage": {"inputTokens": 5, "outputTokens": 6, "cacheReadTokens": 7, "cacheCreationTokens": 8}},
            ]}),
        );
        assert_eq!(failed["result"]["isError"], false, "{failed}");
        let routes = workflow_routes(&fixture.path);
        let fix = routes.iter().find(|route| route.id == fix_id).unwrap();
        assert_eq!(fix.status, "failed");
        let receipt = fix.receipt.as_ref().unwrap();
        assert_eq!(receipt.usage_source, "none");
        assert_eq!(receipt.failed_agents, 1);
        assert_eq!(receipt.output_tokens, None);
        let scout = routes.iter().find(|route| route.id == scout_id).unwrap();
        let receipt = scout.receipt.as_ref().unwrap();
        assert_eq!(receipt.usage_source, "reported");
        assert_eq!(receipt.agents, 2);
        assert_eq!(receipt.cache_creation_tokens, Some(8));
        assert_eq!(receipt.duration_ms, Some(5_000));

        // A later report of the retry turns the failure into an escalation.
        let retried = call_tool(
            &fixture.reader,
            3,
            "report_task_results",
            json!({"results": [{"taskId": fix_id, "passed": false, "retried": {"passed": false}}]}),
        );
        assert_eq!(retried["result"]["isError"], false, "{retried}");
        let routes = workflow_routes(&fixture.path);
        assert_eq!(
            routes
                .iter()
                .find(|route| route.id == fix_id)
                .unwrap()
                .status,
            "escalated"
        );
        let retry = routes
            .iter()
            .find(|route| route.escalated_from_id.as_deref() == Some(fix_id.as_str()))
            .unwrap();
        assert_eq!(retry.status, "failed");
    }

    #[test]
    fn measured_retry_agents_mark_an_escalation_without_a_retried_report() {
        let fixture = workflow_fixture(0);
        let payload = route_batch(&fixture);
        let fix = payload["routes"]["fix"].clone();
        write_routed_agent(
            &fixture,
            "r1",
            fix["retry"]["label"].as_str().unwrap(),
            "claude-opus-5-5",
            40,
        );
        let response = call_tool(
            &fixture.reader,
            2,
            "report_task_results",
            json!({"results": [{"taskId": fix["taskId"], "passed": true}]}),
        );
        assert_eq!(response["result"]["isError"], false, "{response}");
        let routes = workflow_routes(&fixture.path);
        let first = routes
            .iter()
            .find(|route| route.id == fix["taskId"])
            .unwrap();
        assert_eq!(first.status, "escalated");
        let retry = routes
            .iter()
            .find(|route| route.escalated_from_id.as_deref() == Some(first.id.as_str()))
            .unwrap();
        assert_eq!(retry.status, "passed");
    }

    #[test]
    fn report_task_results_rejects_foreign_and_legacy_routes() {
        let fixture = workflow_fixture(0);
        let legacy = call_tool(
            &fixture.reader,
            1,
            "route_task",
            json!({"brief": "Fix it", "workerLane": "claude"}),
        );
        assert_eq!(legacy["result"]["isError"], false, "{legacy}");
        let legacy_id = legacy["result"]["structuredContent"]["taskId"]
            .as_str()
            .unwrap()
            .to_string();
        let legacy_row = stored_routes(&fixture.path)
            .into_iter()
            .find(|route| route.id == legacy_id)
            .unwrap();
        assert!(legacy_row.default_source.is_some());
        let payload = route_batch(&fixture);
        let scout_id = payload["routes"]["scout"]["taskId"].clone();
        for results in [
            json!([{"taskId": legacy_id, "passed": true}]),
            json!([{"taskId": "unknown", "passed": true}]),
            json!([{"taskId": scout_id, "passed": true}, {"taskId": scout_id, "passed": true}]),
            json!([{"taskId": scout_id}]),
        ] {
            let response = call_tool(
                &fixture.reader,
                2,
                "report_task_results",
                json!({"results": results}),
            );
            assert_eq!(response["result"]["isError"], true, "{response}");
        }
        assert!(workflow_routes(&fixture.path)
            .iter()
            .all(|route| route.status == "routed"));
    }

    #[test]
    fn the_workflow_routing_flag_parses_and_defaults_on() {
        let base = [
            "--database",
            "/tmp/halv.sqlite3",
            "--session",
            "s",
            "--rtk-database",
            "/tmp/rtk",
        ];
        let parsed = parse_args(base.iter().map(OsString::from)).unwrap();
        assert!(parsed.workflow_routing);
        let off = parse_args(
            base.iter()
                .copied()
                .chain(["--workflow-routing", "off"])
                .map(OsString::from),
        )
        .unwrap();
        assert!(!off.workflow_routing);
        assert!(parse_args(
            base.iter()
                .copied()
                .chain(["--workflow-routing", "maybe"])
                .map(OsString::from)
        )
        .is_err());
    }

    #[test]
    fn the_task_routing_flag_parses_defaults_on_and_turns_workflow_routing_off() {
        let base = [
            "--database",
            "/tmp/halv.sqlite3",
            "--session",
            "s",
            "--rtk-database",
            "/tmp/rtk",
        ];
        let parse = |extra: &[&str]| {
            parse_args(base.iter().chain(extra.iter()).copied().map(OsString::from))
        };
        let parsed = parse(&[]).unwrap();
        assert!(parsed.task_routing && parsed.workflow_routing);
        let off = parse(&["--task-routing", "off"]).unwrap();
        assert!(!off.task_routing);
        assert!(!off.workflow_routing, "workflow routing needs task routing");
        assert!(parse(&["--task-routing", "maybe"]).is_err());

        let config = |task_routing: bool, workflow_routing: bool| {
            launch_config_with_executable(
                Path::new("/opt/Halv/halv"),
                Path::new("/tmp/halv.sqlite3"),
                "session-123",
                Path::new("/tmp/rtk.sqlite3"),
                None,
                None,
                WorkerLanePreference::Auto,
                TierAgentAssignments::default(),
                task_routing,
                workflow_routing,
            )
            .expect("launch configuration should build")
            .args
        };
        let has = |args: &[String], pair: [&str; 2]| args.windows(2).any(|item| item == pair);
        // The default argument set stays the same as in older apps.
        let on = config(true, true);
        assert!(!on.iter().any(|argument| argument == "--task-routing"));
        assert!(!on.iter().any(|argument| argument == "--workflow-routing"));
        let off = config(false, true);
        assert!(has(&off, ["--task-routing", "off"]));
        assert!(has(&off, ["--workflow-routing", "off"]));
        let parsed = parse_args(off.iter().skip(1).map(OsString::from)).unwrap();
        assert!(!parsed.task_routing && !parsed.workflow_routing);
    }

    #[test]
    fn task_routing_off_lists_and_answers_only_the_savings_tools() {
        let (server_url, requests) =
            spawn_workflow_route_server(std::time::Duration::from_millis(0));
        let (mut reader, path) = fixture_reader();
        sign_in_fixture(&path, &server_url);
        reader.task_routing = false;
        reader.workflow_routing = false;

        let names = |list: Value| {
            list["tools"]
                .as_array()
                .unwrap()
                .iter()
                .map(|tool| tool["name"].as_str().unwrap().to_string())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            names(tools_list(false, true)),
            vec!["get_session_savings", "get_savings_totals"]
        );
        assert!(names(tools_list(true, false)).contains(&"route_task".to_string()));
        let instructions = server_instructions(false, true);
        assert!(!instructions.contains("route_task"), "{instructions}");

        let input = [
            json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"}),
            json!({"jsonrpc": "2.0", "id": 2, "method": "initialize", "params": {}}),
        ]
        .iter()
        .map(Value::to_string)
        .collect::<Vec<_>>()
        .join("\n");
        let mut output = Vec::new();
        serve(Cursor::new(input), &mut output, &reader).expect("server should complete");
        let text = String::from_utf8(output).unwrap();
        let mut lines = text
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap());
        let listed = lines.next().unwrap();
        assert_eq!(
            names(listed["result"].clone()),
            vec!["get_session_savings", "get_savings_totals"]
        );
        let initialized = lines.next().unwrap();
        assert!(!initialized["result"]["instructions"]
            .as_str()
            .unwrap()
            .contains("route_task"));

        for (id, name, arguments) in [
            (3, "route_task", json!({ "brief": "Fix the login button" })),
            (
                4,
                "report_task_result",
                json!({ "taskId": "x", "passed": true }),
            ),
            (
                5,
                "route_tasks",
                json!({ "tasks": [{"key": "scout", "role": "read", "brief": "Map it."}] }),
            ),
            (6, "report_task_results", json!({ "results": [] })),
        ] {
            let response = call_tool(&reader, id, name, arguments);
            assert_eq!(response["result"]["isError"], true, "{name}: {response}");
            assert_eq!(
                response["result"]["content"][0]["text"], TASK_ROUTING_OFF_MESSAGE,
                "{name}"
            );
        }
        let savings = call_tool(&reader, 7, "get_session_savings", json!({}));
        assert_eq!(savings["result"]["isError"], false, "{savings}");
        assert_eq!(request_count(&requests), 0);
        assert!(stored_routes(&path).is_empty());
        remove_brief_fixture(reader, path);
    }

    /// Answers every route request with one HTTP status and no body.
    fn spawn_refusing_route_server(status: &'static str) -> String {
        use std::io::Read;
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").expect("mock route server should bind");
        let address = listener
            .local_addr()
            .expect("mock route server should report its address");
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { break };
                let mut buffer = [0_u8; 4096];
                let _ = stream.read(&mut buffer);
                let response =
                    format!("HTTP/1.1 {status}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
                let _ = stream.write_all(response.as_bytes());
                let _ = stream.flush();
            }
        });
        format!("http://127.0.0.1:{}", address.port())
    }

    fn sign_in_fixture(path: &Path, server_url: &str) {
        Connection::open(path)
            .expect("fixture database should reopen")
            .execute(
                "INSERT INTO auth_session (id, server_url, token) VALUES (1, ?1, ?2)",
                params![server_url, "test-token"],
            )
            .expect("auth session should insert");
    }

    fn assert_cloud_required_route(
        reader: &SavingsReader,
        path: &Path,
        case: &str,
        expected_reason: &str,
    ) {
        let response = call_tool(
            reader,
            1,
            "route_task",
            json!({ "brief": "Fix the login button" }),
        );
        let result = &response["result"];
        assert_eq!(result["isError"], false, "{case}: {response}");
        let payload = &result["structuredContent"];
        assert_eq!(payload["tier"], "max", "{case}");
        assert_eq!(payload["action"], EXECUTE_IN_PARENT, "{case}");
        assert_eq!(payload["reason"], expected_reason, "{case}");
        assert_eq!(payload["reasons"], json!([expected_reason]), "{case}");
        assert_eq!(payload["provider"], expected_reason, "{case}");
        assert_eq!(payload["model"], Value::Null, "{case}");
        assert_eq!(payload["effort"], Value::Null, "{case}");
        let text = result["content"][0]["text"].as_str().expect("route text");
        assert!(
            text.contains("Do this task yourself at the coordinator tier"),
            "{case}: {text}"
        );
        let expected_message = if expected_reason == SIGN_IN_REQUIRED {
            SIGN_IN_REQUIRED_MESSAGE
        } else {
            HALV_CLOUD_REQUIRED_MESSAGE
        };
        assert!(text.contains(expected_message), "{case}: {text}");
        assert!(!text.contains("spawn_agent now"), "{case}: {text}");

        let routes = stored_routes(path);
        assert_eq!(routes.len(), 1, "{case}");
        assert_eq!(routes[0].tier, "max", "{case}");
        assert_eq!(routes[0].provider, expected_reason, "{case}");
        assert_eq!(routes[0].reasons, vec![expected_reason.to_string()]);

        // The coordinator can still report the task like any other route.
        let report = call_tool(
            reader,
            2,
            "report_task_result",
            json!({ "taskId": payload["taskId"], "passed": true }),
        );
        assert_eq!(report["result"]["isError"], false, "{case}: {report}");
    }

    #[test]
    fn route_task_without_halv_cloud_tells_the_coordinator_to_do_the_task() {
        std::env::set_var(QUOTA_PROBE_ENVIRONMENT_VARIABLE, "off");
        let (reader, path) = fixture_reader();
        // A signed-out user and an expired session must sign in. They do not
        // hear that a paid plan is missing.
        assert_cloud_required_route(&reader, &path, "signed out", SIGN_IN_REQUIRED);
        drop(reader);
        let _ = std::fs::remove_file(path);

        for (status, expected_reason) in [
            ("401 Unauthorized", SIGN_IN_REQUIRED),
            ("402 Payment Required", HALV_CLOUD_REQUIRED),
            ("403 Forbidden", HALV_CLOUD_REQUIRED),
        ] {
            let (reader, path) = fixture_reader();
            sign_in_fixture(&path, &spawn_refusing_route_server(status));
            assert_cloud_required_route(&reader, &path, status, expected_reason);
            drop(reader);
            let _ = std::fs::remove_file(path);
        }

        // Other server failures keep the old error answer.
        let (reader, path) = fixture_reader();
        sign_in_fixture(&path, &spawn_refusing_route_server("502 Bad Gateway"));
        let response = call_tool(
            &reader,
            1,
            "route_task",
            json!({ "brief": "Fix the login button" }),
        );
        assert_eq!(response["result"]["isError"], true, "{response}");
        assert!(stored_routes(&path).is_empty());
        drop(reader);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn hierarchy_without_halv_cloud_executes_in_the_parent() {
        std::env::set_var(QUOTA_PROBE_ENVIRONMENT_VARIABLE, "off");
        let (mut reader, path) = fixture_reader_with_worker_lane(WorkerLane::Fixed(Lane::Codex));
        reader.coordinator_model = Some("gpt-6-astra".into());
        let investigation = call_tool(
            &reader,
            1,
            "route_task",
            json!({"stage":"investigation","brief":"Fix the parser","workerLane":"codex"}),
        );
        assert_eq!(investigation["result"]["isError"], false, "{investigation}");
        let root_id = investigation["result"]["structuredContent"]["taskId"]
            .as_str()
            .unwrap()
            .to_owned();
        let report = call_tool(
            &reader,
            2,
            "report_task_result",
            json!({"taskId":root_id,"passed":true,"evidence":"Parser owns src/parser.rs."}),
        );
        assert_eq!(report["result"]["isError"], false, "{report}");

        let l1 = call_tool(
            &reader,
            3,
            "route_task",
            json!({"stage":"l1","parentTaskId":root_id,"brief":"Fix the parser","workerLane":"codex"}),
        );
        assert_eq!(l1["result"]["isError"], false, "{l1}");
        let payload = &l1["result"]["structuredContent"];
        assert_eq!(payload["action"], EXECUTE_IN_PARENT, "{l1}");
        assert_eq!(payload["tier"], "max", "{l1}");
        // The fixture has no session, so the user must sign in.
        assert_eq!(payload["reason"], SIGN_IN_REQUIRED, "{l1}");
        drop(reader);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn route_tasks_without_halv_cloud_lets_every_stage_inherit() {
        std::env::set_var(QUOTA_PROBE_ENVIRONMENT_VARIABLE, "off");
        let (reader, path) = fixture_reader();
        let response = call_tool(
            &reader,
            1,
            "route_tasks",
            json!({
                "workflow": "fix-findings",
                "tasks": [
                    {"key": "fix", "role": "implement", "brief": "Fix one finding."},
                    {"key": "check", "role": "verify", "brief": "Review the fix."},
                ],
            }),
        );
        assert_eq!(response["result"]["isError"], false, "{response}");
        let structured = &response["result"]["structuredContent"];
        assert_eq!(structured["reason"], SIGN_IN_REQUIRED);
        for key in ["fix", "check"] {
            assert_eq!(structured["fallback"][key], SIGN_IN_REQUIRED_MESSAGE);
            assert_eq!(structured["routes"][key]["reasons"], json!(["fallback"]));
        }
        drop(reader);
        let _ = std::fs::remove_file(path);

        // A refused plan names Halv Cloud.
        let (reader, path) = fixture_reader();
        sign_in_fixture(&path, &spawn_refusing_route_server("402 Payment Required"));
        let response = call_tool(
            &reader,
            1,
            "route_tasks",
            json!({
                "workflow": "fix-findings",
                "tasks": [{"key": "fix", "role": "implement", "brief": "Fix one finding."}],
            }),
        );
        assert_eq!(response["result"]["isError"], false, "{response}");
        let structured = &response["result"]["structuredContent"];
        assert_eq!(structured["reason"], HALV_CLOUD_REQUIRED);
        assert_eq!(structured["fallback"]["fix"], HALV_CLOUD_REQUIRED_MESSAGE);
        drop(reader);
        let _ = std::fs::remove_file(path);
    }

    /// A fixture with a session at a server that counts its requests.
    fn counted_brief_fixture(
        entitlements_json: Option<&str>,
    ) -> (
        SavingsReader,
        PathBuf,
        std::sync::Arc<std::sync::atomic::AtomicUsize>,
    ) {
        std::env::set_var(QUOTA_PROBE_ENVIRONMENT_VARIABLE, "off");
        let (reader, path) = fixture_reader();
        let (server_url, requests) =
            spawn_workflow_route_server(std::time::Duration::from_millis(0));
        Connection::open(&path)
            .expect("fixture database should reopen")
            .execute(
                "INSERT INTO auth_session (id, server_url, token, plan, entitlements_json)
                 VALUES (1, ?1, 'test-token', 'basic', ?2)",
                params![server_url, entitlements_json],
            )
            .expect("auth session should insert");
        (reader, path, requests)
    }

    fn request_count(requests: &std::sync::Arc<std::sync::atomic::AtomicUsize>) -> usize {
        requests.load(std::sync::atomic::Ordering::SeqCst)
    }

    fn remove_brief_fixture(reader: SavingsReader, path: PathBuf) {
        let _ = std::fs::remove_file(&reader.preferences_path);
        drop(reader);
        let _ = std::fs::remove_file(path);
    }

    fn assert_parent_route(response: &Value, reason: &str, message: &str) {
        let result = &response["result"];
        assert_eq!(result["isError"], false, "{response}");
        let payload = &result["structuredContent"];
        assert_eq!(payload["tier"], "max", "{response}");
        assert_eq!(payload["action"], EXECUTE_IN_PARENT, "{response}");
        assert_eq!(payload["reason"], reason, "{response}");
        assert_eq!(payload["reasons"], json!([reason]), "{response}");
        assert_eq!(payload["provider"], reason, "{response}");
        assert_eq!(payload["model"], Value::Null, "{response}");
        let text = result["content"][0]["text"].as_str().expect("route text");
        assert!(text.contains(message), "{text}");
        assert!(
            text.contains("Do this task yourself at the coordinator tier"),
            "{text}"
        );
        assert!(!text.contains("spawn_agent now"), "{text}");
    }

    #[test]
    fn the_mcp_sends_no_brief_without_a_yes() {
        // No file, a damaged file, an unset answer, and a "no" all keep the
        // brief on the device.
        for answer in [None, Some("maybe"), Some("unset"), Some("declined")] {
            let (reader, path, requests) = counted_brief_fixture(None);
            set_fixture_brief_sharing(&reader, answer);
            let response = call_tool(
                &reader,
                1,
                "route_task",
                json!({ "brief": "Fix the login button" }),
            );
            assert_parent_route(&response, BRIEF_SHARING_OFF, BRIEF_SHARING_OFF_MESSAGE);
            let batch = call_tool(
                &reader,
                2,
                "route_tasks",
                json!({
                    "workflow": "fix-findings",
                    "tasks": [
                        {"key": "fix", "role": "implement", "brief": "Fix one finding."},
                        {"key": "check", "role": "verify", "brief": "Review the fix."},
                    ],
                }),
            );
            assert_eq!(batch["result"]["isError"], false, "{batch}");
            let structured = &batch["result"]["structuredContent"];
            assert_eq!(structured["reason"], BRIEF_SHARING_OFF, "{answer:?}");
            for key in ["fix", "check"] {
                assert_eq!(structured["fallback"][key], BRIEF_SHARING_OFF_MESSAGE);
            }
            assert_eq!(request_count(&requests), 0, "{answer:?}");
            // The route stays on the device, so the coordinator can report it.
            let routes = stored_routes(&path);
            assert!(routes
                .iter()
                .any(|route| route.reasons == vec![BRIEF_SHARING_OFF.to_string()]));
            let task_id = response["result"]["structuredContent"]["taskId"].clone();
            let report = call_tool(
                &reader,
                3,
                "report_task_result",
                json!({ "taskId": task_id, "passed": true }),
            );
            assert_eq!(report["result"]["isError"], false, "{report}");
            assert_eq!(request_count(&requests), 0, "{answer:?}");
            remove_brief_fixture(reader, path);
        }
    }

    #[test]
    fn a_no_in_settings_stops_the_next_brief_of_a_running_session() {
        let (reader, path, requests) = counted_brief_fixture(None);
        let response = call_tool(
            &reader,
            1,
            "route_task",
            json!({ "brief": "Rename one variable." }),
        );
        assert_eq!(response["result"]["isError"], false, "{response}");
        assert_eq!(request_count(&requests), 1);
        set_fixture_brief_sharing(&reader, Some("declined"));
        let response = call_tool(
            &reader,
            2,
            "route_task",
            json!({ "brief": "Rename another variable." }),
        );
        assert_parent_route(&response, BRIEF_SHARING_OFF, BRIEF_SHARING_OFF_MESSAGE);
        assert_eq!(request_count(&requests), 1);
        remove_brief_fixture(reader, path);
    }

    #[test]
    fn cached_entitlements_without_halv_cloud_send_no_brief() {
        let no_plan = json!({
            "beta": false,
            "billingRequired": true,
            "canUseApp": false,
            "plan": "none",
            "message": null,
            "checkoutUrl": null,
        })
        .to_string();
        let unpaid = json!({
            "beta": false,
            "billingRequired": true,
            "canUseApp": true,
            "plan": "basic",
            "message": null,
            "checkoutUrl": null,
            "paymentIssue": "unpaid",
        })
        .to_string();
        for entitlements in [no_plan, unpaid] {
            // The answer does not matter: the server would refuse.
            for answer in ["allowed", "unset"] {
                let (reader, path, requests) = counted_brief_fixture(Some(&entitlements));
                set_fixture_brief_sharing(&reader, Some(answer));
                let response = call_tool(
                    &reader,
                    1,
                    "route_task",
                    json!({ "brief": "Fix the login button" }),
                );
                assert_parent_route(&response, HALV_CLOUD_REQUIRED, HALV_CLOUD_REQUIRED_MESSAGE);
                assert_eq!(request_count(&requests), 0, "{entitlements} {answer}");
                remove_brief_fixture(reader, path);
            }
        }
    }

    #[test]
    fn granted_or_unknown_entitlements_send_a_brief_only_after_a_yes() {
        let granted = json!({
            "beta": false,
            "billingRequired": true,
            "canUseApp": true,
            "plan": "basic",
            "message": null,
            "checkoutUrl": null,
        })
        .to_string();
        for entitlements in [Some(granted.as_str()), None, Some("{ damaged")] {
            let (reader, path, requests) = counted_brief_fixture(entitlements);
            set_fixture_brief_sharing(&reader, Some("unset"));
            let response = call_tool(&reader, 1, "route_task", json!({ "brief": "Fix one bug." }));
            assert_parent_route(&response, BRIEF_SHARING_OFF, BRIEF_SHARING_OFF_MESSAGE);
            assert_eq!(request_count(&requests), 0, "{entitlements:?}");
            set_fixture_brief_sharing(&reader, Some("allowed"));
            let response = call_tool(&reader, 2, "route_task", json!({ "brief": "Fix one bug." }));
            assert_eq!(response["result"]["isError"], false, "{response}");
            assert_eq!(response["result"]["structuredContent"]["provider"], "jev");
            assert_eq!(request_count(&requests), 1, "{entitlements:?}");
            remove_brief_fixture(reader, path);
        }
    }

    #[test]
    fn a_signed_out_user_hears_about_sign_in_before_brief_sharing() {
        std::env::set_var(QUOTA_PROBE_ENVIRONMENT_VARIABLE, "off");
        let (reader, path) = fixture_reader();
        set_fixture_brief_sharing(&reader, Some("unset"));
        let response = call_tool(&reader, 1, "route_task", json!({ "brief": "Fix one bug." }));
        assert_parent_route(&response, SIGN_IN_REQUIRED, SIGN_IN_REQUIRED_MESSAGE);
        remove_brief_fixture(reader, path);
    }

    #[test]
    fn cached_cloud_access_follows_the_app_rules() {
        let entitlements = |can_use_app: bool, plan: &str, payment_issue: Option<&str>| {
            json!({
                "beta": false,
                "billingRequired": true,
                "canUseApp": can_use_app,
                "plan": plan,
                "message": null,
                "checkoutUrl": null,
                "paymentIssue": payment_issue,
            })
            .to_string()
        };
        let cases = [
            (None, CachedCloudAccess::Unknown),
            (Some("not json".to_string()), CachedCloudAccess::Unknown),
            (
                Some(entitlements(true, "basic", None)),
                CachedCloudAccess::Granted,
            ),
            (
                Some(entitlements(true, "none", None)),
                CachedCloudAccess::Granted,
            ),
            (
                Some(entitlements(true, "basic", Some("unpaid"))),
                CachedCloudAccess::Denied,
            ),
            (
                Some(entitlements(true, "basic", Some("past_due"))),
                CachedCloudAccess::Granted,
            ),
            (
                Some(entitlements(false, "none", None)),
                CachedCloudAccess::Denied,
            ),
            (
                Some(entitlements(false, "beta_free", None)),
                CachedCloudAccess::Granted,
            ),
            (
                Some(entitlements(false, "beta_free", Some("past_due"))),
                CachedCloudAccess::Denied,
            ),
            (
                Some(entitlements(false, "", None)),
                CachedCloudAccess::Denied,
            ),
        ];
        for (value, expected) in cases {
            assert_eq!(
                cached_cloud_access(value.as_deref(), None),
                expected,
                "{value:?}"
            );
        }
        // The cached plan counts when the entitlements have none.
        assert_eq!(
            cached_cloud_access(Some(&entitlements(false, "", None)), Some("beta_free")),
            CachedCloudAccess::Granted
        );
    }
}
